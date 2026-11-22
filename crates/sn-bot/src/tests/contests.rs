use super::*;
use chrono::{Duration, Utc};
use sn_domain::bot_content::Content;
use sn_domain::module::ModuleId;
use sn_domain::settings::keys;
use sn_modules::catalog::contests::{self, ConditionsDraft, ContestDraft, PrizeConfigDraft};
use sn_modules::catalog::prize::PrizeDraft;

fn draft(title: &str, subscription: bool) -> ContestDraft {
    ContestDraft {
        title: title.into(),
        description: Some("Разыгрываем призы среди клиентов бота".into()),
        prize: PrizeConfigDraft::Single(PrizeDraft::Days { days: 7 }),
        winners: 2,
        starts_at: None,
        ends_at: Utc::now() + Duration::days(3),
        conditions: ConditionsDraft {
            subscription,
            ..ConditionsDraft::default()
        },
        messages: None,
    }
}

async fn app_urls(server: &MockServer) -> Vec<String> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["web_app"]["url"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn клиент_участвует_в_конкурсе_кнопкой_в_боте() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 601, "/start")).await;
    assert!(
        !last_buttons(&server)
            .await
            .contains(&"x:contests".to_string())
    );
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Contests, true, None)
        .await
        .unwrap();
    handle_update(&ctx, text_update(2, 601, "/start")).await;
    assert!(
        !last_buttons(&server)
            .await
            .contains(&"x:contests".to_string())
    );
    let module = services.module_context();
    let open = contests::create(&module, &draft("Розыгрыш для всех", false), None)
        .await
        .unwrap();
    let strict = contests::create(&module, &draft("Для подписчиков", true), None)
        .await
        .unwrap();
    handle_update(&ctx, text_update(3, 601, "/start")).await;
    assert!(
        last_buttons(&server)
            .await
            .contains(&"x:contests".to_string())
    );
    handle_update(&ctx, callback_update(4, 601, "x:contests")).await;
    assert!(last_text(&server).await.contains("Выберите конкурс"));
    let buttons = last_buttons(&server).await;
    assert!(buttons.contains(&format!("ct:{}", open.id)));
    assert!(buttons.contains(&format!("ct:{}", strict.id)));
    assert!(
        last_labels(&server)
            .await
            .contains(&"Розыгрыш для всех".to_string())
    );

    handle_update(&ctx, callback_update(5, 601, &format!("ct:{}", open.id))).await;
    let card = last_text(&server).await;
    assert!(card.contains("<b>Розыгрыш для всех</b>"), "{card}");
    assert!(card.contains("Приз: +7"), "{card}");
    assert!(card.contains("Участвовать может любой клиент"), "{card}");
    assert!(card.contains("Отпечаток жеребьёвки"), "{card}");
    assert!(
        last_buttons(&server)
            .await
            .contains(&format!("cj:{}", open.id))
    );

    handle_update(&ctx, callback_update(6, 601, &format!("cj:{}", open.id))).await;
    let joined = last_text(&server).await;
    assert!(
        joined.starts_with("Готово, вы участвуете в конкурсе «Розыгрыш для всех»"),
        "{joined}"
    );
    assert!(joined.contains("Вы участвуете"), "{joined}");
    assert!(
        !last_buttons(&server)
            .await
            .contains(&format!("cj:{}", open.id))
    );
    let client = client_of(&services, 601).await.unwrap();
    let seen = contests::client_view(&module, open.id, client.id)
        .await
        .unwrap()
        .unwrap();
    assert!(seen.joined);
    assert_eq!(seen.entries, 1);

    handle_update(&ctx, callback_update(7, 601, &format!("cj:{}", strict.id))).await;
    let refused = last_text(&server).await;
    assert!(refused.contains("▫️ Действующая подписка"), "{refused}");
    assert!(refused.contains("выполните условия"), "{refused}");
    let buttons = last_buttons(&server).await;
    assert!(buttons.contains(&"buy".to_string()));
    assert!(buttons.contains(&format!("cj:{}", strict.id)));
    assert!(
        !contests::client_view(&module, strict.id, client.id)
            .await
            .unwrap()
            .unwrap()
            .joined
    );
    db.cleanup().await;
}

#[test]
fn уведомление_победителю_с_промокодом_и_кнопкой_конкурсов() {
    let content = Content::default();
    let rendered = notify::render(
        &content,
        contests::TEMPLATE_WON,
        &serde_json::json!({
            "title": "Зимний <розыгрыш>",
            "prize": "Промокод на скидку 20%",
            "note": "Промокод действует до 1 марта",
            "code": "WIN-ABCD2345"
        }),
        chrono::FixedOffset::east_opt(0).unwrap(),
    )
    .unwrap();
    assert!(
        rendered.text.contains("«Зимний &lt;розыгрыш&gt;»"),
        "{}",
        rendered.text
    );
    assert!(
        rendered.text.contains("<code>WIN-ABCD2345</code>"),
        "{}",
        rendered.text
    );
    let keyboard = serde_json::to_value(&rendered.keyboard).unwrap();
    assert_eq!(
        keyboard["inline_keyboard"][0][0]["callback_data"],
        "x:contests"
    );
    let waiting = notify::render(
        &content,
        contests::TEMPLATE_WAITING,
        &serde_json::json!({ "title": "Розыгрыш", "prize": "+7 дней к подписке" }),
        chrono::FixedOffset::east_opt(0).unwrap(),
    )
    .unwrap();
    assert!(waiting.text.contains("как только у вас появится подписка"));
}

#[tokio::test]
async fn кнопка_рулетки_открывает_ленту_призов_в_мини_приложении() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::PUBLIC_URL,
                &serde_json::json!("https://vpn.example.com"),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    handle_update(&ctx, text_update(1, 602, "/start")).await;
    assert_eq!(app_urls(&server).await, ["https://vpn.example.com/app"]);
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Roulette, true, None)
        .await
        .unwrap();
    handle_update(&ctx, text_update(2, 602, "/start")).await;
    assert_eq!(app_urls(&server).await, ["https://vpn.example.com/app"]);
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO roulette_config (id, config) VALUES (1, '{\"sectors\": [{\"prize\": {\"kind\": \"empty\"}, \"weight\": 1}, {\"prize\": {\"kind\": \"days\", \"days\": 3}, \"weight\": 1}], \"free_every_days\": 7}')",
        )
        .execute(sn_store::Db::conn(&mut conn))
        .await
        .unwrap();
    }
    handle_update(&ctx, text_update(3, 602, "/start")).await;
    assert_eq!(
        app_urls(&server).await,
        [
            "https://vpn.example.com/app",
            "https://vpn.example.com/app/roulette"
        ]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn антибот_просит_кнопку_перед_конкурсом_и_не_пускает_в_режиме_запрета() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Contests, true, None)
        .await
        .unwrap();
    antibot_mode(&services, "confirm").await;
    handle_update(&ctx, text_update(1, 611, "/start")).await;
    let client = client_of(&services, 611).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sn_modules::catalog::antibot::store::save_mark(
            &mut conn,
            client.id,
            &[sn_modules::catalog::antibot::signals::Signal::DisposableEmail],
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }
    let module = services.module_context();
    let open = contests::create(&module, &draft("Розыгрыш с проверкой", false), None)
        .await
        .unwrap();
    handle_update(&ctx, callback_update(2, 611, &format!("cj:{}", open.id))).await;
    let asked = last_text(&server).await;
    assert!(asked.contains("подтвердите, что вы не робот"), "{asked}");
    assert!(
        last_labels(&server)
            .await
            .contains(&"Я не робот".to_string())
    );
    let button = last_buttons(&server)
        .await
        .into_iter()
        .find(|b| b.starts_with(&format!("cjr:{}:", open.id)))
        .unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE antibot_marks SET challenge_at = now() - interval '5 seconds' WHERE client_id = $1",
        )
        .bind(client.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    handle_update(&ctx, callback_update(3, 611, &button)).await;
    let joined = last_text(&server).await;
    assert!(
        joined.starts_with("Готово, вы участвуете в конкурсе «Розыгрыш с проверкой»"),
        "{joined}"
    );
    assert!(
        contests::client_view(&module, open.id, client.id)
            .await
            .unwrap()
            .unwrap()
            .joined
    );

    antibot_mode(&services, "block").await;
    handle_update(&ctx, text_update(4, 612, "/start")).await;
    let other = client_of(&services, 612).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sn_modules::catalog::antibot::store::save_mark(
            &mut conn,
            other.id,
            &[sn_modules::catalog::antibot::signals::Signal::DisposableEmail],
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }
    handle_update(&ctx, callback_update(5, 612, &format!("cj:{}", open.id))).await;
    let denied = last_text(&server).await;
    assert!(
        denied.contains("Участие в конкурсах для этого аккаунта недоступно"),
        "{denied}"
    );
    assert!(
        !denied.contains("антибот") && !denied.contains("подозр"),
        "{denied}"
    );
    assert!(
        !contests::client_view(&module, open.id, other.id)
            .await
            .unwrap()
            .unwrap()
            .joined
    );
    db.cleanup().await;
}
