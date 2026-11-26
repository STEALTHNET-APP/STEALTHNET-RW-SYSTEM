use super::*;
use crate::checkout::testkit::{Kit, kit, kit_with};
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::{PaymentId, PeriodId, TariffId};
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::payment::{NewPayment, PaymentStatus, ProviderId};
use sn_domain::pricing::{Quote, QuoteLine, QuoteLineKind};
use sn_modules::catalog::referral::{self, ReferralModule};
use sn_store::AuditRecord;
use std::sync::Arc;

async fn owner(kit: &Kit) -> Actor {
    Actor::Admin(
        kit.services
            .auth()
            .upsert_owner("owner@example.com", "очень-надёжный-пароль")
            .await
            .unwrap()
            .0,
    )
}

async fn person(kit: &Kit, telegram_id: i64, name: &str, start: Option<&str>) -> ClientId {
    let profile = TelegramProfile {
        id: telegram_id,
        username: Some(format!("u{telegram_id}")),
        first_name: Some(name.into()),
        last_name: None,
        language_code: Some("ru".into()),
        is_premium: false,
    };
    kit.services
        .clients()
        .ensure_telegram(&profile, RegistrationSource::Bot, start)
        .await
        .unwrap()
        .0
        .id
}

async fn referrer_of(kit: &Kit, client: ClientId) -> Option<i64> {
    sqlx::query_scalar("SELECT referrer_id FROM referral_links WHERE client_id = $1")
        .bind(client.0)
        .fetch_optional(kit.services.store().pool())
        .await
        .unwrap()
}

async fn audits(kit: &Kit, client: ClientId) -> Vec<AuditRecord> {
    let mut conn = kit.services.store().conn().await.unwrap();
    let mut found: Vec<AuditRecord> = conn
        .audit()
        .for_entity("client", client.0, 50)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action.starts_with("client.referrer_"))
        .collect();
    found.sort_by_key(|r| r.id);
    found
}

fn quote(total: Money) -> Quote {
    Quote {
        currency: total.currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::Period {
                tariff_id: TariffId(1),
                period_id: PeriodId(1),
                days: 30,
                months: 1,
            },
            label: "Стандарт · 30 дней".into(),
            amount: total,
            stars: None,
        }],
        subtotal: total,
        discount: Money::zero(total.currency),
        total,
        stars: None,
        days: 30,
        extra_devices: 0,
    }
}

async fn pay(kit: &Kit, payer: ClientId, minor: i64) -> PaymentId {
    let currency = kit.services.settings().currency;
    let total = Money::new(minor, currency);
    let amount = Amount::new(minor, PayCurrency::from(currency));
    let payment = {
        let mut conn = kit.services.store().conn().await.unwrap();
        let order = conn
            .orders()
            .create(&NewOrder {
                client_id: payer,
                subscription_id: None,
                kind: OrderKind::New,
                intent: PurchaseIntent::Subscription {
                    tariff_id: TariffId(1),
                    period_id: PeriodId(1),
                    extra_devices: 0,
                    target: None,
                },
                quote: quote(total),
                snapshot: None,
            })
            .await
            .unwrap();
        let payment = conn
            .payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id: payer,
                provider: ProviderId::from("testpay"),
                method: None,
                amount,
                expires_at: None,
            })
            .await
            .unwrap();
        conn.payments()
            .transition(
                payment.id,
                PaymentStatus::Paid,
                Some(&format!("tx-{}", payment.id)),
                None,
            )
            .await
            .unwrap()
            .payment()
            .clone()
    };
    referral::accrue(
        &kit.services.module_context(),
        payment.id,
        payer,
        amount,
        &ProviderId::from("testpay"),
    )
    .await
    .unwrap();
    payment.id
}

async fn credits(kit: &Kit, referrer: ClientId) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM referral_credits WHERE referrer_id = $1",
    )
    .bind(referrer.0)
    .fetch_one(kit.services.store().pool())
    .await
    .unwrap()
}

fn code(err: ServiceError) -> &'static str {
    match err {
        ServiceError::Rejected { code, .. } => code,
        other => panic!("ожидался отказ, пришло {other:?}"),
    }
}

#[tokio::test]
async fn назначить_сменить_и_убрать_пригласившего_пишет_журнал() {
    let Some(kit) = kit().await else { return };
    let actor = owner(&kit).await;
    let ops = kit.services.referral_links();
    let alice = person(&kit, 7_100_001, "Алиса", None).await;
    let boris = person(&kit, 7_100_002, "Борис", None).await;
    let client = person(&kit, 7_100_003, "Вера", None).await;

    let set = ops.set_referrer(client, alice, actor).await.unwrap();
    assert!(set.changed);
    assert_eq!(referrer_of(&kit, client).await, Some(alice.0));
    let again = ops.set_referrer(client, alice, actor).await.unwrap();
    assert!(!again.changed);
    assert_eq!(audits(&kit, client).await.len(), 1);

    ops.set_referrer(client, boris, actor).await.unwrap();
    assert_eq!(referrer_of(&kit, client).await, Some(boris.0));
    ops.remove_referrer(client, None, actor).await.unwrap();
    assert_eq!(referrer_of(&kit, client).await, None);
    let gone = ops.remove_referrer(client, None, actor).await.unwrap();
    assert!(!gone.changed);

    let records = audits(&kit, client).await;
    let actions: Vec<&str> = records.iter().map(|r| r.action.as_str()).collect();
    assert_eq!(
        actions,
        [
            "client.referrer_set",
            "client.referrer_set",
            "client.referrer_removed"
        ]
    );
    assert!(records.iter().all(|r| r.actor_kind == "admin"));
    let first = &records[0].details;
    assert!(first["from"]["referrer"].is_null(), "{first}");
    assert_eq!(
        first["to"]["referrer"],
        format!("Алиса · @u7100001 · №{}", alice.0)
    );
    assert_eq!(first["referrer_id"], alice.0);
    assert_eq!(
        first["invitee"],
        format!("Вера · @u7100003 · №{}", client.0)
    );
    let changed = &records[1].details;
    assert_eq!(
        changed["from"]["referrer"],
        format!("Алиса · @u7100001 · №{}", alice.0)
    );
    assert_eq!(
        changed["to"]["referrer"],
        format!("Борис · @u7100002 · №{}", boris.0)
    );
    assert_eq!(changed["previous_referrer_id"], alice.0);
    assert_eq!(
        crate::audit_journal::title_for(&records[1].action, changed),
        "Сменён пригласивший"
    );
    assert_eq!(
        crate::audit_journal::title_for(&records[0].action, first),
        "Назначен пригласивший"
    );
    let removed = &records[2].details;
    assert_eq!(
        removed["from"]["referrer"],
        format!("Борис · @u7100002 · №{}", boris.0)
    );
    assert!(removed["to"]["referrer"].is_null());
    assert_eq!(removed["previous_referrer_id"], boris.0);
}

#[tokio::test]
async fn нельзя_назначить_самого_клиента_круг_и_удалённого() {
    let Some(kit) = kit().await else { return };
    let actor = owner(&kit).await;
    let ops = kit.services.referral_links();
    let ids = [7_200_001, 7_200_002, 7_200_003, 7_200_004, 7_200_005];
    let mut chain = Vec::new();
    for (n, id) in ids.into_iter().enumerate() {
        chain.push(person(&kit, id, &format!("Звено {n}"), None).await);
    }
    for pair in chain.windows(2) {
        ops.set_referrer(pair[1], pair[0], actor).await.unwrap();
    }
    let top = chain[0];
    let bottom = chain[4];

    let itself = ops.set_referrer(top, top, actor).await.unwrap_err();
    assert_eq!(code(itself), SELF);
    let direct = ops.set_referrer(top, chain[1], actor).await.unwrap_err();
    assert_eq!(code(direct), CYCLE);
    let deep = ops.set_referrer(top, bottom, actor).await.unwrap_err();
    assert_eq!(code(deep), CYCLE);
    assert_eq!(referrer_of(&kit, top).await, None);
    ops.set_referrer(bottom, top, actor).await.unwrap();
    assert_eq!(referrer_of(&kit, bottom).await, Some(top.0));

    let ghost = person(&kit, 7_200_009, "Удалённый", None).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.clients().soft_delete(ghost).await.unwrap();
    }
    let deleted = ops.set_referrer(chain[2], ghost, actor).await.unwrap_err();
    assert_eq!(code(deleted), UNKNOWN);
    let missing = ops
        .set_referrer(chain[2], ClientId(9_999_999), actor)
        .await
        .unwrap_err();
    assert_eq!(code(missing), UNKNOWN);
    assert_eq!(referrer_of(&kit, chain[2]).await, Some(chain[1].0));
}

#[tokio::test]
async fn параллельные_правки_не_замыкают_круг() {
    let Some(kit) = kit().await else { return };
    let actor = owner(&kit).await;
    for round in 0..5_i64 {
        let a = person(&kit, 7_300_000 + round * 10, "А", None).await;
        let b = person(&kit, 7_300_001 + round * 10, "Б", None).await;
        let ops = kit.services.referral_links();
        let (first, second) =
            tokio::join!(ops.set_referrer(a, b, actor), ops.set_referrer(b, a, actor));
        let done = [first.is_ok(), second.is_ok()];
        assert_eq!(done.iter().filter(|ok| **ok).count(), 1, "{done:?}");
        let refused = first.err().or(second.err()).unwrap();
        assert_eq!(code(refused), CYCLE);
    }
}

#[tokio::test]
async fn отвязать_можно_только_своего_приглашённого() {
    let Some(kit) = kit().await else { return };
    let actor = owner(&kit).await;
    let ops = kit.services.referral_links();
    let alice = person(&kit, 7_400_001, "Алиса", None).await;
    let boris = person(&kit, 7_400_002, "Борис", None).await;
    let client = person(&kit, 7_400_003, "Вера", None).await;
    ops.set_referrer(client, boris, actor).await.unwrap();

    let foreign = ops
        .remove_referrer(client, Some(alice), actor)
        .await
        .unwrap_err();
    assert_eq!(code(foreign), NOT_INVITED);
    assert_eq!(referrer_of(&kit, client).await, Some(boris.0));

    let done = ops
        .remove_referrer(client, Some(boris), actor)
        .await
        .unwrap();
    assert!(done.changed);
    assert_eq!(referrer_of(&kit, client).await, None);
    let again = ops
        .remove_referrer(client, Some(boris), actor)
        .await
        .unwrap();
    assert!(!again.changed);
    let records = audits(&kit, client).await;
    assert_eq!(records.last().unwrap().action, "client.referrer_removed");
    assert_eq!(
        records.last().unwrap().details["previous_referrer_id"],
        boris.0
    );
}

#[tokio::test]
async fn ручная_привязка_датируется_регистрацией_и_помнит_кто_привязал() {
    let Some(kit) = kit().await else { return };
    let actor = owner(&kit).await;
    let alice = person(&kit, 7_500_001, "Алиса", None).await;
    let client = person(&kit, 7_500_002, "Вера", None).await;
    sqlx::query("UPDATE clients SET created_at = now() - interval '40 days' WHERE id = $1")
        .bind(client.0)
        .execute(kit.services.store().pool())
        .await
        .unwrap();
    kit.services
        .referral_links()
        .set_referrer(client, alice, actor)
        .await
        .unwrap();
    let (linked_same, set_recent, set_by): (bool, bool, Option<i64>) = sqlx::query_as(
        "SELECT l.created_at = c.created_at, l.set_at > now() - interval '1 minute', l.set_by \
         FROM referral_links l JOIN clients c ON c.id = l.client_id WHERE l.client_id = $1",
    )
    .bind(client.0)
    .fetch_one(kit.services.store().pool())
    .await
    .unwrap();
    assert!(linked_same);
    assert!(set_recent);
    let Actor::Admin(admin) = actor else {
        unreachable!()
    };
    assert_eq!(set_by, Some(admin.0));

    let section = kit
        .services
        .referral_links()
        .section(client, true, None)
        .await
        .unwrap();
    let inviter = section.inviter.unwrap();
    assert_eq!(inviter.client.client_id, alice);
    let manual = inviter.manual.unwrap();
    assert_eq!(manual.by.as_deref(), Some("owner@example.com"));
    assert!(inviter.source.is_none() && inviter.code.is_none());
}

#[tokio::test]
async fn раздел_показывает_пригласившего_приглашённых_итоги_и_прячет_деньги() {
    let Some(kit) = kit_with(vec![Arc::new(ReferralModule)]).await else {
        return;
    };
    let actor = owner(&kit).await;
    let alice = person(&kit, 7_600_001, "Алиса", None).await;
    let invite = {
        let mut conn = kit.services.store().conn().await.unwrap();
        referral::ensure_code(&mut conn, alice).await.unwrap()
    };
    let boris = person(&kit, 7_600_002, "Борис", None).await;
    assert!(
        referral::link_by_start_param(
            &kit.services.module_context(),
            boris,
            &format!("ref_{invite}")
        )
        .await
        .unwrap()
    );
    let start = format!("s_tiktok__ref_{}", invite.to_lowercase());
    let vera = person(&kit, 7_600_003, "Вера", Some(&start)).await;
    assert!(
        referral::link_by_start_param(&kit.services.module_context(), vera, &start)
            .await
            .unwrap()
    );
    let gleb = person(&kit, 7_600_004, "Глеб", None).await;
    kit.services
        .referral_links()
        .set_referrer(gleb, alice, actor)
        .await
        .unwrap();

    let ops = kit.services.referral_links();
    let mine = ops.section(vera, true, None).await.unwrap();
    let inviter = mine.inviter.unwrap();
    assert_eq!(inviter.client.client_id, alice);
    assert_eq!(inviter.source.as_deref(), Some("bot"));
    assert_eq!(inviter.code.as_deref(), Some(invite.as_str()));
    assert!(inviter.manual.is_none());
    assert!(!inviter.blocked && !inviter.deleted);

    pay(&kit, vera, 10_000).await;
    pay(&kit, vera, 5_000).await;
    let section = ops.section(alice, true, Some(2)).await.unwrap();
    assert!(section.inviter.is_none());
    assert_eq!(section.invited_count, 3);
    let money = section.money.unwrap();
    assert_eq!(money.invited_paid, 1);
    assert_eq!(money.earned.minor, 4_500);
    assert_eq!(money.balance.minor, 4_500);
    assert_eq!(money.withdrawn.minor, 0);
    assert_eq!(section.invited.items.len(), 2);
    assert_eq!(section.invited.items[0].client.client_id, gleb);
    assert!(section.invited.items[0].manual_at.is_some());
    assert_eq!(section.invited.items[1].client.client_id, vera);
    assert_eq!(section.invited.items[1].paid, Some(true));
    assert_eq!(section.invited.items[1].bonus.unwrap().minor, 4_500);
    let next = section.invited.next.unwrap();
    let rest = ops
        .invited(alice, PageRequest::new(Some(2), Some(next)), true)
        .await
        .unwrap();
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.items[0].client.client_id, boris);
    assert_eq!(rest.items[0].paid, Some(false));
    assert!(rest.next.is_none());

    let hidden = ops.section(alice, false, None).await.unwrap();
    assert!(hidden.money.is_none());
    assert!(
        hidden
            .invited
            .items
            .iter()
            .all(|i| i.paid.is_none() && i.bonus.is_none())
    );
    let json = serde_json::to_value(&hidden).unwrap();
    assert!(json.get("money").is_none(), "{json}");
    assert!(json["invited"]["items"][0].get("bonus").is_none(), "{json}");
}

#[tokio::test]
async fn после_смены_старые_бонусы_остаются_а_новые_идут_новому() {
    let Some(kit) = kit_with(vec![Arc::new(ReferralModule)]).await else {
        return;
    };
    let actor = owner(&kit).await;
    let alice = person(&kit, 7_700_001, "Алиса", None).await;
    let boris = person(&kit, 7_700_002, "Борис", None).await;
    let client = person(&kit, 7_700_003, "Вера", None).await;
    let ops = kit.services.referral_links();
    ops.set_referrer(client, alice, actor).await.unwrap();
    pay(&kit, client, 10_000).await;
    assert_eq!(credits(&kit, alice).await, 3_000);

    ops.set_referrer(client, boris, actor).await.unwrap();
    pay(&kit, client, 20_000).await;
    assert_eq!(credits(&kit, alice).await, 3_000);
    assert_eq!(credits(&kit, boris).await, 6_000);
    let alice_view = ops.section(alice, true, None).await.unwrap();
    assert_eq!(alice_view.invited_count, 0);
    assert_eq!(alice_view.money.unwrap().earned.minor, 3_000);
    let boris_view = ops.section(boris, true, None).await.unwrap();
    assert_eq!(boris_view.invited.items[0].bonus.unwrap().minor, 6_000);

    ops.remove_referrer(client, None, actor).await.unwrap();
    pay(&kit, client, 10_000).await;
    assert_eq!(credits(&kit, alice).await, 3_000);
    assert_eq!(credits(&kit, boris).await, 6_000);
}

#[tokio::test]
async fn история_показывает_правку_всем_трём_клиентам() {
    let Some(kit) = kit().await else { return };
    let actor = owner(&kit).await;
    let ops = kit.services.referral_links();
    let alice = person(&kit, 7_800_001, "Алиса", None).await;
    let boris = person(&kit, 7_800_002, "Борис", None).await;
    let client = person(&kit, 7_800_003, "Вера", None).await;
    let outsider = person(&kit, 7_800_004, "Гость", None).await;
    ops.set_referrer(client, alice, actor).await.unwrap();
    ops.set_referrer(client, boris, actor).await.unwrap();

    assert_eq!(referral_lines(&kit, client).await, 2);
    assert_eq!(referral_lines(&kit, alice).await, 2);
    assert_eq!(referral_lines(&kit, boris).await, 1);
    assert_eq!(referral_lines(&kit, outsider).await, 0);
}

async fn referral_lines(kit: &Kit, id: ClientId) -> usize {
    let page = kit
        .services
        .clients()
        .history(id, PageRequest::new(Some(50), None), true)
        .await
        .unwrap();
    page.items
        .iter()
        .filter(|item| {
            matches!(
                &item.record,
                crate::clients::HistoryRecord::Audit { action, .. } if action.starts_with("client.referrer_")
            )
        })
        .count()
}

#[tokio::test]
async fn кандидаты_отмечают_самого_клиента_круг_и_текущего() {
    let Some(kit) = kit().await else { return };
    let actor = owner(&kit).await;
    let ops = kit.services.referral_links();
    let top = person(&kit, 7_900_001, "Кандидат Верх", None).await;
    let client = person(&kit, 7_900_002, "Кандидат Клиент", None).await;
    let child = person(&kit, 7_900_003, "Кандидат Друг", None).await;
    let grandchild = person(&kit, 7_900_004, "Кандидат Внук", None).await;
    let free = person(&kit, 7_900_005, "Кандидат Свободный", None).await;
    ops.set_referrer(client, top, actor).await.unwrap();
    ops.set_referrer(child, client, actor).await.unwrap();
    ops.set_referrer(grandchild, child, actor).await.unwrap();

    let found = ops.candidates(client, "Кандидат").await.unwrap();
    let refusal = |id: ClientId| {
        found
            .iter()
            .find(|c| c.client.client_id == id)
            .map(|c| c.refusal)
    };
    assert_eq!(refusal(client), Some(Some(Refusal::Itself)));
    assert_eq!(refusal(child), Some(Some(Refusal::Cycle)));
    assert_eq!(refusal(grandchild), Some(Some(Refusal::Cycle)));
    assert_eq!(refusal(top), Some(Some(Refusal::Current)));
    assert_eq!(refusal(free), Some(None));
    assert!(ops.candidates(client, "   ").await.unwrap().is_empty());
    let by_id = ops.candidates(client, &free.0.to_string()).await.unwrap();
    assert!(by_id.iter().any(|c| c.client.client_id == free));
}

#[test]
fn подпись_клиента_для_журнала() {
    let person = ClientPerson {
        client_id: ClientId(12),
        name: Some("Елена Воронова".into()),
        telegram_username: Some("lena_vo".into()),
        telegram_id: Some("5093".into()),
        email: Some("lena@example.com".into()),
    };
    assert_eq!(
        person_label(Some(&person), ClientId(12)),
        "Елена Воронова · @lena_vo · №12"
    );
    let mail_only = ClientPerson {
        telegram_username: None,
        name: None,
        ..person.clone()
    };
    assert_eq!(
        person_label(Some(&mail_only), ClientId(12)),
        "lena@example.com · №12"
    );
    assert_eq!(person_label(None, ClientId(7)), "№7");
    let telegram_only = ClientPerson {
        name: None,
        telegram_username: None,
        email: None,
        ..person
    };
    assert_eq!(
        person_label(Some(&telegram_only), ClientId(12)),
        "id 5093 · №12"
    );
}
