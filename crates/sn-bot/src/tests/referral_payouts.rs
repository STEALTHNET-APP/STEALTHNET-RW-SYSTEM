use super::*;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money};
use sn_services::ServiceError;
use sn_services::settings::ModuleConfigUpdate;

async fn referral_config(
    services: &Services,
    config: serde_json::Value,
) -> Result<(), ServiceError> {
    let (owner, _) = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    let serde_json::Value::Object(config) = config else {
        panic!("настройки — объект");
    };
    services
        .settings_ops()
        .update_module_config(
            ModuleId::Referral,
            &ModuleConfigUpdate {
                config,
                clear: Vec::new(),
            },
            owner,
        )
        .await
        .map(|_| ())
}

async fn referral_money(services: &Services, client: ClientId, minor: i64) {
    let mut tx = services.store().begin().await.unwrap();
    tx.ledger()
        .append(&NewLedgerEntry {
            client_id: client,
            wallet: Wallet::Referral,
            amount: Money::new(minor, Currency::Usd),
            kind: LedgerKind::Contest,
            order_id: None,
            payment_id: None,
            admin_id: None,
            comment: None,
            idempotency_key: Some(format!("test:referral:{}", client.0)),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn кнопки_вывода_в_боте_берутся_из_настроек() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 731, "/start")).await;
    let client = client_of(&services, 731).await.unwrap();
    referral_money(&services, client.id, 2000).await;

    handle_update(&ctx, callback_update(2, 731, "ref:w")).await;
    assert_eq!(
        last_buttons(&server).await,
        ["ref:m:sbp", "ref:m:card", "ref:m:usdt_trc20", "x:referral"]
    );
    assert_eq!(
        &last_labels(&server).await[..3],
        ["СБП", "Банковская карта", "USDT (TRC20)"]
    );

    let refused = referral_config(
        &services,
        serde_json::json!({ "payout_methods": [
            { "id": "sbp", "title": "СБП", "hint": "", "check": "phone", "enabled": false }
        ] }),
    )
    .await
    .unwrap_err();
    assert!(
        refused.to_string().contains("включите хотя бы один способ"),
        "{refused}"
    );

    referral_config(
        &services,
        serde_json::json!({
            "withdraw_min": "2",
            "payout_methods": [
                { "id": "paypal", "title": "PayPal", "hint": "Почта PayPal <для перевода>", "check": "text", "min": "5", "enabled": true },
                { "id": "card", "title": "Карта", "hint": "", "check": "card", "enabled": false },
                { "id": "usdt_trc20", "title": "USDT", "hint": "Адрес TRC20", "check": "trc20", "enabled": true }
            ]
        }),
    )
    .await
    .unwrap();

    handle_update(&ctx, callback_update(3, 731, "x:referral")).await;
    assert!(
        last_buttons(&server).await.contains(&"ref:w".to_string()),
        "{:?}",
        last_buttons(&server).await
    );
    handle_update(&ctx, callback_update(4, 731, "ref:w")).await;
    assert_eq!(
        last_buttons(&server).await,
        ["ref:m:paypal", "ref:m:usdt_trc20", "x:referral"]
    );
    assert_eq!(&last_labels(&server).await[..2], ["PayPal", "USDT"]);

    handle_update(&ctx, callback_update(5, 731, "ref:m:card")).await;
    assert!(
        !last_text(&server).await.contains("Сколько вывести"),
        "{}",
        last_text(&server).await
    );

    handle_update(&ctx, callback_update(6, 731, "ref:m:paypal")).await;
    let text = last_text(&server).await;
    assert!(text.contains("Сколько вывести"), "{text}");
    assert!(text.contains("Минимум — $5.00"), "{text}");
    handle_update(&ctx, text_update(7, 731, "3")).await;
    let text = last_text(&server).await;
    assert!(text.contains("Почта PayPal &lt;для перевода&gt;"), "{text}");
    handle_update(&ctx, text_update(8, 731, "me@example.com")).await;
    let text = last_text(&server).await;
    assert!(text.contains("Минимальная сумма вывода — $5.00"), "{text}");

    handle_update(&ctx, callback_update(9, 731, "ref:m:usdt_trc20")).await;
    let text = last_text(&server).await;
    assert!(text.contains("Минимум — $2.00"), "{text}");
    handle_update(&ctx, text_update(10, 731, "3")).await;
    assert!(last_text(&server).await.contains("Адрес TRC20"));
    handle_update(&ctx, text_update(11, 731, "me@example.com")).await;
    assert!(
        last_text(&server)
            .await
            .contains("Реквизиты указаны неверно")
    );

    handle_update(&ctx, callback_update(12, 731, "ref:m:paypal")).await;
    handle_update(&ctx, text_update(13, 731, "6")).await;
    handle_update(&ctx, text_update(14, 731, " me@example.com ")).await;
    let text = last_text(&server).await;
    assert!(text.contains("принята"), "{text}");
    let overview = referral::overview(&services.module_context(), client.id)
        .await
        .unwrap();
    let request = &overview.withdrawals[0];
    assert_eq!(request.method, "paypal");
    assert_eq!(request.method_title, "PayPal");
    assert_eq!(request.details, "me@example.com");
    assert_eq!(request.amount, Money::new(600, Currency::Usd));
    db.cleanup().await;
}

#[tokio::test]
async fn экран_приглашения_называет_второй_и_третий_уровень() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 741, "/start")).await;

    handle_update(&ctx, callback_update(2, 741, "x:referral")).await;
    let text = last_text(&server).await;
    assert!(
        text.contains(
            "вы получаете 30% с каждой оплаты друга.\nЕщё 10% — с оплат тех, кого пригласят ваши друзья.\nИ 10% — с оплат тех, кого пригласят друзья ваших друзей."
        ),
        "{text}"
    );

    referral_config(
        &services,
        serde_json::json!({ "level2_percent": "5", "level3_percent": "0" }),
    )
    .await
    .unwrap();
    handle_update(&ctx, callback_update(3, 741, "x:referral")).await;
    let text = last_text(&server).await;
    assert!(
        text.contains("Ещё 5% — с оплат тех, кого пригласят ваши друзья."),
        "{text}"
    );
    assert!(!text.contains("друзья ваших друзей"), "{text}");

    referral_config(
        &services,
        serde_json::json!({ "level2_percent": "0", "level3_percent": "7" }),
    )
    .await
    .unwrap();
    handle_update(&ctx, callback_update(4, 741, "x:referral")).await;
    let text = last_text(&server).await;
    assert!(!text.contains("пригласят ваши друзья"), "{text}");
    assert!(
        text.contains("И 7% — с оплат тех, кого пригласят друзья ваших друзей."),
        "{text}"
    );
    db.cleanup().await;
}
