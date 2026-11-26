use super::stars::{paid_update, payment_of, stars_invoice};
use super::*;
use sn_domain::ids::AdminId;
use sn_domain::payment::PaymentStatus;
use sn_services::maintenance::{MaintenanceUpdate, TelegramUpdate};

const TEXT: &str = "Обновляем серверы, скоро вернёмся";

async fn owner(services: &Services) -> AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

async fn maintenance_on(services: &Services, admin: AdminId) {
    services
        .maintenance()
        .update(
            &MaintenanceUpdate {
                enabled: true,
                message_ru: Some(TEXT.into()),
                ..MaintenanceUpdate::default()
            },
            admin,
        )
        .await
        .unwrap();
}

async fn calls(server: &MockServer, name: &str) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with(&format!("/{name}")))
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

async fn age_notice(db: &TestDb, chat: i64, minutes: i32) {
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query(
        "UPDATE bot_maintenance_notices SET sent_at = sent_at - make_interval(mins => $2) WHERE chat_id = $1",
    )
    .bind(chat)
    .bind(minutes)
    .execute(conn.conn())
    .await
    .unwrap();
}

fn pre_checkout(update_id: i64, from: i64, payload: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "pre_checkout_query": {
            "id": format!("pcq{update_id}"), "from": user(from), "currency": "XTR",
            "total_amount": 350, "invoice_payload": payload
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn во_время_работ_бот_отвечает_сообщением_не_чаще_раза_в_десять_минут() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    maintenance_on(&services, admin).await;

    handle_update(&ctx, text_update(1, 9101, "/start")).await;
    let first = sent(&server).await;
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0]["chat_id"], 9101);
    assert_eq!(first[0]["text"], TEXT);
    assert!(first[0]["reply_markup"].is_null());
    assert!(
        client_of(&services, 9101).await.is_none(),
        "во время работ бот не регистрирует новых клиентов"
    );

    handle_update(&ctx, text_update(2, 9101, "привет")).await;
    handle_update(&ctx, text_update(3, 9101, "/buy")).await;
    assert_eq!(sent(&server).await.len(), 1, "повтор раньше 10 минут");

    handle_update(&ctx, text_update(4, 9102, "/start ref_abc")).await;
    let other = sent(&server).await;
    assert_eq!(other.len(), 2);
    assert_eq!(other[1]["chat_id"], 9102);

    age_notice(&db, 9101, 9).await;
    handle_update(&ctx, text_update(5, 9101, "ну что там")).await;
    assert_eq!(sent(&server).await.len(), 2);
    age_notice(&db, 9101, 2).await;
    handle_update(&ctx, text_update(6, 9101, "ну что там")).await;
    let again = sent(&server).await;
    assert_eq!(again.len(), 3);
    assert_eq!(again[2]["chat_id"], 9101);
    assert_eq!(again[2]["text"], TEXT);

    services
        .maintenance()
        .update(&MaintenanceUpdate::default(), admin)
        .await
        .unwrap();
    handle_update(&ctx, text_update(7, 9101, "/start")).await;
    let after = sent(&server).await;
    assert_eq!(after.len(), 4);
    assert_ne!(after[3]["text"], TEXT);
    assert!(client_of(&services, 9101).await.is_some());
    db.cleanup().await;
}

#[tokio::test]
async fn во_время_работ_кнопка_показывает_окно_с_тем_же_текстом() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 9201, "/start")).await;
    let before = sent(&server).await.len();
    let admin = owner(&services).await;
    services
        .maintenance()
        .update(
            &MaintenanceUpdate {
                enabled: true,
                message_ru: Some("Я".repeat(260)),
                ..MaintenanceUpdate::default()
            },
            admin,
        )
        .await
        .unwrap();

    for (id, data) in [(2, "subs"), (3, "m"), (4, "buy")] {
        handle_update(&ctx, callback_update(id, 9201, data)).await;
    }
    assert_eq!(
        sent(&server).await.len(),
        before,
        "кнопки ничего не открывают"
    );
    let answers = calls(&server, "answerCallbackQuery").await;
    let alerts: Vec<&serde_json::Value> =
        answers.iter().filter(|a| a["show_alert"] == true).collect();
    assert_eq!(alerts.len(), 3, "{answers:?}");
    let shown = alerts[0]["text"].as_str().unwrap();
    assert_eq!(shown.chars().count(), 200);
    assert!(shown.starts_with("ЯЯЯ") && shown.ends_with('…'));
    db.cleanup().await;
}

#[tokio::test]
async fn администратор_с_telegram_id_пользуется_ботом_во_время_работ() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    services
        .maintenance()
        .set_telegram(
            admin,
            &TelegramUpdate {
                telegram_id: Some(9301),
            },
        )
        .await
        .unwrap();
    maintenance_on(&services, admin).await;

    handle_update(&ctx, text_update(1, 9301, "/start")).await;
    let shown = sent(&server).await;
    assert_eq!(shown.len(), 1);
    assert_ne!(shown[0]["text"], TEXT);
    assert!(client_of(&services, 9301).await.is_some());
    handle_update(&ctx, callback_update(2, 9301, "m")).await;
    assert!(
        calls(&server, "answerCallbackQuery")
            .await
            .iter()
            .all(|a| a["show_alert"].is_null()),
        "администратору окно о работах не показывается"
    );
    assert_eq!(sent(&server).await.len(), 2);

    handle_update(&ctx, text_update(3, 9302, "/start")).await;
    assert_eq!(sent(&server).await.last().unwrap()["text"], TEXT);
    db.cleanup().await;
}

#[tokio::test]
async fn во_время_работ_новые_оплаты_звёздами_отклоняются_а_начатые_проводятся() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    services
        .maintenance()
        .set_telegram(
            admin,
            &TelegramUpdate {
                telegram_id: Some(9402),
            },
        )
        .await
        .unwrap();
    let payment = stars_invoice(&db, &services, 9401).await;
    let tester = stars_invoice(&db, &services, 9402).await;
    maintenance_on(&services, admin).await;

    handle_update(&ctx, pre_checkout(1, 9401, &payment.public_id.to_string())).await;
    handle_update(&ctx, pre_checkout(2, 9402, &tester.public_id.to_string())).await;
    let answers = calls(&server, "answerPreCheckoutQuery").await;
    assert_eq!(answers.len(), 2, "{answers:?}");
    let refused = answers
        .iter()
        .find(|a| a["pre_checkout_query_id"] == "pcq1")
        .unwrap();
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error_message"], TEXT);
    let passed = answers
        .iter()
        .find(|a| a["pre_checkout_query_id"] == "pcq2")
        .unwrap();
    assert_eq!(passed["ok"], true, "{passed}");

    let paid: Update = serde_json::from_value(paid_update(3, 9401, &payment, "stx-9401")).unwrap();
    handle_update(&ctx, paid).await;
    let settled = payment_of(&db, payment.id).await;
    assert_eq!(settled.status, PaymentStatus::Paid);
    assert_eq!(settled.provider_txid.as_deref(), Some("stx-9401"));
    let told = sent(&server).await;
    assert!(
        told.iter().all(|m| m["text"] != TEXT),
        "об оплате бот сообщает как обычно"
    );
    assert!(!told.is_empty());
    db.cleanup().await;
}
