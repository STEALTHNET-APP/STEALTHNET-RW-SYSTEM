use super::delivery::{MAX_ATTEMPTS, Sent, outcome, retry_delay};
use super::manifest::{FieldKind, parse};
use super::*;
use chrono::TimeZone;

pub const HELLO: &str = r#"{
  "format": 1,
  "id": "hello",
  "name": "Привет",
  "version": "1.0.0",
  "author": "Студия Пример",
  "description": "Здоровается с новыми клиентами и благодарит за оплату",
  "site": "https://example.com/hello",
  "webhook": "https://hello.example.com/stealthnet/events",
  "events": ["client_registered", "payment_paid"],
  "permissions": ["clients_write", "clients_read", "money"],
  "page": { "url": "https://hello.example.com/admin", "title": "Привет: журнал" },
  "settings": [
    { "key": "greeting", "type": "text", "label": "Текст приветствия", "large": true, "default": "Спасибо, что с нами!" },
    { "key": "thank_after_payment", "type": "flag", "label": "Благодарить за оплату", "default": true },
    { "key": "crm_token", "type": "secret", "label": "Токен CRM", "hint": "Из личного кабинета CRM" },
    { "key": "limit", "type": "number", "label": "Сколько сообщений в час", "default": 20 },
    { "key": "tone", "type": "select", "label": "Тон", "options": ["formal", "friendly"], "labels": ["На вы", "На ты"], "default": "friendly", "required": true }
  ]
}"#;

fn messages(source: &str) -> Vec<(String, String)> {
    parse(source)
        .expect_err("ожидались ошибки")
        .into_iter()
        .map(|p| (p.path, p.message))
        .collect()
}

fn has(problems: &[(String, String)], path: &str, part: &str) -> bool {
    problems.iter().any(|(p, m)| p == path && m.contains(part))
}

#[test]
fn описание_модуля_разбирается_полностью() {
    let manifest = parse(HELLO).unwrap();
    assert_eq!(manifest.id, "hello");
    assert_eq!(manifest.name, "Привет");
    assert_eq!(manifest.author, "Студия Пример");
    assert_eq!(
        manifest.webhook.as_deref(),
        Some("https://hello.example.com/stealthnet/events")
    );
    assert_eq!(manifest.events, ["client_registered", "payment_paid"]);
    assert_eq!(
        manifest.permissions,
        [
            Permission::ClientsRead,
            Permission::ClientsWrite,
            Permission::Money
        ]
    );
    let page = manifest.page.as_ref().unwrap();
    assert_eq!(page.title, "Привет: журнал");
    assert_eq!(page.permission, Permission::Modules);
    assert_eq!(manifest.settings.len(), 5);
    let tone = &manifest.settings[4];
    assert_eq!(
        tone.kind,
        FieldKind::Select {
            options: vec!["formal".into(), "friendly".into()]
        }
    );
    assert_eq!(tone.default.as_deref(), Some("friendly"));
    assert!(tone.required);
    assert_eq!(manifest.settings[1].default.as_deref(), Some("true"));
    assert_eq!(manifest.settings[3].default.as_deref(), Some("20"));
    assert!(manifest.settings[2].is_secret());
    let json = serde_json::to_value(&manifest.settings[4]).unwrap();
    assert_eq!(json["kind"]["type"], "select");
    assert_eq!(json["kind"]["options"][1], "friendly");
    assert_eq!(json["labels"][0], "На вы");
    let stored: Manifest =
        serde_json::from_value(serde_json::to_value(&manifest).unwrap()).unwrap();
    assert_eq!(stored, manifest);
}

#[test]
fn не_json_и_не_тот_формат_объясняются_по_русски() {
    let problems = messages("{\n  \"id\": \"hello\",\n  oops\n}");
    assert!(has(&problems, "", "строке 3"), "{problems:?}");
    assert!(has(&messages("[1, 2]"), "", "фигурных скобках"));
    assert!(has(&messages("   "), "", "пустой"));
    assert!(has(
        &messages(r#"{"format": 2, "id": "x"}"#),
        "format",
        "только 1"
    ));
    assert!(has(
        &messages(r#"{"id": "hello"}"#),
        "format",
        "\"format\": 1"
    ));
    let huge = format!("{{\"format\": 1, \"pad\": \"{}\"}}", "a".repeat(70 * 1024));
    assert!(has(&messages(&huge), "", "64 КБ"));
}

#[test]
fn ошибки_описания_собраны_все_сразу() {
    let problems = messages(
        r#"{
      "format": 1,
      "id": "Promo!",
      "name": "",
      "version": "1",
      "permisions": ["money"],
      "events": ["payment_paid", "payment_done", 5],
      "permissions": ["clients_read", "admins", "superuser"],
      "webhook": "http://hello.example.com/events",
      "page": { "url": "https://192.168.1.10/admin", "color": "red" },
      "settings": "нет"
    }"#,
    );
    assert!(
        has(&problems, "permisions", "неизвестное поле"),
        "{problems:?}"
    );
    assert!(has(&problems, "id", "латинские буквы"));
    assert!(has(&problems, "name", "пустым"));
    assert!(has(&problems, "author", "обязательно"));
    assert!(has(&problems, "events[0]", "«Деньги»"));
    assert!(has(
        &problems,
        "events[1]",
        "неизвестное событие «payment_done»"
    ));
    assert!(has(&problems, "events[2]", "строка"));
    assert!(has(
        &problems,
        "permissions[1]",
        "«Администраторы» модулю дать нельзя"
    ));
    assert!(has(
        &problems,
        "permissions[2]",
        "неизвестное право «superuser»"
    ));
    assert!(has(
        &problems,
        "webhook",
        "http:// можно только для этого же сервера"
    ));
    assert!(has(&problems, "page.url", "внутренней сети"));
    assert!(has(&problems, "page.color", "неизвестное поле"));
    assert!(has(&problems, "settings", "список"));
}

#[test]
fn имя_модуля_не_совпадает_со_встроенными() {
    let base = |id: &str| HELLO.replace("\"id\": \"hello\"", &format!("\"id\": \"{id}\""));
    assert!(has(&messages(&base("promo")), "id", "встроенного модуля"));
    assert!(has(&messages(&base("me")), "id", "занято панелью"));
    assert!(has(&messages(&base("x")), "id", "от 2 до 40"));
    assert!(has(&messages(&base("-crm")), "id", "первая — буква"));
    assert!(parse(&base("crm-sync2")).is_ok());
}

#[test]
fn события_без_адреса_не_принимаются() {
    let source = HELLO.replace(
        "\"webhook\": \"https://hello.example.com/stealthnet/events\",",
        "",
    );
    assert!(has(&messages(&source), "webhook", "укажите адрес"));
    let quiet = source.replace("\"events\": [\"client_registered\", \"payment_paid\"],", "");
    let manifest = parse(&quiet).unwrap();
    assert!(manifest.events.is_empty());
    assert!(manifest.webhook.is_none());
}

#[test]
fn пустой_адрес_для_событий_считается_неуказанным() {
    let with =
        |address: &str| HELLO.replace("\"https://hello.example.com/stealthnet/events\"", address);
    for empty in ["null", "\"\"", "\"  \""] {
        assert_eq!(
            messages(&with(empty)),
            [(
                "webhook".to_string(),
                "укажите адрес, куда слать события: модуль подписан на события".to_string()
            )],
            "{empty}"
        );
        let quiet =
            with(empty).replace("\"events\": [\"client_registered\", \"payment_paid\"],", "");
        assert_eq!(parse(&quiet).unwrap().webhook, None, "{empty}");
    }
    for wrong in ["\"http://hello.example.com/events\"", "5"] {
        let problems = messages(&with(wrong));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(!has(&problems, "webhook", "укажите адрес"), "{problems:?}");
    }
}

#[test]
fn поля_настроек_проверяются() {
    let source = r#"{
      "format": 1, "id": "crm", "name": "CRM", "version": "1", "author": "Я",
      "settings": [
        { "key": "Token", "type": "secret", "label": "Ключ", "default": "x" },
        { "key": "mode", "type": "select", "label": "Режим", "options": [], "default": "a" },
        { "key": "mode", "type": "text", "label": "Дубль" },
        { "key": "level", "type": "number", "label": "Уровень", "default": "много" },
        { "key": "kind", "type": "color", "label": "Цвет" },
        { "key": "flag", "type": "flag", "label": "Флаг", "large": true },
        { "key": "pick", "type": "select", "label": "Выбор", "options": ["a", "b"], "labels": ["А"] },
        { "type": "text" }
      ]
    }"#;
    let problems = messages(source);
    assert!(
        has(&problems, "settings[0].key", "нижнем регистре"),
        "{problems:?}"
    );
    assert!(has(&problems, "settings[0].default", "секретного поля"));
    assert!(has(&problems, "settings[1].options", "от 1 до 50"));
    assert!(has(&problems, "settings[1].default", "из options"));
    assert!(has(&problems, "settings[2].key", "уже есть"));
    assert!(has(&problems, "settings[3].default", "число"));
    assert!(has(
        &problems,
        "settings[4].type",
        "неизвестный тип «color»"
    ));
    assert!(has(&problems, "settings[5].large", "только у type: text"));
    assert!(has(&problems, "settings[6].labels", "столько же"));
    assert!(has(&problems, "settings[7].key", "обязательно"));
    assert!(has(&problems, "settings[7].label", "обязательно"));
}

#[test]
fn повторы_с_паузами_и_пределом() {
    let pauses: Vec<i64> = (1..MAX_ATTEMPTS)
        .map(|n| retry_delay(n).unwrap().num_seconds())
        .collect();
    assert_eq!(pauses, [30, 120, 600, 1_800, 3_600, 10_800, 21_600]);
    assert!(retry_delay(MAX_ATTEMPTS).is_none());
    assert!(retry_delay(0).is_none());
    let now = Utc.with_ymd_and_hms(2026, 11, 18, 12, 0, 0).unwrap();
    let bad = Sent {
        http_status: Some(500),
        duration_ms: 12,
        error: Some("модуль ответил кодом 500".into()),
        response: None,
        retry: true,
    };
    let first = outcome(1, bad.clone(), now, true);
    assert_eq!(first.status, "pending");
    assert_eq!(first.next_attempt_at, Some(now + Duration::seconds(30)));
    let last = outcome(MAX_ATTEMPTS, bad.clone(), now, true);
    assert_eq!(last.status, "failed");
    assert_eq!(last.next_attempt_at, None);
    assert_eq!(outcome(1, bad.clone(), now, false).status, "failed");
    let refused = outcome(
        1,
        Sent {
            retry: false,
            ..bad
        },
        now,
        true,
    );
    assert_eq!(refused.status, "failed");
    assert_eq!(refused.next_attempt_at, None);
    let good = Sent {
        http_status: Some(204),
        duration_ms: 3,
        error: None,
        response: None,
        retry: true,
    };
    assert_eq!(outcome(3, good, now, true).status, "done");
}

#[test]
fn тело_события_без_служебного_типа_внутри_данных() {
    let delivery = store::Claimed {
        id: 9,
        event_id: Some(41),
        kind: "payment_paid".into(),
        payload: serde_json::json!({ "type": "payment_paid", "client_id": 3, "amount": { "minor": 699, "currency": "RUB" } }),
        occurred_at: Utc.with_ymd_and_hms(2026, 11, 18, 12, 0, 0).unwrap(),
        attempts: 0,
    };
    let body: Value = serde_json::from_slice(&delivery::body("hello", &delivery)).unwrap();
    assert_eq!(body["id"], "evt_41");
    assert_eq!(body["type"], "payment_paid");
    assert_eq!(body["module"], "hello");
    assert_eq!(body["data"]["client_id"], 3);
    assert_eq!(body["data"]["amount"]["minor"], 699);
    assert!(body["data"].get("type").is_none());
    let service = store::Claimed {
        event_id: None,
        kind: "ping".into(),
        payload: serde_json::json!({}),
        ..delivery
    };
    assert_eq!(delivery::message_id(&service), "msg_9");
}

#[test]
fn права_событий_модулей_совпадают_с_правами_событий_панели() {
    use sn_domain::client::RegistrationSource;
    use sn_domain::events::DomainEvent;
    use sn_domain::ids::{ClientId, OrderId, PaymentId, SubscriptionId, TariffId, TicketId};
    use sn_domain::ledger::{LedgerKind, Wallet};
    use sn_domain::money::{Amount, Currency, Money, PayCurrency};
    use sn_domain::order::OrderKind;
    use sn_domain::payment::ProviderId;
    use sn_domain::subscription::{SubscriptionKind, SubscriptionStatus};
    let client_id = ClientId(1);
    let subscription_id = SubscriptionId(2);
    let events = [
        DomainEvent::ClientRegistered {
            client_id,
            source: RegistrationSource::Bot,
            start_param: None,
        },
        DomainEvent::ClientBlocked {
            client_id,
            reason: None,
        },
        DomainEvent::ClientUnblocked { client_id },
        DomainEvent::PaymentPaid {
            payment_id: PaymentId(3),
            order_id: OrderId(4),
            client_id,
            order_kind: OrderKind::New,
            amount: Amount::new(699, PayCurrency::Rub),
            provider: ProviderId::from("yookassa"),
        },
        DomainEvent::PaymentRefunded {
            payment_id: PaymentId(3),
            order_id: OrderId(4),
            client_id,
            amount: Amount::new(699, PayCurrency::Rub),
        },
        DomainEvent::OrderFulfilled {
            order_id: OrderId(4),
            client_id,
            order_kind: OrderKind::New,
            subscription_id: Some(subscription_id),
            days: 30,
            devices: 1,
        },
        DomainEvent::ClientTelegramLinked {
            client_id,
            telegram_id: 5_093_118_422,
        },
        DomainEvent::SubscriptionCreated {
            subscription_id,
            client_id,
            kind: SubscriptionKind::Paid,
        },
        DomainEvent::SubscriptionRenewed {
            subscription_id,
            client_id,
            expire_at: Utc.with_ymd_and_hms(2026, 12, 18, 12, 0, 0).unwrap(),
        },
        DomainEvent::TariffChanged {
            subscription_id,
            client_id,
            from: None,
            to: TariffId(5),
        },
        DomainEvent::SubscriptionStatusChanged {
            subscription_id,
            client_id,
            from: None,
            to: SubscriptionStatus::Active,
        },
        DomainEvent::BalanceChanged {
            client_id,
            wallet: Wallet::Main,
            kind: LedgerKind::TopUp,
            delta: Money::new(100, Currency::Rub),
        },
        DomainEvent::TicketOpened {
            ticket_id: TicketId(6),
            client_id,
        },
        DomainEvent::TicketAnswered {
            ticket_id: TicketId(6),
            client_id,
        },
    ];
    assert_eq!(events.len(), manifest::EVENTS.len());
    for event in &events {
        assert_eq!(
            manifest::event_info(event.kind()).map(|info| info.needs),
            Some(event.permission()),
            "{}",
            event.kind()
        );
    }
}

#[test]
fn событие_не_отправляется_без_действующего_ключа_и_права() {
    use super::delivery::withheld;
    use super::store::{Access, KeyState};
    let full = Access {
        state: KeyState::Active,
        granted: vec![Permission::ClientsRead, Permission::Money],
    };
    assert_eq!(withheld(&full, "payment_paid"), None);
    assert_eq!(withheld(&full, "ping"), None);
    assert_eq!(withheld(&full, "settings_changed"), None);
    let narrowed = Access {
        state: KeyState::Active,
        granted: vec![Permission::ClientsRead],
    };
    assert_eq!(withheld(&narrowed, "client_registered"), None);
    assert!(
        withheld(&narrowed, "payment_paid")
            .unwrap()
            .contains("нет права «Деньги»")
    );
    for (state, part) in [
        (KeyState::Revoked, "отозван"),
        (KeyState::Missing, "отозван"),
        (KeyState::Expired, "истёк"),
        (KeyState::OwnerDisabled, "отключён"),
    ] {
        let access = Access {
            state,
            granted: full.granted.clone(),
        };
        let reason = withheld(&access, "client_registered").unwrap();
        assert!(reason.contains(part), "{reason}");
        assert_eq!(withheld(&access, "ping"), None);
    }
}

#[test]
fn свежая_проверка_помощника_считается_ответом_модуля() {
    use super::packages::{Health, overall};
    use super::runtime::{ContainerView, JobView, StepView};
    let now = Utc.with_ymd_and_hms(2026, 11, 24, 12, 0, 0).unwrap();
    let container = ContainerView {
        state: "running".into(),
        started_at: None,
        restarts: None,
        image: None,
        memory_bytes: None,
        data_bytes: None,
        checked_at: None,
    };
    let job = |ago: i64, verify: &str| JobView {
        id: "r1".into(),
        action: "module-install".into(),
        state: "done".into(),
        steps: vec![StepView {
            key: "verify".into(),
            state: verify.into(),
            seconds: Some(1),
        }],
        error: None,
        restored: None,
        requested_at: Some(now - Duration::seconds(ago + 5)),
        finished_at: Some(now - Duration::seconds(ago)),
    };
    let health = |ago: i64, ok: bool| Health {
        ok,
        at: now - Duration::seconds(ago),
        error: (!ok).then(|| "модуль ответил кодом 500".to_string()),
        since: None,
    };
    let fresh = job(10, "done");
    assert_eq!(
        overall(true, Some(&fresh), Some(&container), None, now),
        "running"
    );
    assert_eq!(
        overall(true, Some(&job(600, "done")), Some(&container), None, now),
        "starting"
    );
    assert_eq!(
        overall(true, Some(&job(10, "skipped")), Some(&container), None, now),
        "starting"
    );
    assert_eq!(
        overall(
            true,
            Some(&fresh),
            Some(&container),
            Some(&health(5, false)),
            now
        ),
        "unhealthy"
    );
    assert_eq!(
        overall(
            true,
            Some(&fresh),
            Some(&container),
            Some(&health(60, false)),
            now
        ),
        "running"
    );
    assert_eq!(
        overall(true, None, Some(&container), Some(&health(60, true)), now),
        "running"
    );
    assert_eq!(
        overall(false, Some(&fresh), Some(&container), None, now),
        "stopped"
    );
    let rolled = |restored: Option<bool>| JobView {
        state: "rolled_back".into(),
        action: "module-update".into(),
        restored,
        ..job(10, "failed")
    };
    let sick = health(30, false);
    assert_eq!(
        overall(
            true,
            Some(&rolled(Some(true))),
            Some(&container),
            Some(&sick),
            now
        ),
        "running"
    );
    assert_eq!(
        overall(
            true,
            Some(&rolled(Some(false))),
            Some(&container),
            Some(&sick),
            now
        ),
        "unhealthy"
    );
    assert_eq!(
        overall(true, Some(&rolled(None)), Some(&container), None, now),
        "starting"
    );
}
