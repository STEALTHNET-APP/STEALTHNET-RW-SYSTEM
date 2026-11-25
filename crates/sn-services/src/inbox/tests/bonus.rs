use super::*;
use crate::error::ServiceResult;
use crate::settings::{ProviderUpdate, TopupBonusUpdate};
use sn_domain::ledger::LedgerEntry;
use sn_domain::money::Amount;

fn percent(percent: i64) -> TopupBonusUpdate {
    TopupBonusUpdate {
        enabled: percent > 0,
        percent,
        ..TopupBonusUpdate::default()
    }
}

async fn set_terms(kit: &Kit, provider: &str, terms: TopupBonusUpdate) {
    let admin = admin_of(kit).await;
    kit.services
        .settings_ops()
        .update_provider(
            provider,
            &ProviderUpdate {
                topup_bonus: Some(terms),
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
}

async fn set_bonus(kit: &Kit, provider: &str, value: i64) {
    set_terms(kit, provider, percent(value)).await;
}

async fn admin_of(kit: &Kit) -> AdminId {
    let found: Option<i64> = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT min(id) FROM admins")
            .fetch_one(sn_store::Db::conn(&mut conn))
            .await
            .unwrap()
    };
    match found {
        Some(id) => AdminId(id),
        None => admin(kit).await,
    }
}

async fn ledger(kit: &Kit, client_id: ClientId) -> Vec<LedgerEntry> {
    let mut conn = kit.services.store().conn().await.unwrap();
    let mut entries = conn
        .ledger()
        .entries(client_id, Default::default())
        .await
        .unwrap()
        .items;
    entries.reverse();
    entries
}

async fn moves(kit: &Kit, client_id: ClientId) -> Vec<(LedgerKind, i64)> {
    ledger(kit, client_id)
        .await
        .iter()
        .map(|e| (e.kind, e.amount.minor))
        .collect()
}

async fn bonus_sum(kit: &Kit, client_id: ClientId) -> i64 {
    moves(kit, client_id)
        .await
        .into_iter()
        .filter(|(kind, _)| {
            matches!(
                kind,
                LedgerKind::TopupBonus | LedgerKind::TopupBonusReversal
            )
        })
        .map(|(_, minor)| minor)
        .sum()
}

async fn pay(kit: &Kit, provider: &str, payment: &Payment) -> AcceptResult {
    kit.services
        .inbox()
        .apply_outcome(provider, &outcome(payment, ProviderStatus::Paid))
        .await
        .unwrap()
}

async fn paid_with_bonus(
    kit: &Kit,
    provider: &str,
    client_id: ClientId,
    minor: i64,
    percent: i64,
) -> Payment {
    set_bonus(kit, provider, percent).await;
    let payment = match provider {
        KASSA => kassa_topup(kit, client_id, minor).await,
        _ => topup(kit, client_id, minor).await,
    };
    pay(kit, provider, &payment).await;
    payment
}

fn part(payment: &Payment, key: &str, minor: i64) -> WebhookOutcome {
    WebhookOutcome {
        event_key: format!("refund:{key}"),
        ..outcome(
            payment,
            ProviderStatus::PartiallyRefunded {
                refunded: Amount::new(minor, PayCurrency::Usd),
            },
        )
    }
}

async fn spend(kit: &Kit, client_id: ClientId, minor: i64, key: &str) {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.ledger()
        .append(&NewLedgerEntry {
            client_id,
            wallet: Wallet::Main,
            amount: Money::new(-minor, Currency::Usd),
            kind: LedgerKind::Purchase,
            order_id: None,
            payment_id: None,
            admin_id: None,
            comment: None,
            idempotency_key: Some(key.into()),
        })
        .await
        .unwrap();
}

async fn shortfall(kit: &Kit, payment: PaymentId) -> Vec<(Option<Amount>, String)> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payment_notices()
        .for_payment(payment)
        .await
        .unwrap()
        .into_iter()
        .map(|n| (n.amount, n.text))
        .collect()
}

#[tokio::test]
async fn бонус_начисляется_отдельной_записью_один_раз() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8100).await;
    set_bonus(&kit, TEST_PAY, 7).await;
    let payment = topup(&kit, client_id, 1_005).await;
    assert_eq!(balance(&kit.services, client_id).await, 0);
    pay(&kit, TEST_PAY, &payment).await;
    assert_eq!(
        pay(&kit, TEST_PAY, &payment).await,
        AcceptResult::AlreadyApplied {
            payment_id: payment.id
        }
    );
    let admin = admin_of(&kit).await;
    let again = kit
        .services
        .inbox()
        .mark_paid(payment.id, admin, None)
        .await
        .unwrap();
    assert_eq!(
        again,
        AcceptResult::AlreadyApplied {
            payment_id: payment.id
        }
    );
    kit.services.inbox().poll_payment(payment.id).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1_075);
    let entries = ledger(&kit, client_id).await;
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.kind, e.amount.minor))
            .collect::<Vec<_>>(),
        vec![(LedgerKind::TopUp, 1_005), (LedgerKind::TopupBonus, 70)]
    );
    assert_eq!(entries[1].payment_id, Some(payment.id));
    assert_eq!(entries[1].order_id, Some(payment.order_id));
    let notified: Vec<serde_json::Value> = jobs(&kit.services)
        .await
        .into_iter()
        .filter_map(|job| match job {
            JobPayload::Notify {
                template, params, ..
            } if template == TOPUP_TEMPLATE => Some(params),
            _ => None,
        })
        .collect();
    assert_eq!(notified.len(), 1);
    assert_eq!(notified[0]["amount"]["minor"], 1_005);
    assert_eq!(notified[0]["bonus"]["minor"], 70);
    assert_eq!(notified[0]["total"]["minor"], 1_075);
    let paid: Vec<Amount> = events(&kit.services, client_id)
        .await
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::PaymentPaid { amount, .. } => Some(amount),
            _ => None,
        })
        .collect();
    assert_eq!(paid, vec![Amount::new(1_005, PayCurrency::Usd)]);
    let credited: Vec<(LedgerKind, i64)> = events(&kit.services, client_id)
        .await
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::BalanceChanged { kind, delta, .. } => Some((kind, delta.minor)),
            _ => None,
        })
        .collect();
    assert_eq!(
        credited,
        vec![(LedgerKind::TopUp, 1_005), (LedgerKind::TopupBonus, 70)]
    );
    let history = kit
        .services
        .clients()
        .payments(client_id, None, None)
        .await
        .unwrap();
    assert_eq!(
        history.items[0].bonus,
        Some(Amount::new(70, PayCurrency::Usd))
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn параллельные_вебхуки_начисляют_бонус_один_раз() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8101).await;
    set_bonus(&kit, TEST_PAY, 10).await;
    let payment = topup(&kit, client_id, 2_000).await;
    let runs = (0..6).map(|n| {
        let services = kit.services.clone();
        let incoming = WebhookOutcome {
            event_key: format!("paid:{n}"),
            ..outcome(&payment, ProviderStatus::Paid)
        };
        tokio::spawn(async move { services.inbox().apply_outcome(TEST_PAY, &incoming).await })
    });
    let mut accepted = 0;
    for run in runs.collect::<Vec<_>>() {
        if matches!(run.await.unwrap().unwrap(), AcceptResult::Accepted { .. }) {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 1);
    assert_eq!(
        moves(&kit, client_id).await,
        vec![(LedgerKind::TopUp, 2_000), (LedgerKind::TopupBonus, 200)]
    );
    assert_eq!(balance(&kit.services, client_id).await, 2_200);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn процент_берётся_из_счёта_а_не_из_настройки_на_момент_оплаты() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8102).await;
    let old = topup(&kit, client_id, 1_000).await;
    set_bonus(&kit, TEST_PAY, 5).await;
    let promised = topup(&kit, client_id, 1_000).await;
    set_bonus(&kit, TEST_PAY, 50).await;
    let raised = topup(&kit, client_id, 1_000).await;
    set_bonus(&kit, TEST_PAY, 0).await;
    let off = topup(&kit, client_id, 1_000).await;
    for payment in [&old, &promised, &raised, &off] {
        pay(&kit, TEST_PAY, payment).await;
    }
    let bonuses: Vec<(Option<PaymentId>, i64)> = ledger(&kit, client_id)
        .await
        .into_iter()
        .filter(|e| e.kind == LedgerKind::TopupBonus)
        .map(|e| (e.payment_id, e.amount.minor))
        .collect();
    assert_eq!(
        bonuses,
        vec![(Some(promised.id), 50), (Some(raised.id), 500)]
    );
    assert_eq!(balance(&kit.services, client_id).await, 4_550);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_снимает_долю_бонуса_и_повтор_события_её_не_снимает() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8103).await;
    let payment = paid_with_bonus(&kit, TEST_PAY, client_id, 1_005, 7).await;
    let inbox = kit.services.inbox();
    for (key, left) in [("1", 672 + 46), ("2", 339 + 23)] {
        let refund = part(&payment, key, 333);
        inbox.apply_outcome(TEST_PAY, &refund).await.unwrap();
        assert_eq!(balance(&kit.services, client_id).await, left);
        assert_eq!(
            inbox.apply_outcome(TEST_PAY, &refund).await.unwrap(),
            AcceptResult::AlreadyApplied {
                payment_id: payment.id
            }
        );
        assert_eq!(balance(&kit.services, client_id).await, left);
    }
    let history = kit
        .services
        .clients()
        .payments(client_id, None, None)
        .await
        .unwrap();
    assert_eq!(
        history.items[0].bonus,
        Some(Amount::new(23, PayCurrency::Usd))
    );
    let full = outcome(&payment, ProviderStatus::Refunded);
    inbox.apply_outcome(TEST_PAY, &full).await.unwrap();
    inbox.apply_outcome(TEST_PAY, &full).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(bonus_sum(&kit, client_id).await, 0);
    let taken: Vec<i64> = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.refunds()
            .for_payment(payment.id)
            .await
            .unwrap()
            .iter()
            .map(|r| r.bonus_minor)
            .collect()
    };
    assert_eq!(taken, vec![24, 23, 23]);
    let history = kit
        .services
        .clients()
        .payments(client_id, None, None)
        .await
        .unwrap();
    assert_eq!(history.items[0].bonus, None);
    assert!(shortfall(&kit, payment.id).await.is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn параллельные_уведомления_о_возврате_снимают_бонус_один_раз() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8104).await;
    let payment = paid_with_bonus(&kit, TEST_PAY, client_id, 1_000, 10).await;
    let runs: Vec<_> = (0..5)
        .map(|_| {
            let services = kit.services.clone();
            let incoming = outcome(&payment, ProviderStatus::Refunded);
            tokio::spawn(async move { services.inbox().apply_outcome(TEST_PAY, &incoming).await })
        })
        .collect();
    for run in runs {
        run.await.unwrap().unwrap();
    }
    assert_eq!(
        moves(&kit, client_id).await,
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::TopupBonus, 100),
            (LedgerKind::Purchase, -1_000),
            (LedgerKind::TopupBonusReversal, -100),
        ]
    );
    assert_eq!(balance(&kit.services, client_id).await, 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn при_нехватке_денег_снимается_сколько_есть_и_остаток_уходит_на_решение() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8105).await;
    let payment = paid_with_bonus(&kit, TEST_PAY, client_id, 1_000, 10).await;
    spend(&kit, client_id, 60, "spent-1").await;
    let full = outcome(&payment, ProviderStatus::Refunded);
    kit.services
        .inbox()
        .apply_outcome(TEST_PAY, &full)
        .await
        .unwrap();
    kit.services
        .inbox()
        .apply_outcome(TEST_PAY, &full)
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        moves(&kit, client_id).await[3..],
        [
            (LedgerKind::Purchase, -1_000),
            (LedgerKind::TopupBonusReversal, -40),
        ]
    );
    let notices = shortfall(&kit, payment.id).await;
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].0, Some(Amount::new(60, PayCurrency::Usd)));
    assert!(notices[0].1.contains("$0.60"), "{}", notices[0].1);
    assert!(notices[0].1.contains("бонус"), "{}", notices[0].1);

    let empty = client(&kit.services, 8106).await;
    let second = topup(&kit, empty, 1_000).await;
    pay(&kit, TEST_PAY, &second).await;
    spend(&kit, empty, 900, "spent-2").await;
    kit.services
        .inbox()
        .apply_outcome(TEST_PAY, &outcome(&second, ProviderStatus::Refunded))
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, empty).await, 0);
    assert_eq!(
        moves(&kit, empty).await[3..],
        [(LedgerKind::Purchase, -200)]
    );
    assert_eq!(
        shortfall(&kit, second.id).await[0].0,
        Some(Amount::new(900, PayCurrency::Usd))
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn недобранный_при_возврате_бонус_не_снимается_следующим_возвратом() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8107).await;
    let payment = paid_with_bonus(&kit, TEST_PAY, client_id, 1_000, 10).await;
    spend(&kit, client_id, 1_100, "spent").await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(TEST_PAY, &part(&payment, "1", 500))
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.ledger()
        .append(&NewLedgerEntry {
            client_id,
            wallet: Wallet::Main,
            amount: Money::new(5_000, Currency::Usd),
            kind: LedgerKind::AdminAdjust,
            order_id: None,
            payment_id: None,
            admin_id: Some(admin_of(&kit).await),
            comment: Some("Компенсация".into()),
            idempotency_key: None,
        })
        .await
        .unwrap();
    drop(conn);
    inbox
        .apply_outcome(TEST_PAY, &part(&payment, "2", 500))
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 5_000 - 550);
    assert_eq!(shortfall(&kit, payment.id).await.len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_из_админки_удерживает_бонус_а_отказ_кассы_его_возвращает() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin_of(&kit).await;
    let client_id = client(&kit.services, 8108).await;
    let payment = paid_with_bonus(&kit, KASSA, client_id, 1_000, 5).await;
    let inbox = kit.services.inbox();
    let options = inbox.refund_options(payment.id).await.unwrap();
    let bonus = options.bonus.unwrap();
    assert_eq!(bonus.left.minor, 50);

    spend(&kit, client_id, 20, "spent").await;
    let refused = inbox
        .refund(
            payment.id,
            &refund_command(None, RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "refund.topup_spent");
    assert!(refused.to_string().contains("$0.50"), "{refused}");
    assert_eq!(balance(&kit.services, client_id).await, 1_030);
    assert!(kassa.refunds.lock().unwrap().is_empty());

    let requested = inbox
        .refund(
            payment.id,
            &refund_command(Some(600), RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    assert_eq!(balance(&kit.services, client_id).await, 1_030 - 630);
    assert_eq!(kassa.refunds.lock().unwrap()[0].amount.minor, 600);
    let held = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments()
            .held_refunds_minor(client_id, PayCurrency::Usd)
            .await
            .unwrap()
    };
    assert_eq!(held, 630);
    let options = inbox.refund_options(payment.id).await.unwrap();
    assert_eq!(options.remaining.minor, 400);
    assert_eq!(options.bonus.unwrap().left.minor, 20);
    let declined = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        amount: Some(Amount::new(600, PayCurrency::Usd)),
        ..outcome(&payment, ProviderStatus::RefundDeclined)
    };
    inbox.apply_outcome(KASSA, &declined).await.unwrap();
    inbox.apply_outcome(KASSA, &declined).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1_030);
    assert_eq!(bonus_sum(&kit, client_id).await, 50);
    assert_eq!(
        inbox
            .refund_options(payment.id)
            .await
            .unwrap()
            .bonus
            .unwrap()
            .left
            .minor,
        50
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn подтверждённый_возврат_из_админки_снимает_бонус_один_раз() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin_of(&kit).await;
    let client_id = client(&kit.services, 8109).await;
    let payment = paid_with_bonus(&kit, KASSA, client_id, 1_000, 5).await;
    let inbox = kit.services.inbox();
    inbox
        .refund(
            payment.id,
            &refund_command(Some(400), RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    let confirmed = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        ..part(&payment, "kr-1", 400)
    };
    inbox.apply_outcome(KASSA, &confirmed).await.unwrap();
    inbox.apply_outcome(KASSA, &confirmed).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 630);
    assert_eq!(
        moves(&kit, client_id).await,
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::TopupBonus, 50),
            (LedgerKind::RefundHold, -400),
            (LedgerKind::TopupBonusReversal, -20),
        ]
    );
    let second = inbox
        .refund(
            payment.id,
            &refund_command(None, RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    inbox
        .cancel_refund_request(second.request_id.unwrap(), admin, "деньги остались у нас")
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 630);
    *kassa.refund_completed.lock().unwrap() = true;
    inbox
        .refund(
            payment.id,
            &refund_command(None, RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(bonus_sum(&kit, client_id).await, 0);
    assert!(
        inbox
            .refund_options(payment.id)
            .await
            .unwrap()
            .bonus
            .is_none()
    );
    let refund = kassa.refunds.lock().unwrap().last().unwrap().amount;
    assert_eq!(refund.minor, 600);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn подтверждение_меньшей_части_возвращает_лишнее_удержание_бонуса() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin_of(&kit).await;
    let client_id = client(&kit.services, 8110).await;
    let payment = paid_with_bonus(&kit, KASSA, client_id, 1_000, 10).await;
    let inbox = kit.services.inbox();
    inbox
        .refund(
            payment.id,
            &refund_command(Some(800), RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 220);
    inbox
        .apply_outcome(KASSA, &part(&payment, "elsewhere", 500))
        .await
        .unwrap();
    inbox
        .apply_outcome(
            KASSA,
            &WebhookOutcome {
                event_key: "refund:kr-1".into(),
                amount: Some(Amount::new(800, PayCurrency::Usd)),
                ..outcome(&payment, ProviderStatus::Refunded)
            },
        )
        .await
        .unwrap();
    assert!(bonus_sum(&kit, client_id).await >= 0);
    let taken: i64 = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.refunds()
            .for_payment(payment.id)
            .await
            .unwrap()
            .iter()
            .map(|r| r.bonus_minor)
            .sum()
    };
    assert_eq!(taken, 100);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_отмеченный_вне_панели_снимает_бонус() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin_of(&kit).await;
    let client_id = client(&kit.services, 8111).await;
    let payment = paid_with_bonus(&kit, KASSA, client_id, 1_000, 5).await;
    let command = RefundCommand {
        outside: true,
        key: Some(Uuid::new_v4()),
        ..refund_command(None, RefundSubscriptionAction::Keep, None)
    };
    let inbox = kit.services.inbox();
    inbox.refund(payment.id, &command, admin).await.unwrap();
    inbox.refund(payment.id, &command, admin).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(bonus_sum(&kit, client_id).await, 0);
    inbox
        .apply_outcome(
            KASSA,
            &WebhookOutcome {
                event_key: "refund:late".into(),
                ..outcome(&payment, ProviderStatus::Refunded)
            },
        )
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    let to_balance = inbox
        .refund(
            payment.id,
            &RefundCommand {
                method: RefundMethod::Balance,
                ..refund_command(None, RefundSubscriptionAction::Keep, None)
            },
            admin,
        )
        .await
        .unwrap_err();
    assert_eq!(to_balance.code(), "refund.not_paid");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn недоплата_и_излишек_бонуса_не_дают() {
    let Some(kit) = kit().await else { return };
    set_bonus(&kit, providers::CRYPTOPAY, 10).await;
    set_bonus(&kit, TEST_PAY, 10).await;
    let inbox = kit.services.inbox();

    let short = client(&kit.services, 8112).await;
    let payment = topup(&kit, short, 1_000).await;
    inbox
        .apply_outcome(
            TEST_PAY,
            &outcome(
                &payment,
                ProviderStatus::Underpaid {
                    received: Amount::new(900, PayCurrency::Usd),
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(moves(&kit, short).await, vec![(LedgerKind::TopUp, 900)]);

    let over = client(&kit.services, 8113).await;
    let payment = topup(&kit, over, 1_000).await;
    inbox
        .apply_outcome(
            TEST_PAY,
            &outcome(
                &payment,
                ProviderStatus::Overpaid {
                    received: Amount::new(1_500, PayCurrency::Usd),
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        moves(&kit, over).await,
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::TopupBonus, 100),
            (LedgerKind::TopUp, 500),
        ]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn крипта_даёт_бонус_от_суммы_счёта() {
    let Some(kit) = kit().await else { return };
    set_bonus(&kit, providers::CRYPTOPAY, 10).await;
    let client_id = client(&kit.services, 8114).await;
    let mut tx = kit.services.store().begin().await.unwrap();
    let amount = Money::new(1_000, Currency::Usd);
    let order = tx
        .orders()
        .create(&NewOrder {
            client_id,
            subscription_id: None,
            kind: OrderKind::TopUp,
            intent: PurchaseIntent::TopUp { amount },
            quote: sn_domain::pricing::topup_quote(
                amount,
                sn_domain::pricing::TopUpLimits {
                    min: amount,
                    max: amount,
                },
            )
            .unwrap(),
            snapshot: None,
        })
        .await
        .unwrap();
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: order.id,
            client_id,
            provider: ProviderId::from(providers::CRYPTOPAY),
            method: None,
            amount: amount.into(),
            expires_at: None,
        })
        .await
        .unwrap();
    let terms = kit
        .services
        .payments()
        .topup_bonus(providers::CRYPTOPAY, None, Currency::Usd)
        .unwrap();
    tx.payments()
        .promise_bonus(payment.id, &terms)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for update in [1, 2] {
        kit.services
            .inbox()
            .handle(crypto_hook(payment.public_id, "10.00", update))
            .await
            .unwrap();
    }
    assert_eq!(
        moves(&kit, client_id).await,
        vec![(LedgerKind::TopUp, 1_000), (LedgerKind::TopupBonus, 100)]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn покупка_и_повторная_оплата_бонуса_не_дают() {
    let Some(kit) = kit().await else { return };
    set_bonus(&kit, TEST_PAY, 50).await;
    let client_id = client(&kit.services, 8115).await;
    let tariff = standard(&kit.services).await;
    let bought = invoice(&kit, client_id, &tariff).await;
    pay(&kit, TEST_PAY, &bought).await;
    assert!(moves(&kit, client_id).await.is_empty());
    let promised = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().bonus(bought.id).await.unwrap()
    };
    assert_eq!(promised.terms, None);

    let payment = topup(&kit, client_id, 1_000).await;
    pay(&kit, TEST_PAY, &payment).await;
    kit.services
        .inbox()
        .apply_outcome(TEST_PAY, &paid_by(&payment, "another-tx"))
        .await
        .unwrap();
    assert_eq!(
        moves(&kit, client_id).await,
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::TopupBonus, 500),
            (LedgerKind::TopUp, 1_000),
        ]
    );
    let repeat = by_txid(&kit, TEST_PAY, "another-tx").await.unwrap();
    kit.services
        .inbox()
        .apply_outcome(
            TEST_PAY,
            &WebhookOutcome {
                event_key: "refund:repeat".into(),
                ..outcome(&repeat, ProviderStatus::Refunded)
            },
        )
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1_500);
    assert_eq!(bonus_sum(&kit, client_id).await, 500);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заблокированный_клиент_получает_бонус_без_сообщения() {
    let Some(kit) = kit().await else { return };
    set_bonus(&kit, TEST_PAY, 5).await;
    let client_id = client(&kit.services, 8116).await;
    let payment = topup(&kit, client_id, 1_000).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE clients SET status = 'blocked', blocked_at = now() WHERE id = $1")
            .bind(client_id.0)
            .execute(sn_store::Db::conn(&mut conn))
            .await
            .unwrap();
    }
    pay(&kit, TEST_PAY, &payment).await;
    assert_eq!(balance(&kit.services, client_id).await, 1_050);
    assert!(notifications(&jobs(&kit.services).await, client_id).is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ручное_подтверждение_и_выдача_с_проверки_начисляют_бонус_один_раз() {
    let Some(kit) = kit().await else { return };
    set_bonus(&kit, TEST_PAY, 5).await;
    let admin = admin_of(&kit).await;
    let client_id = client(&kit.services, 8117).await;
    let payment = topup(&kit, client_id, 1_000).await;
    set_bonus(&kit, TEST_PAY, 0).await;
    kit.services
        .inbox()
        .mark_paid(payment.id, admin, Some("видно в кабинете кассы"))
        .await
        .unwrap();
    pay(&kit, TEST_PAY, &payment).await;
    assert_eq!(balance(&kit.services, client_id).await, 1_050);

    set_bonus(&kit, TEST_PAY, 5).await;
    let held = topup(&kit, client_id, 2_000).await;
    kit.services
        .inbox()
        .apply_outcome(
            TEST_PAY,
            &WebhookOutcome {
                event_key: "refund:early".into(),
                ..outcome(&held, ProviderStatus::Refunded)
            },
        )
        .await
        .unwrap();
    pay(&kit, TEST_PAY, &held).await;
    assert_eq!(
        order(&kit.services, held.order_id).await.status,
        OrderStatus::Review
    );
    assert_eq!(balance(&kit.services, client_id).await, 1_050);
    kit.services
        .fulfillment()
        .redeliver(held.order_id, crate::Actor::Admin(admin))
        .await
        .unwrap();
    assert_eq!(
        kit.services.fulfillment().run(held.order_id).await.unwrap(),
        FulfillmentOutcome::Done {
            subscription_id: None
        }
    );
    kit.services.fulfillment().run(held.order_id).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1_050 + 2_100);
    kit.db.cleanup().await;
}

async fn topup_by(kit: &Kit, client_id: ClientId, minor: i64, method: &str) -> Payment {
    let result = kit
        .services
        .checkout()
        .topup(
            client_id,
            Money::new(minor, Currency::Usd),
            TEST_PAY,
            Some(method),
            Channel::Cabinet,
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().by_public_id(payment_id).await.unwrap()
}

#[tokio::test]
async fn порог_и_предел_запоминаются_в_счёте() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8120).await;
    let limited = TopupBonusUpdate {
        min_minor: Some(1_000),
        max_minor: Some(150),
        ..percent(10)
    };
    set_terms(&kit, TEST_PAY, limited.clone()).await;
    let below = topup(&kit, client_id, 999).await;
    let exact = topup(&kit, client_id, 1_000).await;
    let capped = topup(&kit, client_id, 4_000).await;
    set_terms(
        &kit,
        TEST_PAY,
        TopupBonusUpdate {
            min_minor: Some(100),
            max_minor: Some(5_000),
            ..percent(50)
        },
    )
    .await;
    for payment in [&below, &exact, &capped] {
        pay(&kit, TEST_PAY, payment).await;
        pay(&kit, TEST_PAY, payment).await;
    }
    let bonuses: Vec<(Option<PaymentId>, i64)> = ledger(&kit, client_id)
        .await
        .into_iter()
        .filter(|e| e.kind == LedgerKind::TopupBonus)
        .map(|e| (e.payment_id, e.amount.minor))
        .collect();
    assert_eq!(bonuses, vec![(Some(exact.id), 100), (Some(capped.id), 150)]);
    let stored = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().bonus(capped.id).await.unwrap()
    };
    assert_eq!(stored.credited_minor, 150);
    let terms = stored.terms.unwrap();
    assert_eq!(
        (terms.percent, terms.min_minor, terms.max_minor),
        (10, Some(1_000), Some(150))
    );

    let inbox = kit.services.inbox();
    let before = balance(&kit.services, client_id).await;
    inbox
        .apply_outcome(TEST_PAY, &part(&capped, "half", 2_000))
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, before - 2_000 - 75);
    inbox
        .apply_outcome(TEST_PAY, &part(&exact, "most", 900))
        .await
        .unwrap();
    assert_eq!(
        balance(&kit.services, client_id).await,
        before - 2_000 - 75 - 900 - 90
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn бонус_действует_только_для_выбранных_способов() {
    let Some(kit) = kit().await else { return };
    let admin = admin_of(&kit).await;
    let client_id = client(&kit.services, 8121).await;
    kit.services
        .settings_ops()
        .update_provider(
            TEST_PAY,
            &ProviderUpdate {
                methods: Some(
                    ["sbp", "card"]
                        .into_iter()
                        .map(|code| sn_domain::payment::MethodChoice {
                            code: code.into(),
                            title: None,
                        })
                        .collect(),
                ),
                topup_bonus: Some(TopupBonusUpdate {
                    methods: vec!["sbp".into()],
                    ..percent(10)
                }),
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
    let options = kit
        .services
        .checkout()
        .topup_options(client_id, Channel::Cabinet)
        .await
        .unwrap();
    let shown: Vec<(Option<String>, Option<u8>)> = options
        .methods
        .iter()
        .filter(|m| m.provider == TEST_PAY)
        .map(|m| (m.method.clone(), m.bonus.map(|b| b.percent)))
        .collect();
    assert_eq!(
        shown,
        vec![(Some("sbp".into()), Some(10)), (Some("card".into()), None)]
    );
    let card = topup_by(&kit, client_id, 1_000, "card").await;
    let sbp = topup_by(&kit, client_id, 1_000, "sbp").await;
    pay(&kit, TEST_PAY, &card).await;
    pay(&kit, TEST_PAY, &sbp).await;
    assert_eq!(
        moves(&kit, client_id).await,
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::TopupBonus, 100),
        ]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn выключение_не_стирает_настройки_и_не_отменяет_обещанное() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 8122).await;
    let terms = TopupBonusUpdate {
        min_minor: Some(500),
        max_minor: Some(900),
        ..percent(10)
    };
    set_terms(&kit, TEST_PAY, terms.clone()).await;
    let promised = topup(&kit, client_id, 1_000).await;
    set_terms(
        &kit,
        TEST_PAY,
        TopupBonusUpdate {
            enabled: false,
            ..terms
        },
    )
    .await;
    let kept = kit
        .services
        .settings_ops()
        .provider(TEST_PAY)
        .unwrap()
        .provider
        .topup_bonus[&Currency::Usd]
        .clone();
    assert!(!kept.enabled);
    assert_eq!(kept.percent, 10);
    assert_eq!(kept.min, Some(Money::new(500, Currency::Usd)));
    assert_eq!(kept.max, Some(Money::new(900, Currency::Usd)));
    let options = kit
        .services
        .checkout()
        .topup_options(client_id, Channel::Cabinet)
        .await
        .unwrap();
    assert!(options.methods.iter().all(|m| m.bonus.is_none()));
    let plain = topup(&kit, client_id, 1_000).await;
    pay(&kit, TEST_PAY, &plain).await;
    pay(&kit, TEST_PAY, &promised).await;
    assert_eq!(
        moves(&kit, client_id).await,
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::TopupBonus, 100),
        ]
    );
    kit.db.cleanup().await;
}

async fn save_in(
    kit: &Kit,
    provider: &str,
    currency: Currency,
    update: ProviderUpdate,
) -> ServiceResult<crate::settings::ProviderDetails> {
    let admin = admin_of(kit).await;
    kit.services
        .settings_ops()
        .update_provider(
            provider,
            &ProviderUpdate {
                currency: Some(currency),
                ..update
            },
            admin,
        )
        .await
}

#[tokio::test]
async fn бонус_задаётся_для_каждой_валюты_отдельно() {
    let Some(kit) = kassa_kit_with(
        Arc::new(Kassa::default()),
        Arc::new(SystemClock),
        vec![
            Arc::new(sn_payments::providers::Lava::new()),
            Arc::new(sn_payments::providers::Heleket::new()),
        ],
    )
    .await
    else {
        return;
    };
    let rub = TopupBonusUpdate {
        min_minor: Some(50_000),
        max_minor: Some(10_000),
        ..percent(5)
    };
    let usd = TopupBonusUpdate {
        min_minor: Some(1_000),
        methods: vec!["card".into()],
        ..percent(10)
    };
    let choice = |code: &str| sn_domain::payment::MethodChoice {
        code: code.into(),
        title: None,
    };
    save_in(
        &kit,
        providers::LAVA,
        Currency::Rub,
        ProviderUpdate {
            methods: Some(vec![choice("sbp"), choice("card")]),
            topup_bonus: Some(rub),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let saved = save_in(
        &kit,
        providers::LAVA,
        Currency::Usd,
        ProviderUpdate {
            methods: Some(vec![choice("paypal"), choice("card")]),
            topup_bonus: Some(usd),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .provider;
    let usd_view = &saved.topup_bonus[&Currency::Usd];
    assert_eq!(usd_view.min, Some(Money::new(1_000, Currency::Usd)));
    assert_eq!(usd_view.methods, ["card"]);
    let rub_view = &saved.topup_bonus[&Currency::Rub];
    assert_eq!(rub_view.min, Some(Money::new(50_000, Currency::Rub)));
    assert_eq!(rub_view.max, Some(Money::new(10_000, Currency::Rub)));
    assert_eq!(
        saved
            .methods
            .iter()
            .filter(|m| m.enabled)
            .map(|m| m.code)
            .collect::<Vec<_>>(),
        ["paypal", "card"]
    );
    let rub_form = kit
        .services
        .settings_ops()
        .provider_in(providers::LAVA, Some(Currency::Rub))
        .unwrap()
        .provider;
    assert_eq!(
        rub_form
            .methods
            .iter()
            .filter(|m| m.enabled)
            .map(|m| m.code)
            .collect::<Vec<_>>(),
        ["sbp", "card"]
    );
    let registry = kit.services.payments();
    assert_eq!(
        registry
            .topup_bonus(providers::LAVA, Some("card"), Currency::Usd)
            .map(|t| (t.percent, t.min_minor)),
        Some((10, Some(1_000)))
    );
    assert_eq!(
        registry.topup_bonus(providers::LAVA, Some("paypal"), Currency::Usd),
        None
    );
    assert_eq!(
        registry
            .topup_bonus(providers::LAVA, Some("sbp"), Currency::Rub)
            .map(|t| (t.percent, t.max_minor)),
        Some((5, Some(10_000)))
    );
    let refused = save_in(
        &kit,
        providers::HELEKET,
        Currency::Rub,
        ProviderUpdate {
            topup_bonus: Some(percent(5)),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code(), "settings.provider_currency");
    assert!(refused.to_string().contains("рубли"), "{refused}");
    save_in(
        &kit,
        providers::HELEKET,
        Currency::Rub,
        ProviderUpdate {
            channels: Some(sn_domain::payment::PaymentChannels::ALL),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let journal: Vec<serde_json::Value> = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT details -> 'topup_bonus_currency' FROM audit_log \
             WHERE action = 'settings.provider' AND details ->> 'provider' = 'lava' ORDER BY id",
        )
        .fetch_all(sn_store::Db::conn(&mut conn))
        .await
        .unwrap()
    };
    assert_eq!(
        journal,
        [serde_json::json!("RUB"), serde_json::json!("USD")]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn старый_бонус_одним_блоком_читается_как_раньше() {
    let Some(kit) = kit().await else { return };
    for (stored, rub, usd) in [
        (
            serde_json::json!({"enabled": true, "percent": 7}),
            Some(7),
            Some(7),
        ),
        (
            serde_json::json!({
                "enabled": true, "percent": 5,
                "min": {"minor": 50_000, "currency": "RUB"}
            }),
            Some(5),
            None,
        ),
    ] {
        {
            let mut conn = kit.services.store().conn().await.unwrap();
            conn.settings()
                .set(
                    &keys::provider(TEST_PAY),
                    &serde_json::json!({"enabled": true, "topup_bonus": stored}),
                    None,
                )
                .await
                .unwrap();
        }
        kit.services.reload().await.unwrap();
        let registry = kit.services.payments();
        assert_eq!(
            registry
                .topup_bonus(TEST_PAY, None, Currency::Rub)
                .map(|t| t.percent),
            rub
        );
        assert_eq!(
            registry
                .topup_bonus(TEST_PAY, None, Currency::Usd)
                .map(|t| t.percent),
            usd
        );
    }
    let kept = save_in(
        &kit,
        TEST_PAY,
        Currency::Usd,
        ProviderUpdate {
            topup_bonus: Some(percent(3)),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .provider
    .topup_bonus;
    assert_eq!(kept[&Currency::Usd].percent, 3);
    assert_eq!(
        kept[&Currency::Rub].min,
        Some(Money::new(50_000, Currency::Rub))
    );
    let raw: serde_json::Value = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .get(&keys::provider(TEST_PAY))
            .await
            .unwrap()
            .unwrap()
    };
    assert_eq!(raw["topup_bonus"]["RUB"]["min_minor"], 50_000);
    assert_eq!(raw["topup_bonus"]["USD"]["percent"], 3);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn настройка_бонуса_проверяется_и_попадает_в_журнал_и_в_варианты_пополнения() {
    let Some(kit) = kit().await else { return };
    let admin = admin_of(&kit).await;
    let client_id = client(&kit.services, 8118).await;
    let save = |provider: &'static str, terms: TopupBonusUpdate| {
        let services = kit.services.clone();
        async move {
            services
                .settings_ops()
                .update_provider(
                    provider,
                    &ProviderUpdate {
                        topup_bonus: Some(terms),
                        ..Default::default()
                    },
                    admin,
                )
                .await
        }
    };
    let on = |value: i64| TopupBonusUpdate {
        enabled: true,
        ..percent(value)
    };
    for (wrong, code) in [
        (on(-1), "settings.topup_bonus_percent"),
        (on(0), "settings.topup_bonus_percent"),
        (on(101), "settings.topup_bonus_percent"),
        (
            TopupBonusUpdate {
                min_minor: Some(0),
                ..on(5)
            },
            "settings.topup_bonus_min",
        ),
        (
            TopupBonusUpdate {
                max_minor: Some(-5),
                ..on(5)
            },
            "settings.topup_bonus_max",
        ),
        (
            TopupBonusUpdate {
                max_minor: Some(100_000_001),
                ..on(5)
            },
            "settings.topup_bonus_max",
        ),
        (
            TopupBonusUpdate {
                methods: vec!["bitcoin".into()],
                ..on(5)
            },
            "settings.topup_bonus_methods",
        ),
    ] {
        assert_eq!(save(TEST_PAY, wrong).await.unwrap_err().code(), code);
    }
    assert_eq!(
        save(providers::BALANCE, on(5)).await.unwrap_err().code(),
        "settings.topup_bonus_kind"
    );
    save(providers::BALANCE, percent(0)).await.unwrap();
    let saved = save(
        TEST_PAY,
        TopupBonusUpdate {
            min_minor: Some(1_000),
            methods: vec!["card".into(), "card".into()],
            ..on(100)
        },
    )
    .await
    .unwrap()
    .provider
    .topup_bonus[&Currency::Usd]
        .clone();
    assert!(saved.enabled);
    assert_eq!(saved.methods, ["card"]);
    let all = save(
        TEST_PAY,
        TopupBonusUpdate {
            methods: vec!["card".into(), "sbp".into(), "loan".into()],
            max_minor: Some(250),
            ..on(5)
        },
    )
    .await
    .unwrap()
    .provider
    .topup_bonus[&Currency::Usd]
        .clone();
    assert!(all.methods.is_empty());
    let untouched = kit
        .services
        .settings_ops()
        .update_provider(
            TEST_PAY,
            &ProviderUpdate {
                enabled: Some(true),
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(untouched.provider.topup_bonus[&Currency::Usd], all);
    let recorded: Vec<serde_json::Value> = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT details FROM audit_log WHERE action = 'settings.provider' \
             AND details ->> 'provider' = $1 ORDER BY id",
        )
        .bind(TEST_PAY)
        .fetch_all(sn_store::Db::conn(&mut conn))
        .await
        .unwrap()
    };
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[0]["topup_bonus_enabled"], true);
    assert_eq!(recorded[0]["topup_bonus_percent"], 100);
    assert_eq!(recorded[0]["topup_bonus_min"]["minor"], 1_000);
    assert_eq!(recorded[0]["topup_bonus_max"], "без ограничения");
    assert_eq!(recorded[0]["topup_bonus_methods"][0], "Карта");
    assert_eq!(recorded[1]["topup_bonus_min"], "без ограничения");
    assert_eq!(recorded[1]["topup_bonus_max"]["minor"], 250);
    assert_eq!(recorded[1]["topup_bonus_methods"], "все");
    assert!(recorded[2].get("topup_bonus_enabled").is_none());
    kit.services.set_remna(Some(kit.remna.clone()));
    let options = kit
        .services
        .checkout()
        .topup_options(client_id, Channel::Cabinet)
        .await
        .unwrap();
    for method in &options.methods {
        let expected = (method.provider == TEST_PAY).then_some(5);
        assert_eq!(method.bonus.map(|b| b.percent), expected);
    }
    let bonus = options
        .methods
        .iter()
        .find(|m| m.provider == TEST_PAY)
        .and_then(|m| m.bonus)
        .unwrap();
    assert_eq!(bonus.min, None);
    assert_eq!(bonus.max, Some(Money::new(250, Currency::Usd)));
    kit.db.cleanup().await;
}
