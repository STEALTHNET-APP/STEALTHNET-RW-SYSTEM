use super::*;
use sn_domain::ledger::LedgerEntry;

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

fn moves(entries: &[LedgerEntry]) -> Vec<(LedgerKind, i64)> {
    entries.iter().map(|e| (e.kind, e.amount.minor)).collect()
}

async fn paid_topup(kit: &Kit, client_id: ClientId, minor: i64) -> Payment {
    let topped = kassa_topup(kit, client_id, minor).await;
    kit.services
        .inbox()
        .apply_outcome(KASSA, &outcome(&topped, ProviderStatus::Paid))
        .await
        .unwrap();
    topped
}

fn confirmed(payment: &Payment, refund: &str) -> WebhookOutcome {
    WebhookOutcome {
        event_key: format!("refund:{refund}"),
        ..outcome(payment, ProviderStatus::Refunded)
    }
}

fn balance_purchase(tariff: &Tariff, days: u32) -> CheckoutRequest {
    CheckoutRequest {
        cart: cart(tariff, days, None),
        provider: providers::BALANCE.into(),
        method: None,
        channel: Channel::Cabinet,
        return_url: None,
    }
}

#[tokio::test]
async fn деньги_ожидающего_возврата_пополнения_нельзя_потратить() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7500).await;
    let tariff = standard(&kit.services).await;
    let topped = paid_topup(&kit, client_id, 1_000).await;
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    let inbox = kit.services.inbox();
    let requested = inbox
        .refund(
            topped.id,
            &refund_command(
                None,
                RefundSubscriptionAction::Keep,
                Some("клиент просит вернуть пополнение"),
            ),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    assert_eq!(balance(&kit.services, client_id).await, 0);
    let refused = kit
        .services
        .checkout()
        .start(client_id, &balance_purchase(&tariff, 30))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "checkout.method_unavailable");
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        inbox
            .apply_outcome(KASSA, &confirmed(&topped, "kr-1"))
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: topped.id,
            status: PaymentStatus::Refunded
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert!(
        !audit_actions(&kit, topped.id)
            .await
            .contains(&"refund.topup_shortfall".to_string())
    );
    let entries = ledger(&kit, client_id).await;
    assert_eq!(
        moves(&entries),
        vec![(LedgerKind::TopUp, 1_000), (LedgerKind::RefundHold, -1_000)]
    );
    assert_eq!(entries[1].payment_id, Some(topped.id));
    let requests = provider_refunds(&kit, topped.id).await;
    assert_eq!(requests[0].hold_entry_id, Some(entries[1].id));
    assert_eq!(requests[0].status, ProviderRefundStatus::Succeeded);
    let orders_paid = jobs(&kit.services)
        .await
        .into_iter()
        .filter(|j| matches!(j, JobPayload::FulfillOrder { .. }))
        .count();
    assert_eq!(orders_paid, 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказ_кассы_возвращает_удержание_записью_сторно() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7501).await;
    let tariff = standard(&kit.services).await;
    let topped = paid_topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    let requested = inbox
        .refund(
            topped.id,
            &refund_command(Some(600), RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    assert_eq!(balance(&kit.services, client_id).await, 400);
    let declined = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        amount: Some(Amount::new(600, PayCurrency::Usd)),
        ..outcome(&topped, ProviderStatus::RefundDeclined)
    };
    assert_eq!(
        inbox.apply_outcome(KASSA, &declined).await.unwrap(),
        AcceptResult::StatusChanged {
            payment_id: topped.id,
            status: PaymentStatus::Paid
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    assert!(matches!(
        inbox.apply_outcome(KASSA, &declined).await.unwrap(),
        AcceptResult::Ignored { .. }
    ));
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    assert_eq!(
        moves(&ledger(&kit, client_id).await),
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::RefundHold, -600),
            (LedgerKind::RefundRelease, 600)
        ]
    );
    let changes: Vec<(LedgerKind, i64)> = events(&kit.services, client_id)
        .await
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::BalanceChanged { kind, delta, .. } => Some((kind, delta.minor)),
            _ => None,
        })
        .collect();
    assert_eq!(
        changes,
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::RefundHold, -600),
            (LedgerKind::RefundRelease, 600)
        ]
    );
    buy_with_balance(&kit.services, client_id, cart(&tariff, 90, None)).await;
    assert_eq!(balance(&kit.services, client_id).await, 301);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn запрос_возврата_учитывает_удержания_по_всем_пополнениям_клиента() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7502).await;
    let tariff = standard(&kit.services).await;
    let first = paid_topup(&kit, client_id, 1_000).await;
    let second = paid_topup(&kit, client_id, 400).await;
    buy_with_balance(&kit.services, client_id, cart(&tariff, 90, None)).await;
    assert_eq!(balance(&kit.services, client_id).await, 701);
    let inbox = kit.services.inbox();
    let keep = |minor: Option<i64>| refund_command(minor, RefundSubscriptionAction::Keep, None);
    assert_eq!(
        inbox
            .refund(first.id, &keep(None), admin)
            .await
            .unwrap_err()
            .code(),
        "refund.topup_spent"
    );
    assert!(kassa.refunds.lock().unwrap().is_empty());
    assert!(
        inbox
            .refund(second.id, &keep(None), admin)
            .await
            .unwrap()
            .pending
    );
    assert_eq!(balance(&kit.services, client_id).await, 301);
    assert_eq!(
        inbox
            .refund(first.id, &keep(Some(500)), admin)
            .await
            .unwrap_err()
            .code(),
        "refund.topup_spent"
    );
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    assert!(
        inbox
            .refund(first.id, &keep(Some(300)), admin)
            .await
            .unwrap()
            .pending
    );
    assert_eq!(balance(&kit.services, client_id).await, 1);
    for (payment, refund) in [(&second, "kr-1"), (&first, "kr-2")] {
        inbox
            .apply_outcome(
                KASSA,
                &WebhookOutcome {
                    event_key: format!("refund:{refund}"),
                    amount: None,
                    ..outcome(
                        payment,
                        ProviderStatus::PartiallyRefunded {
                            refunded: Amount::new(
                                if payment.id == first.id { 300 } else { 400 },
                                PayCurrency::Usd,
                            ),
                        },
                    )
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(balance(&kit.services, client_id).await, 1);
    assert_eq!(
        payment(&kit, second.id).await.status,
        PaymentStatus::Refunded
    );
    assert_eq!(
        payment(&kit, first.id).await.status,
        PaymentStatus::PartiallyRefunded
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn сбой_кассы_при_запросе_возврата_не_оставляет_удержания() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7503).await;
    let topped = paid_topup(&kit, client_id, 1_000).await;
    *kassa.refund_fails.lock().unwrap() = true;
    let failed = kit
        .services
        .inbox()
        .refund(
            topped.id,
            &refund_command(None, RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await;
    assert!(
        matches!(
            failed,
            Err(ServiceError::Payment(PaymentError::Transport(_)))
        ),
        "{failed:?}"
    );
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    assert_eq!(
        moves(&ledger(&kit, client_id).await),
        vec![(LedgerKind::TopUp, 1_000)]
    );
    assert!(provider_refunds(&kit, topped.id).await.is_empty());
    assert_eq!(payment(&kit, topped.id).await.status, PaymentStatus::Paid);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_пополнения_с_немедленным_ответом_кассы_списывает_баланс_один_раз() {
    let kassa = Arc::new(Kassa::default());
    *kassa.refund_completed.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7504).await;
    let topped = paid_topup(&kit, client_id, 1_000).await;
    let refunded = kit
        .services
        .inbox()
        .refund(
            topped.id,
            &refund_command(Some(700), RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert!(!refunded.pending);
    assert_eq!(refunded.status, PaymentStatus::PartiallyRefunded);
    assert_eq!(balance(&kit.services, client_id).await, 300);
    assert_eq!(
        moves(&ledger(&kit, client_id).await),
        vec![(LedgerKind::TopUp, 1_000), (LedgerKind::RefundHold, -700)]
    );
    assert!(provider_refunds(&kit, topped.id).await.is_empty());
    assert!(
        !audit_actions(&kit, topped.id)
            .await
            .contains(&"refund.topup_shortfall".to_string())
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ожидающий_возврат_без_удержания_учитывается_и_списывается_при_подтверждении() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7505).await;
    let tariff = standard(&kit.services).await;
    let old = paid_topup(&kit, client_id, 600).await;
    let fresh = paid_topup(&kit, client_id, 500).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments()
            .create_provider_refund(&NewProviderRefund {
                bonus_minor: 0,
                payment_id: old.id,
                amount: Amount::new(600, PayCurrency::Usd),
                subscription_action: RefundSubscriptionAction::Keep,
                reason: None,
                admin_id: Some(admin),
                provider_refund_id: Some("kr-old".into()),
                hold_entry_id: None,
                requested_at: kit.services.now(),
            })
            .await
            .unwrap();
    }
    assert_eq!(balance(&kit.services, client_id).await, 1_100);
    let inbox = kit.services.inbox();
    let keep = || refund_command(None, RefundSubscriptionAction::Keep, None);
    assert!(
        inbox
            .refund(fresh.id, &keep(), admin)
            .await
            .unwrap()
            .pending
    );
    assert_eq!(balance(&kit.services, client_id).await, 600);
    let extra = paid_topup(&kit, client_id, 300).await;
    buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    assert_eq!(balance(&kit.services, client_id).await, 601);
    assert_eq!(
        inbox
            .refund(extra.id, &keep(), admin)
            .await
            .unwrap_err()
            .code(),
        "refund.topup_spent"
    );
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    inbox
        .apply_outcome(KASSA, &confirmed(&old, "kr-old"))
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1);
    assert_eq!(payment(&kit, old.id).await.status, PaymentStatus::Refunded);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn админ_отменяет_зависшую_заявку_и_удержание_возвращается() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7506).await;
    let topped = paid_topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    let requested = inbox
        .refund(
            topped.id,
            &refund_command(Some(600), RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    let request_id = requested.request_id.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 400);
    assert_eq!(
        inbox
            .cancel_refund_request(request_id, admin, " ")
            .await
            .unwrap_err()
            .code(),
        "refund.request_note"
    );
    inbox
        .cancel_refund_request(request_id, admin, "Касса не провела возврат")
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    let requests = provider_refunds(&kit, topped.id).await;
    assert_eq!(requests[0].status, ProviderRefundStatus::Declined);
    assert_eq!(
        requests[0].failure_reason.as_deref(),
        Some("отменена администратором: Касса не провела возврат")
    );
    assert!(
        audit_actions(&kit, topped.id)
            .await
            .contains(&"payment.refund_canceled".to_string())
    );
    assert_eq!(
        inbox
            .cancel_refund_request(request_id, admin, "ещё раз")
            .await
            .unwrap_err()
            .code(),
        "refund.request_resolved"
    );
    assert_eq!(payment(&kit, topped.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        moves(&ledger(&kit, client_id).await),
        vec![
            (LedgerKind::TopUp, 1_000),
            (LedgerKind::RefundHold, -600),
            (LedgerKind::RefundRelease, 600)
        ]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn админ_подтверждает_зависшую_заявку_без_второго_списания() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7507).await;
    let topped = paid_topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    let requested = inbox
        .refund(
            topped.id,
            &refund_command(None, RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    let request_id = requested.request_id.unwrap();
    assert_eq!(
        inbox
            .confirm_refund_request(request_id, admin, "Возврат виден в кабинете кассы")
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: topped.id,
            status: PaymentStatus::Refunded
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        moves(&ledger(&kit, client_id).await),
        vec![(LedgerKind::TopUp, 1_000), (LedgerKind::RefundHold, -1_000)]
    );
    let actions = audit_actions(&kit, topped.id).await;
    assert!(actions.contains(&"payment.refund_confirmed_manually".to_string()));
    assert!(actions.contains(&"payment.refund_confirmed".to_string()));
    assert_eq!(
        inbox
            .apply_outcome(KASSA, &confirmed(&topped, "kr-1"))
            .await
            .unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: topped.id
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        inbox
            .confirm_refund_request(request_id, admin, "ещё раз")
            .await
            .unwrap_err()
            .code(),
        "refund.request_resolved"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn клиент_видит_сумму_удержанную_до_ответа_платёжной_системы() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7510).await;
    let first = paid_topup(&kit, client_id, 1_000).await;
    let second = paid_topup(&kit, client_id, 500).await;
    let held = |card: &crate::clients::ClientCard| (card.balance.minor, card.balance_held.minor);
    let clients = kit.services.clients();
    assert_eq!(held(&clients.card(client_id).await.unwrap()), (1_500, 0));
    let inbox = kit.services.inbox();
    let keep = |minor: Option<i64>| refund_command(minor, RefundSubscriptionAction::Keep, None);
    assert!(
        inbox
            .refund(first.id, &keep(Some(600)), admin)
            .await
            .unwrap()
            .pending
    );
    assert!(
        inbox
            .refund(second.id, &keep(None), admin)
            .await
            .unwrap()
            .pending
    );
    let card = clients.card(client_id).await.unwrap();
    assert_eq!(held(&card), (400, 1_100));
    assert_eq!(card.balance_held.currency, card.balance.currency);
    let declined = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        amount: Some(Amount::new(600, PayCurrency::Usd)),
        ..outcome(&first, ProviderStatus::RefundDeclined)
    };
    inbox.apply_outcome(KASSA, &declined).await.unwrap();
    assert_eq!(held(&clients.card(client_id).await.unwrap()), (1_000, 500));
    inbox
        .apply_outcome(KASSA, &confirmed(&second, "kr-2"))
        .await
        .unwrap();
    assert_eq!(
        held(&clients.card(client_id).await.unwrap()),
        (1_000, 0),
        "после ответа платёжной системы удержания нет"
    );
    kit.db.cleanup().await;
}
