use super::*;
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::money::Money;
use sn_domain::payment::providers;
use sn_domain::settings::{keys, secret_keys};
use sn_modules::{BalanceChange, CoreApi};

async fn balance_off(services: &Services) {
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::BALANCE),
                &serde_json::json!({"enabled": false}),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
}

#[tokio::test]
async fn выключенный_баланс_прячет_кнопку_и_пополнение_в_боте() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    balance_off(&services).await;
    handle_update(&ctx, text_update(1, 611, "/start")).await;
    let buttons = last_buttons(&server).await;
    assert!(!buttons.contains(&"bal".to_string()), "{buttons:?}");
    assert!(buttons.contains(&"buy".to_string()), "{buttons:?}");
    for (update, data) in [(2, "top:1000"), (3, "topx"), (4, "tp:cryptopay")] {
        handle_update(&ctx, callback_update(update, 611, data)).await;
        let text = last_text(&server).await;
        assert!(
            text.contains("Пополнить баланс сейчас нельзя"),
            "{data}: {text}"
        );
        let buttons = last_buttons(&server).await;
        assert!(
            buttons.iter().all(|b| !b.starts_with("top")),
            "{data}: {buttons:?}"
        );
    }
    let client = client_of(&services, 611).await.unwrap();
    services
        .change_balance(BalanceChange {
            client_id: client.id,
            wallet: Wallet::Main,
            amount: Money::new(300, services.settings().currency),
            kind: LedgerKind::TopUp,
            comment: None,
            idempotency_key: "bot-balance-off".into(),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
    handle_update(&ctx, text_update(5, 611, "/start")).await;
    assert!(last_buttons(&server).await.contains(&"bal".to_string()));
    handle_update(&ctx, callback_update(6, 611, "bal")).await;
    let text = last_text(&server).await;
    assert!(text.contains("На балансе"), "{text}");
    assert!(text.contains("Пополнить баланс сейчас нельзя"), "{text}");
    db.cleanup().await;
}

#[test]
fn напоминание_при_выключенном_балансе_зовёт_продлить_а_не_пополнить() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let content = sn_domain::bot_content::Content::default();
    let params = serde_json::json!({
        "amount": {"minor": 699, "currency": "USD"},
        "missing": {"minor": 399, "currency": "USD"},
        "expire_at": "2027-02-14T10:00:00Z",
        "balance_off": true
    });
    for template in [
        notify::templates::AUTORENEW_REMINDER,
        notify::templates::AUTORENEW_CHARGE_FAILED,
    ] {
        let rendered = notify::render(&content, template, &params, utc).unwrap();
        assert!(
            rendered
                .text
                .contains("Продлите её до 14\u{a0}февраля\u{a0}2027"),
            "{}",
            rendered.text
        );
        assert!(!rendered.text.contains("Пополните"), "{}", rendered.text);
        let keyboard = serde_json::to_value(&rendered.keyboard).unwrap();
        assert!(keyboard.to_string().contains("\"buy\""), "{keyboard}");
    }
    let usual = notify::render(
        &content,
        notify::templates::AUTORENEW_REMINDER,
        &serde_json::json!({"missing": {"minor": 399, "currency": "USD"}}),
        utc,
    )
    .unwrap();
    assert!(usual.text.contains("Пополните баланс"), "{}", usual.text);
}

#[tokio::test]
async fn бот_показывает_бонус_у_способа_и_сколько_придёт_на_баланс() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(keys::CURRENCY, &serde_json::json!("RUB"), None)
            .await
            .unwrap();
        conn.settings()
            .set(
                keys::PUBLIC_URL,
                &serde_json::json!("https://panel.example.com"),
                None,
            )
            .await
            .unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::YOOMONEY),
                &serde_json::json!({
                    "enabled": true,
                    "config": {"wallet": "410011111111111"},
                    "topup_bonus": {
                        "enabled": true,
                        "percent": 5,
                        "min": {"minor": 50_000, "currency": "RUB"},
                        "max": {"minor": 10_000, "currency": "RUB"}
                    }
                }),
                None,
            )
            .await
            .unwrap();
        sn_services::settings::write_secret(
            &mut conn,
            services.secrets(),
            &secret_keys::provider(providers::YOOMONEY, "notification_secret"),
            Some("ym-test-secret"),
            None,
        )
        .await
        .unwrap();
    }
    services.reload().await.unwrap();
    handle_update(&ctx, text_update(1, 612, "/start")).await;
    handle_update(&ctx, callback_update(2, 612, "top:50100")).await;
    let labels = last_labels(&server).await;
    assert!(labels.iter().any(|l| l.contains("+5%")), "{labels:?}");
    handle_update(&ctx, callback_update(8, 612, "top:30000")).await;
    let labels = last_labels(&server).await;
    assert!(
        labels.iter().any(|l| l.contains("+5% от 500\u{a0}₽")),
        "{labels:?}"
    );
    handle_update(&ctx, callback_update(9, 612, "tp:yoomoney")).await;
    let text = last_text(&server).await;
    assert!(!text.contains("бонус"), "{text}");
    handle_update(&ctx, callback_update(10, 612, "top:500000")).await;
    handle_update(&ctx, callback_update(11, 612, "tp:yoomoney")).await;
    let text = last_text(&server).await;
    assert!(text.contains("и бонус 100\u{a0}₽"), "{text}");
    handle_update(&ctx, callback_update(12, 612, "top:50100")).await;
    handle_update(&ctx, callback_update(13, 612, "tp:yoomoney")).await;
    let text = last_text(&server).await;
    assert!(
        text.contains("На баланс придёт 526,05\u{a0}₽: 501\u{a0}₽ и бонус 25,05\u{a0}₽"),
        "{text}"
    );
    db.cleanup().await;
}

#[test]
fn сообщение_о_пополнении_называет_бонус_и_сумму_вместе_с_ним() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let content = sn_domain::bot_content::Content::default();
    let plain = serde_json::json!({"amount": {"minor": 10_700, "currency": "RUB"}});
    let rendered = notify::render(&content, notify::templates::TOPUP_DONE, &plain, utc).unwrap();
    assert_eq!(rendered.text, "Баланс пополнен на 107\u{a0}₽.");
    let bonus = serde_json::json!({
        "amount": {"minor": 10_700, "currency": "RUB"},
        "bonus": {"minor": 535, "currency": "RUB"},
        "total": {"minor": 11_235, "currency": "RUB"}
    });
    let rendered = notify::render(&content, notify::templates::TOPUP_DONE, &bonus, utc).unwrap();
    assert_eq!(
        rendered.text,
        "На баланс пришло 112,35\u{a0}₽: 107\u{a0}₽ и бонус 5,35\u{a0}₽."
    );
}
