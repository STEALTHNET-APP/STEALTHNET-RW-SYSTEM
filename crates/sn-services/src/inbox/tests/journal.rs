use super::*;
use sn_domain::paging::PageRequest;
use sn_payments::SavedMethod;
use sn_store::{JournalFilter, JournalRow};

fn kassa_body(
    event: &str,
    payment: Option<Uuid>,
    txid: Option<&str>,
    minor: i64,
) -> serde_json::Value {
    let mut body = serde_json::json!({ "event": event, "status": "paid", "amount": minor });
    if let Some(payment) = payment {
        body["payment"] = serde_json::json!(payment);
    }
    if let Some(txid) = txid {
        body["txid"] = serde_json::json!(txid);
    }
    body
}

fn usd(minor: i64) -> Amount {
    Amount::new(minor, PayCurrency::Usd)
}

fn first_page() -> PageRequest {
    PageRequest::new(None, None)
}

struct Filled {
    client_id: ClientId,
    paid: Payment,
    short: Payment,
    unknown: Uuid,
}

async fn fill(kit: &Kit) -> Filled {
    let client_id = client(&kit.services, 9101).await;
    let paid = kassa_topup(kit, client_id, 1_500).await;
    let txid = paid.provider_txid.clone().unwrap();
    let inbox = kit.services.inbox();
    let first = kassa_body("e-1", Some(paid.public_id), Some(&txid), 1_500);
    inbox
        .handle(kassa_hook(KASSA_SECRET, &first))
        .await
        .unwrap();
    let again = kassa_body("e-2", Some(paid.public_id), Some(&txid), 1_500);
    inbox
        .handle(kassa_hook(KASSA_SECRET, &again))
        .await
        .unwrap();
    let foreign = kassa_body("e-3", None, None, 700);
    inbox
        .handle(kassa_hook(KASSA_SECRET, &foreign))
        .await
        .unwrap();
    let unknown = Uuid::new_v4();
    let stray = kassa_body("e-4", Some(unknown), None, 700);
    inbox
        .handle(kassa_hook(KASSA_SECRET, &stray))
        .await
        .unwrap();
    let forged = kassa_body("e-5", Some(paid.public_id), Some("kt-5"), 1_500);
    let reply = inbox
        .handle(kassa_hook("чужой-секрет", &forged))
        .await
        .unwrap();
    assert_eq!(reply.reply.status(), 401);
    let short = kassa_topup(kit, client_id, 2_000).await;
    let cut = kassa_body("e-6", Some(short.public_id), Some("kt-2"), 1_999);
    inbox.handle(kassa_hook(KASSA_SECRET, &cut)).await.unwrap();
    inbox
        .handle(kassa_hook(KASSA_SECRET, &first))
        .await
        .unwrap();
    Filled {
        client_id,
        paid,
        short,
        unknown,
    }
}

async fn search(kit: &Kit, query: JournalQuery) -> Vec<i64> {
    kit.services
        .inbox()
        .journal(&query, first_page())
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.id)
        .collect()
}

async fn all_ids(kit: &Kit) -> Vec<i64> {
    search(kit, JournalQuery::default()).await
}

async fn stored(
    kit: &Kit,
    id: i64,
) -> (
    Option<Vec<u8>>,
    serde_json::Value,
    Option<serde_json::Value>,
) {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_as("SELECT body, headers, parsed FROM webhook_inbox WHERE id = $1")
        .bind(id)
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn failed_row(kit: &Kit, key: &str, outcome: &WebhookOutcome) -> i64 {
    let mut conn = kit.services.store().conn().await.unwrap();
    let record = conn
        .webhooks()
        .record(&NewInboundWebhook {
            source: KASSA.into(),
            dedupe_key: key.into(),
            remote_ip: Some("203.0.113.9".into()),
            headers: serde_json::json!({ "content-type": "application/json" }),
            body: Some(b"{\"status\":\"paid\"}".to_vec()),
            parsed: stored_outcome(outcome),
            event: None,
        })
        .await
        .unwrap();
    conn.webhooks()
        .finish(
            record.id,
            InboxOutcome::Error,
            None,
            Some("база данных недоступна"),
            None,
        )
        .await
        .unwrap();
    record.id
}

#[tokio::test]
async fn журнал_показывает_итог_каждого_уведомления_простыми_словами() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let filled = fill(&kit).await;
    let page = kit
        .services
        .inbox()
        .journal(&JournalQuery::default(), first_page())
        .await
        .unwrap();
    let lines: Vec<(&str, Option<&str>, Option<&str>)> = page
        .items
        .iter()
        .map(|i| (i.title.as_str(), i.note.as_deref(), i.event.as_deref()))
        .collect();
    assert_eq!(
        lines,
        vec![
            (
                "Сумма не сошлась",
                Some("в уведомлении $19.99, по счёту $20.00"),
                Some("Оплата")
            ),
            ("Отклонено", Some("неверная подпись"), None),
            (
                "Пропущено",
                Some("событие без нашего платежа"),
                Some("Оплата")
            ),
            (
                "Пропущено",
                Some("событие без нашего платежа"),
                Some("Оплата")
            ),
            ("Повтор, уже было", None, Some("Оплата")),
            ("Засчитано", None, Some("Оплата")),
        ]
    );
    let [mismatch, rejected, stray, foreign, repeat, accepted] = page.items.as_slice() else {
        panic!("ожидалось шесть уведомлений");
    };
    assert_eq!(accepted.outcome, InboxOutcome::Accepted);
    assert_eq!(accepted.tone, Tone::Ok);
    assert_eq!(accepted.provider, KASSA);
    assert_eq!(accepted.provider_title, "Касса с подтверждением возвратов");
    assert_eq!(
        accepted.payment.as_ref().map(|p| p.id),
        Some(filled.paid.id)
    );
    assert_eq!(accepted.client_id, Some(filled.client_id));
    assert!(accepted.client.is_some());
    assert_eq!(accepted.txid, filled.paid.provider_txid);
    assert_eq!(accepted.amount_text.as_deref(), Some("$15.00"));
    assert_eq!(accepted.deliveries, 2);
    assert_eq!(repeat.outcome, InboxOutcome::Duplicate);
    assert_eq!(repeat.payment.as_ref().map(|p| p.id), Some(filled.paid.id));
    assert_eq!(foreign.outcome, InboxOutcome::Ignored);
    assert!(foreign.payment.is_none());
    assert_eq!(stray.outcome, InboxOutcome::PaymentNotFound);
    assert!(stray.payment.is_none() && stray.client_id.is_none());
    assert_eq!(rejected.outcome, InboxOutcome::RejectedSignature);
    assert_eq!(rejected.tone, Tone::Bad);
    assert!(rejected.payment.is_none());
    assert!(!rejected.retry.available);
    assert_eq!(mismatch.outcome, InboxOutcome::AmountMismatch);
    assert_eq!(
        mismatch.payment.as_ref().map(|p| p.id),
        Some(filled.short.id)
    );
    assert_eq!(mismatch.client_id, Some(filled.client_id));
    assert_eq!(balance(&kit.services, filled.client_id).await, 1_500);
    assert_eq!(page.retention.days, 30);
    assert_eq!(page.retention.rejected_days, 3);
    assert!(page.providers.iter().any(|p| p.id == KASSA));
    assert!(page.providers.iter().all(|p| p.id != providers::BALANCE));

    let card = kit
        .services
        .inbox()
        .journal_card(accepted.id)
        .await
        .unwrap();
    let steps: Vec<&str> = card.steps.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(
        steps,
        vec![
            "Получено с адреса 203.0.113.9",
            "Засчитано",
            "Платёжка прислала это уведомление ещё раз",
        ]
    );
    assert!(card.facts.iter().any(|f| f.label == "Транзакция платёжки"
        && Some(&f.value) == filled.paid.provider_txid.as_ref()));
    assert_eq!(card.raw_event.as_deref(), Some("e-1"));
    let card = kit
        .services
        .inbox()
        .journal_card(rejected.id)
        .await
        .unwrap();
    assert_eq!(
        card.body,
        BodyView::Missing {
            reason: "Тело не сохраняется: уведомление не прошло проверку".into()
        }
    );
    assert!(card.advice.unwrap().contains("Настройки → Оплата"));
    let card = kit
        .services
        .inbox()
        .journal_card(mismatch.id)
        .await
        .unwrap();
    assert!(card.steps.iter().any(|s| s.text == "Сумма не сошлась"));
    assert!(matches!(
        kit.services.inbox().journal_card(i64::MAX).await,
        Err(ServiceError::NotFound(_))
    ));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn журнал_фильтруется_по_платёжке_итогу_периоду_и_ищет_платёж() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let filled = fill(&kit).await;
    let crypto = InboundWebhook {
        provider: providers::CRYPTOPAY.into(),
        request: WebhookRequest {
            headers: vec![("crypto-pay-api-signature".into(), "00".into())],
            body: Bytes::from_static(b"{\"update_type\":\"invoice_paid\"}"),
            remote_ip: "198.51.100.4".parse().ok(),
            route_suffix: None,
            query: None,
        },
    };
    kit.services.inbox().handle(crypto).await.unwrap();
    let ids = all_ids(&kit).await;
    assert_eq!(ids.len(), 7);
    let crypto_id = ids[0];
    let [mismatch, rejected, stray, foreign, repeat, accepted] = ids[1..] else {
        panic!("ожидалось шесть уведомлений кассы");
    };

    let by = |provider: &str| JournalQuery {
        provider: Some(provider.into()),
        ..JournalQuery::default()
    };
    assert_eq!(
        search(&kit, by(providers::CRYPTOPAY)).await,
        vec![crypto_id]
    );
    assert_eq!(search(&kit, by(KASSA)).await, ids[1..].to_vec());

    let outcome = |group: OutcomeGroup| JournalQuery {
        outcome: Some(group),
        ..JournalQuery::default()
    };
    assert_eq!(
        search(&kit, outcome(OutcomeGroup::Credited)).await,
        vec![accepted]
    );
    assert_eq!(
        search(&kit, outcome(OutcomeGroup::Repeat)).await,
        vec![repeat]
    );
    assert_eq!(
        search(&kit, outcome(OutcomeGroup::Skipped)).await,
        vec![stray, foreign]
    );
    assert_eq!(
        search(&kit, outcome(OutcomeGroup::Rejected)).await,
        vec![crypto_id, rejected]
    );
    assert_eq!(
        search(&kit, outcome(OutcomeGroup::Mismatch)).await,
        vec![mismatch]
    );
    assert_eq!(
        search(&kit, outcome(OutcomeGroup::Attention)).await,
        vec![crypto_id, mismatch, rejected]
    );
    assert!(search(&kit, outcome(OutcomeGroup::Failed)).await.is_empty());

    let now = Utc::now();
    let period = |from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>| JournalQuery {
        from,
        to,
        ..JournalQuery::default()
    };
    assert_eq!(
        search(&kit, period(Some(now - Duration::hours(1)), None)).await,
        ids
    );
    assert!(
        search(&kit, period(Some(now + Duration::hours(1)), None))
            .await
            .is_empty()
    );
    assert!(
        search(&kit, period(None, Some(now - Duration::hours(1))))
            .await
            .is_empty()
    );

    let find = |q: &str| JournalQuery {
        q: Some(q.into()),
        ..JournalQuery::default()
    };
    assert_eq!(
        search(&kit, find(&filled.paid.id.to_string())).await,
        vec![repeat, accepted]
    );
    assert_eq!(
        search(&kit, find(&filled.paid.public_id.to_string())).await,
        vec![repeat, accepted]
    );
    assert_eq!(search(&kit, find(" kt-2 ")).await, vec![mismatch]);
    assert_eq!(search(&kit, find("e-3")).await, vec![foreign]);
    assert_eq!(
        search(&kit, find(&filled.unknown.to_string())).await,
        vec![stray]
    );
    assert!(search(&kit, find("ничего-такого")).await.is_empty());
    assert_eq!(
        search(
            &kit,
            JournalQuery {
                provider: Some(KASSA.into()),
                outcome: Some(OutcomeGroup::Skipped),
                q: Some("e-4".into()),
                ..JournalQuery::default()
            }
        )
        .await,
        vec![stray]
    );

    let inbox = kit.services.inbox();
    let first = inbox
        .journal(&JournalQuery::default(), PageRequest::new(Some(4), None))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 4);
    let rest = inbox
        .journal(
            &JournalQuery::default(),
            PageRequest::new(Some(4), first.next),
        )
        .await
        .unwrap();
    assert_eq!(rest.next, None);
    let paged: Vec<i64> = first
        .items
        .iter()
        .chain(rest.items.iter())
        .map(|i| i.id)
        .collect();
    assert_eq!(paged, ids);

    let mut conn = kit.services.store().conn().await.unwrap();
    let direct = conn
        .webhooks()
        .journal(
            &JournalFilter {
                outcomes: vec![InboxOutcome::Duplicate],
                ..JournalFilter::default()
            },
            None,
            10,
        )
        .await
        .unwrap();
    assert_eq!(direct.len(), 1);
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn секреты_и_личные_данные_маскируются_при_хранении_и_в_карточке() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 9102).await;
    let payment = kassa_topup(&kit, client_id, 1_500).await;
    let txid = payment.provider_txid.clone().unwrap();
    let mut body = kassa_body("e-m", Some(payment.public_id), Some(&txid), 1_500);
    body["email"] = serde_json::json!("ivan.petrov@example.com");
    body["phone"] = serde_json::json!("+79161234567");
    body["card"] = serde_json::json!({ "number": "4111111111111111", "holder": "IVAN PETROV" });
    body["token"] = serde_json::json!("tok_live_abc123456");
    body["ip"] = serde_json::json!("203.0.113.77");
    let mut hook = kassa_hook(KASSA_SECRET, &body);
    hook.request.headers.extend([
        ("Content-Type".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), "Kassa-Hooks/2.0".to_string()),
        (
            "Authorization".to_string(),
            "Bearer live-secret".to_string(),
        ),
    ]);
    kit.services.inbox().handle(hook).await.unwrap();
    let id = all_ids(&kit).await[0];
    assert_eq!(balance(&kit.services, client_id).await, 1_500);

    let (raw, headers, parsed) = stored(&kit, id).await;
    let raw = String::from_utf8(raw.unwrap()).unwrap();
    for secret in [
        "ivan.petrov@example.com",
        "+79161234567",
        "4111111111111111",
        "IVAN PETROV",
        "tok_live_abc123456",
        "203.0.113.77",
    ] {
        assert!(!raw.contains(secret), "{secret} сохранился в теле");
    }
    assert!(raw.contains(&txid));
    let headers = headers.as_object().unwrap();
    assert!(headers.contains_key("content-type"));
    assert!(headers.contains_key("user-agent"));
    assert!(!headers.contains_key("x-kassa-secret"));
    assert!(!headers.contains_key("authorization"));
    assert_eq!(parsed.unwrap()["provider_txid"], txid.as_str());

    let card = kit.services.inbox().journal_card(id).await.unwrap();
    let BodyView::Json { text } = &card.body else {
        panic!("ожидалось тело JSON, получено {:?}", card.body);
    };
    assert!(text.contains("i•••@example.com"));
    assert!(text.contains("+•••••••••67"));
    assert!(text.contains("•••• 1111"));
    assert!(text.contains("I•••"));
    assert!(text.contains("203.0.113.•"));
    assert!(!text.contains("tok_live_abc123456"));
    assert!(card.masked);
    let names: Vec<&str> = card.headers.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(names, vec!["content-type", "user-agent"]);
    assert_eq!(card.remote_ip.as_deref(), Some("203.0.113.9"));

    let saved = WebhookOutcome {
        saved_method: Some(SavedMethod {
            token: "pm-2d9e2f42".into(),
            title: Some("Visa *1111".into()),
        }),
        ..paid_by(&payment, "kt-s")
    };
    let value = stored_outcome(&saved).unwrap();
    assert_eq!(value["saved_method"]["token"], "");
    assert_eq!(value["saved_method"]["title"], "Visa *1111");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повторная_обработка_упавшего_уведомления_не_зачисляет_дважды() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 9103).await;
    let payment = kassa_topup(&kit, client_id, 1_500).await;
    let owner = admin(&kit).await;
    let inbox = kit.services.inbox();
    let txid = payment.provider_txid.clone().unwrap();
    let paid = paid_by(&payment, &txid);
    let failed = failed_row(&kit, "retry-1", &paid).await;

    let card = inbox.journal_card(failed).await.unwrap();
    assert_eq!(card.item.title, "Ошибка обработки");
    assert_eq!(card.item.note.as_deref(), Some("база данных недоступна"));
    assert!(card.item.retry.available);
    assert!(card.advice.unwrap().contains("Обработать ещё раз"));
    assert_eq!(balance(&kit.services, client_id).await, 0);

    let (left, right) = tokio::join!(
        inbox.retry_webhook(failed, owner),
        inbox.retry_webhook(failed, owner)
    );
    let done = match (left, right) {
        (Ok(card), Err(ServiceError::Rejected { code, .. }))
        | (Err(ServiceError::Rejected { code, .. }), Ok(card)) => {
            assert_eq!(code, "webhook.retry_unavailable");
            card
        }
        other => panic!("повтор должен пройти ровно один раз: {other:?}"),
    };
    assert_eq!(done.item.outcome, InboxOutcome::Accepted);
    assert_eq!(done.item.title, "Засчитано");
    assert_eq!(done.item.payment.as_ref().map(|p| p.id), Some(payment.id));
    assert!(!done.item.retry.available);
    let steps: Vec<&str> = done.steps.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(
        steps,
        vec![
            "Получено с адреса 203.0.113.9",
            "Ошибка обработки: база данных недоступна",
            "owner@example.com запускает повторную обработку",
            "Засчитано (повторно, owner@example.com)",
        ]
    );
    assert_eq!(balance(&kit.services, client_id).await, 1_500);
    let again = inbox.retry_webhook(failed, owner).await;
    assert!(
        matches!(&again, Err(ServiceError::Rejected { code, .. }) if *code == "webhook.retry_unavailable"),
        "{again:?}"
    );

    let second = failed_row(&kit, "retry-2", &paid).await;
    let repeat = inbox.retry_webhook(second, owner).await.unwrap();
    assert_eq!(repeat.item.outcome, InboxOutcome::Duplicate);
    assert_eq!(repeat.item.title, "Повтор, уже было");
    assert_eq!(balance(&kit.services, client_id).await, 1_500);

    let resent = kassa_body("e-r", Some(payment.public_id), Some(&txid), 1_500);
    inbox
        .handle(kassa_hook(KASSA_SECRET, &resent))
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1_500);

    let stuck = {
        let mut conn = kit.services.store().conn().await.unwrap();
        let record = conn
            .webhooks()
            .record(&NewInboundWebhook {
                source: KASSA.into(),
                dedupe_key: "retry-3".into(),
                remote_ip: None,
                headers: serde_json::json!({}),
                body: Some(b"{}".to_vec()),
                parsed: stored_outcome(&paid),
                event: None,
            })
            .await
            .unwrap();
        record.id
    };
    let fresh = inbox.retry_webhook(stuck, owner).await;
    assert!(matches!(fresh, Err(ServiceError::Rejected { .. })));
    assert_eq!(
        inbox.journal_card(stuck).await.unwrap().item.title,
        "Обрабатывается"
    );
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE webhook_inbox SET received_at = now() - interval '20 minutes' WHERE id = $1",
        )
        .bind(stuck)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let card = inbox.journal_card(stuck).await.unwrap();
    assert_eq!(card.item.title, "Ошибка обработки");
    assert!(card.item.retry.available);
    let unstuck = inbox.retry_webhook(stuck, owner).await.unwrap();
    assert_eq!(unstuck.item.outcome, InboxOutcome::Duplicate);

    let rejected = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.webhooks()
            .record_rejected(
                &NewInboundWebhook {
                    source: KASSA.into(),
                    dedupe_key: "rejected:retry-4".into(),
                    remote_ip: None,
                    headers: serde_json::json!({}),
                    body: None,
                    parsed: None,
                    event: None,
                },
                InboxOutcome::Error,
                Some("касса не ответила на проверку"),
            )
            .await
            .unwrap();
        all_ids(&kit).await[0]
    };
    let card = inbox.journal_card(rejected).await.unwrap();
    assert!(!card.item.retry.available);
    assert!(
        card.item
            .retry
            .note
            .as_deref()
            .unwrap()
            .contains("пришлёт его ещё раз")
    );
    match inbox.retry_webhook(rejected, owner).await {
        Err(ServiceError::Rejected { code, message }) => {
            assert_eq!(code, "webhook.retry_unavailable");
            assert!(message.contains("не прошло проверку"));
        }
        other => panic!("ожидался отказ: {other:?}"),
    }
    assert!(matches!(
        inbox.retry_webhook(i64::MAX, owner).await,
        Err(ServiceError::NotFound(_))
    ));

    let mut conn = kit.services.store().conn().await.unwrap();
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'payment.webhook_retried' AND actor_id = $1",
    )
    .bind(owner.0)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(audited, 3);
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn главная_видит_отклонённые_и_упавшие_уведомления_за_сутки() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let inbox = kit.services.inbox();
    let quiet = inbox.webhook_attention().await.unwrap();
    assert_eq!((quiet.rejected, quiet.failed), (0, 0));
    assert!(quiet.providers.is_empty());

    let filled = fill(&kit).await;
    let short = filled.short.provider_txid.clone().unwrap();
    let failed = failed_row(&kit, "attention-1", &paid_by(&filled.short, &short)).await;
    let old = failed_row(&kit, "attention-2", &paid_by(&filled.short, &short)).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE webhook_inbox SET received_at = now() - interval '25 hours' WHERE id = $1",
        )
        .bind(old)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let attention = inbox.webhook_attention().await.unwrap();
    assert_eq!(attention.rejected, 2);
    assert_eq!(attention.failed, 1);
    assert_eq!(
        attention.providers,
        vec!["Касса с подтверждением возвратов".to_string()]
    );
    assert_eq!(attention.hours, 24);

    let owner = admin(&kit).await;
    inbox.retry_webhook(failed, owner).await.unwrap();
    let after = inbox.webhook_attention().await.unwrap();
    assert_eq!((after.rejected, after.failed), (2, 0));

    let money = kit
        .services
        .admin()
        .dashboard(crate::admin::Sees::ALL)
        .await
        .unwrap();
    assert_eq!(money.webhooks.map(|w| w.rejected), Some(2));
    let support = kit
        .services
        .admin()
        .dashboard(crate::admin::Sees {
            money: false,
            refunds: false,
            infrastructure: true,
        })
        .await
        .unwrap();
    assert!(support.webhooks.is_none());
    kit.db.cleanup().await;
}

#[test]
fn технические_ошибки_базы_пишутся_понятно() {
    let row = |error: &str| JournalRow {
        id: 1,
        source: KASSA.into(),
        outcome: InboxOutcome::Error,
        payment_id: None,
        client_id: None,
        error: Some(error.into()),
        remote_ip: None,
        received_at: Utc::now(),
        processed_at: Some(Utc::now()),
        claimed_at: None,
        deliveries: 1,
        parsed: None,
        event: None,
        has_body: false,
        payment: None,
        legacy_path: false,
        legacy_invoice: false,
    };
    let note = |error: &str| crate::inbox::journal::verdict_note(&row(error));
    assert_eq!(
        note("error returned from database: сервер недоступен").as_deref(),
        Some("ошибка базы данных: сервер недоступен")
    );
    assert_eq!(
        note("pool timed out while waiting for an open connection").as_deref(),
        Some("база данных не ответила вовремя")
    );
    assert_eq!(
        note("платёж уже paid").as_deref(),
        Some("платёж уже оплачен")
    );
}

#[test]
fn точная_сумма_не_округляет_копейки() {
    assert_eq!(
        exact(Amount::new(49_950, PayCurrency::Rub)),
        "499,50\u{a0}₽"
    );
    assert_eq!(exact(Amount::new(50_000, PayCurrency::Rub)), "500\u{a0}₽");
    assert_eq!(exact(usd(1_999)), "$19.99");
}

#[tokio::test]
async fn поиск_не_заглядывает_в_тела_сохранённые_до_маскирования() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let mut conn = kit.services.store().conn().await.unwrap();
    let row = |key: &'static str, body: &'static [u8]| NewInboundWebhook {
        source: KASSA.into(),
        dedupe_key: key.into(),
        remote_ip: Some("203.0.113.9".into()),
        headers: serde_json::json!({}),
        body: Some(body.to_vec()),
        parsed: None,
        event: None,
    };
    let legacy = conn
        .webhooks()
        .record(&row(
            "legacy-1",
            b"TransactionId=777&Token=tk_secret_123456&Email=ivan.petrov@example.com",
        ))
        .await
        .unwrap()
        .id;
    sqlx::query(
        "UPDATE webhook_inbox SET received_at = (SELECT installed_on FROM _sqlx_migrations WHERE version = 290) \
         - interval '1 day' WHERE id = $1",
    )
    .bind(legacy)
    .execute(conn.conn())
    .await
    .unwrap();
    let fresh = conn
        .webhooks()
        .record(&row("fresh-1", b"{\"comment\":\"order-for-summer\"}"))
        .await
        .unwrap()
        .id;
    drop(conn);
    let query = |q: &str| JournalQuery {
        q: Some(q.into()),
        ..JournalQuery::default()
    };
    assert!(search(&kit, query("tk_secret_1")).await.is_empty());
    assert!(search(&kit, query("ivan.petrov")).await.is_empty());
    assert_eq!(search(&kit, query("order-for-summer")).await, vec![fresh]);
    assert!(all_ids(&kit).await.contains(&legacy));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn поиск_находит_уведомление_по_заказу_и_номеру_счёта_в_кассе() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 9105).await;
    let paid = kassa_topup(&kit, client_id, 1_500).await;
    let txid = paid.provider_txid.clone().unwrap();
    kit.services
        .inbox()
        .handle(kassa_hook(
            KASSA_SECRET,
            &kassa_body("e-1", Some(paid.public_id), None, 1_500),
        ))
        .await
        .unwrap();
    let ids = all_ids(&kit).await;
    assert_eq!(ids.len(), 1);
    let mut conn = kit.services.store().conn().await.unwrap();
    let (written, order): (Option<String>, Uuid) = sqlx::query_as(
        "SELECT w.parsed ->> 'provider_txid', o.public_id FROM webhook_inbox w \
         JOIN orders o ON o.id = $2 WHERE w.id = $1",
    )
    .bind(ids[0])
    .bind(paid.order_id.0)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(written, None);
    let find = |q: &str| JournalQuery {
        q: Some(q.into()),
        ..JournalQuery::default()
    };
    assert_eq!(search(&kit, find(&txid)).await, ids);
    assert_eq!(search(&kit, find(&order.to_string())).await, ids);
    assert!(
        search(&kit, find(&Uuid::new_v4().to_string()))
            .await
            .is_empty()
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn карта_сохраняется_по_повтору_кассы_после_ручной_обработки() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8601).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let with_card = WebhookOutcome {
        saved_method: Some(SavedMethod {
            token: "pm-retry-1".into(),
            title: Some("Visa *4242".into()),
        }),
        ..paid_by(&payment, "kt-card")
    };
    let id = failed_row(&kit, "kt-card", &with_card).await;
    kit.services.inbox().retry_webhook(id, admin).await.unwrap();
    let mut conn = kit.services.store().conn().await.unwrap();
    assert_eq!(
        conn.payments().get(payment.id).await.unwrap().status,
        PaymentStatus::Paid
    );
    assert!(
        conn.saved_methods()
            .active(client_id, KASSA)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    kit.services
        .inbox()
        .apply_outcome(KASSA, &with_card)
        .await
        .unwrap();
    let mut conn = kit.services.store().conn().await.unwrap();
    let saved = conn
        .saved_methods()
        .active(client_id, KASSA)
        .await
        .unwrap()
        .expect("карта сохранена по повтору кассы");
    assert_eq!(saved.title.as_deref(), Some("Visa *4242"));
    conn.saved_methods().revoke(saved.id).await.unwrap();
    drop(conn);
    kit.services
        .inbox()
        .apply_outcome(KASSA, &with_card)
        .await
        .unwrap();
    let mut conn = kit.services.store().conn().await.unwrap();
    assert!(
        conn.saved_methods()
            .active(client_id, KASSA)
            .await
            .unwrap()
            .is_none(),
        "удалённая клиентом карта не возвращается"
    );
    drop(conn);
    kit.db.cleanup().await;
}

async fn payment_now(kit: &Kit, payment: &Payment) -> Payment {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().get(payment.id).await.unwrap()
}

async fn later_row(kit: &Kit, key: &str, outcome: &WebhookOutcome, payment: &Payment) {
    let mut conn = kit.services.store().conn().await.unwrap();
    let record = conn
        .webhooks()
        .record(&NewInboundWebhook {
            source: KASSA.into(),
            dedupe_key: key.into(),
            remote_ip: Some("203.0.113.9".into()),
            headers: serde_json::json!({}),
            body: Some(b"{}".to_vec()),
            parsed: stored_outcome(outcome),
            event: None,
        })
        .await
        .unwrap();
    conn.webhooks()
        .finish(
            record.id,
            InboxOutcome::Ignored,
            Some(payment.id),
            Some("платёж уже failed"),
            None,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn повтор_не_проводит_оплату_после_возврата() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8602).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let paid = paid_by(&payment, "kt-late");
    let id = failed_row(&kit, "kt-late", &paid).await;
    let refunded = WebhookOutcome {
        event_key: "refund:kt-late".into(),
        status: ProviderStatus::Refunded,
        ..paid_by(&payment, "kt-late")
    };
    let answer = kit
        .services
        .inbox()
        .apply_outcome(KASSA, &refunded)
        .await
        .unwrap();
    assert!(matches!(answer, AcceptResult::Ignored { .. }), "{answer:?}");
    let refused = kit
        .services
        .inbox()
        .retry_webhook(id, admin)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), RETRY_REVERSED);
    assert!(refused.to_string().contains("о возврате"), "{refused}");
    assert_eq!(
        payment_now(&kit, &payment).await.status,
        PaymentStatus::Pending
    );
    let card = kit.services.inbox().journal_card(id).await.unwrap();
    assert_eq!(card.item.outcome, InboxOutcome::Error);
    let mut conn = kit.services.store().conn().await.unwrap();
    let noticed = conn
        .payment_notices()
        .for_payment(payment.id)
        .await
        .unwrap();
    assert_eq!(noticed.len(), 1, "{noticed:?}");
    assert_eq!(noticed[0].kind, NoticeKind::RefundReported);
    assert_eq!(noticed[0].amount, Some(usd(1_000)));
    drop(conn);
    kit.services
        .inbox()
        .apply_outcome(KASSA, &paid)
        .await
        .unwrap();
    assert_eq!(
        payment_now(&kit, &payment).await.status,
        PaymentStatus::Paid
    );
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Review,
        "оплата после сообщения о возврате не выдаётся сама"
    );
    assert!(
        !jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder {
                order_id: payment.order_id
            })
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повтор_сверяет_статус_у_кассы() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8603).await;
    let tariff = standard(&kit.services).await;
    let mut cases = Vec::new();
    for (n, now) in [
        ProviderStatus::Canceled,
        ProviderStatus::Refunded,
        ProviderStatus::Pending,
    ]
    .into_iter()
    .enumerate()
    {
        let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
        let txid = format!("kt-check-{n}");
        let id = failed_row(&kit, &txid, &paid_by(&payment, &txid)).await;
        *kassa.status.lock().unwrap() = Some(WebhookOutcome {
            status: now,
            ..paid_by(&payment, &txid)
        });
        let refused = kit
            .services
            .inbox()
            .retry_webhook(id, admin)
            .await
            .unwrap_err();
        cases.push(refused.code());
        assert_eq!(
            payment_now(&kit, &payment).await.status,
            PaymentStatus::Pending
        );
        assert!(kassa.lookups.lock().unwrap().contains(&txid));
    }
    assert_eq!(cases, [RETRY_REVERSED, RETRY_REVERSED, RETRY_UNCONFIRMED]);
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let id = failed_row(&kit, "kt-check-ok", &paid_by(&payment, "kt-check-ok")).await;
    *kassa.status.lock().unwrap() = Some(paid_by(&payment, "kt-check-ok"));
    let card = kit.services.inbox().retry_webhook(id, admin).await.unwrap();
    assert_eq!(card.item.outcome, InboxOutcome::Accepted);
    assert_eq!(
        payment_now(&kit, &payment).await.status,
        PaymentStatus::Paid
    );
    let silent = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let id = failed_row(&kit, "kt-check-none", &paid_by(&silent, "kt-check-none")).await;
    *kassa.status.lock().unwrap() = None;
    kit.services.inbox().retry_webhook(id, admin).await.unwrap();
    assert_eq!(payment_now(&kit, &silent).await.status, PaymentStatus::Paid);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повтор_сверяет_статус_по_номеру_панели_если_касса_знает_только_его() {
    let kassa = Arc::new(Kassa::default());
    *kassa.knows_order_only.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8604).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let paid = paid_by(&payment, "kt-order");
    let id = failed_row(&kit, "kt-order", &paid).await;
    *kassa.status.lock().unwrap() = Some(WebhookOutcome {
        status: ProviderStatus::Refunded,
        ..paid.clone()
    });
    let refused = kit
        .services
        .inbox()
        .retry_webhook(id, admin)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), RETRY_REVERSED);
    *kassa.status.lock().unwrap() = Some(paid);
    let card = kit.services.inbox().retry_webhook(id, admin).await.unwrap();
    assert_eq!(card.item.outcome, InboxOutcome::Accepted);
    assert_eq!(
        kassa.lookups.lock().unwrap().clone(),
        vec![payment.public_id.to_string(), payment.public_id.to_string()]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повтор_запрещён_после_отмены_той_же_транзакции() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8604).await;
    let tariff = standard(&kit.services).await;
    let cancel = |payment: &Payment, txid: &str| WebhookOutcome {
        event_key: format!("cancel:{txid}"),
        status: ProviderStatus::Canceled,
        ..paid_by(payment, txid)
    };
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let id = failed_row(&kit, "kt-a", &paid_by(&payment, "kt-a")).await;
    later_row(&kit, "cancel-a", &cancel(&payment, "kt-a"), &payment).await;
    let refused = kit
        .services
        .inbox()
        .retry_webhook(id, admin)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), RETRY_REVERSED);
    assert!(refused.to_string().contains("отменён"), "{refused}");
    assert_eq!(
        payment_now(&kit, &payment).await.status,
        PaymentStatus::Pending
    );
    let other = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    later_row(&kit, "cancel-c", &cancel(&other, "kt-c"), &other).await;
    let other_id = failed_row(&kit, "kt-c", &paid_by(&other, "kt-c")).await;
    later_row(&kit, "cancel-d", &cancel(&other, "kt-d"), &other).await;
    kit.services
        .inbox()
        .retry_webhook(other_id, admin)
        .await
        .unwrap();
    assert_eq!(payment_now(&kit, &other).await.status, PaymentStatus::Paid);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn приём_уведомления_не_обрабатывает_строку_пока_её_повторяют() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8605).await;
    let payment = kassa_topup(&kit, client_id, 1_500).await;
    let txid = payment.provider_txid.clone().unwrap();
    let body = kassa_body("e-claim", Some(payment.public_id), Some(&txid), 1_500);
    let hook = kassa_hook(KASSA_SECRET, &body);
    let key = dedupe_key(&hook.request);
    let parsed = kit
        .services
        .payments()
        .get(KASSA)
        .unwrap()
        .parse_webhook(&kit.services.payments().config(KASSA), &hook.request)
        .await
        .unwrap();
    let id = failed_row(&kit, &key, &parsed).await;
    let fresh = Utc::now() - Duration::minutes(RETRY_STALE_MINUTES);
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        assert!(
            conn.webhooks()
                .claim_retry(id, admin, fresh, "")
                .await
                .unwrap()
                .is_some()
        );
    }
    let busy = kit.services.inbox().handle(hook.clone()).await.unwrap();
    assert_eq!(busy.reply.status(), 503);
    assert_eq!(
        payment_now(&kit, &payment).await.status,
        PaymentStatus::Pending
    );
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.webhooks()
            .finish(id, InboxOutcome::Error, None, Some("сбой"), Some(admin))
            .await
            .unwrap();
        assert!(conn.webhooks().claim(id, fresh).await.unwrap());
    }
    let taken = kit
        .services
        .inbox()
        .retry_webhook(id, admin)
        .await
        .unwrap_err();
    assert_eq!(taken.code(), "webhook.retry_unavailable");
    assert!(taken.to_string().contains("обрабатывается"), "{taken}");
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.webhooks()
            .finish(id, InboxOutcome::Error, None, Some("сбой"), None)
            .await
            .unwrap();
    }
    let done = kit.services.inbox().handle(hook).await.unwrap();
    assert_eq!(done.reply, kassa_reply());
    assert_eq!(
        payment_now(&kit, &payment).await.status,
        PaymentStatus::Paid
    );
    let card = kit.services.inbox().journal_card(id).await.unwrap();
    assert_eq!(card.item.outcome, InboxOutcome::Accepted);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn аудит_повтора_пишется_до_зачисления() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8606).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let id = failed_row(&kit, "kt-crash", &paid_by(&payment, "kt-crash")).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query(
            "CREATE FUNCTION finish_breaks() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
             IF NEW.processed_at IS DISTINCT FROM OLD.processed_at THEN \
             RAISE EXCEPTION 'итог не записан'; END IF; RETURN NEW; END $$",
        )
        .execute(conn.conn())
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER finish_breaks BEFORE UPDATE ON webhook_inbox \
             FOR EACH ROW EXECUTE FUNCTION finish_breaks()",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    assert!(kit.services.inbox().retry_webhook(id, admin).await.is_err());
    assert_eq!(
        payment_now(&kit, &payment).await.status,
        PaymentStatus::Paid
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let started: Vec<(String, Option<i64>, serde_json::Value)> = sqlx::query_as(
        "SELECT actor_kind, actor_id, details FROM audit_log \
         WHERE action = 'payment.webhook_retry_started' AND entity_id = $1",
    )
    .bind(id)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0].0, "admin");
    assert_eq!(started[0].1, Some(admin.0));
    assert_eq!(started[0].2["provider"], KASSA);
    drop(conn);
    kit.db.cleanup().await;
}
