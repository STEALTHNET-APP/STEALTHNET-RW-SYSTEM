use super::*;

#[tokio::test]
async fn проверка_почты_только_с_правом_настроек_и_признак_для_сайта() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(site["email_auth"], "password");
    assert_eq!(site["client_access"]["miniapp"], true);
    assert_eq!(site["client_access"]["devices"], true);
    let (status, _) = h
        .write(
            Method::POST,
            "/api/admin/mail/test",
            &support,
            json!({ "to": "support@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/mail/test",
            &owner,
            json!({ "to": "owner@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "mail.unavailable");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email",
            None,
            Some(json!({ "email": "alex@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.email_password_mode");
    h.db.cleanup().await;
}

#[tokio::test]
async fn шаблоны_писем_правит_только_администратор_и_подстановки_проверяются() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get("/api/admin/mail/letters", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, view) = h.get("/api/admin/mail/letters", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let ids: Vec<&str> = view["letters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"login_code") && ids.contains(&"renewal_short"));
    assert_eq!(view["limits"]["subject"], 150);
    assert_eq!(view["sends"], false);
    let typo = json!({
        "letters": { "topup_done": { "ru": { "subject": "Баланс {balance}", "text": "", "button": "" } } }
    });
    let (status, body) = h
        .write(Method::PUT, "/api/admin/mail/letters", &owner, typo)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "mail_letters.invalid");
    assert_eq!(
        body["error"]["message"],
        "«Баланс пополнен», тема: подстановки {balance} в этом письме нет, можно: {amount}, {bonus}, {total}, {brand}, {email}"
    );
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/mail/letters",
            &support,
            json!({ "groups": { "payments": false } }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, saved) = h
        .write(
            Method::PUT,
            "/api/admin/mail/letters",
            &owner,
            json!({
                "letters": { "topup_done": { "ru": { "subject": "Пополнили на {amount}", "text": "", "button": "" } } }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let topup = saved["letters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == "topup_done")
        .unwrap();
    assert_eq!(topup["saved"]["ru"]["subject"], "Пополнили на {amount}");
    let (status, preview) = h
        .write(
            Method::POST,
            "/api/admin/mail/letters/preview",
            &owner,
            json!({ "letter": "topup_done", "lang": "ru", "subject": "Пополнили на {amount}", "text": "", "button": "" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["subject"], "Пополнили на $10.00");
    assert!(
        preview["html"]
            .as_str()
            .unwrap()
            .starts_with("<!doctype html>")
    );
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/mail/letters/test",
            &owner,
            json!({ "to": "owner@example.com", "letter": "topup_done", "lang": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "mail.unavailable");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/mail/letters/preview",
            &owner,
            json!({ "letter": "nope", "lang": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    h.db.cleanup().await;
}
