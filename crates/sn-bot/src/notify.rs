use crate::BotConfig;
use crate::gram_wallet;
use crate::telegram::{Button, Keyboard, TelegramApi, TelegramError};
use crate::text::{self, escape};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use sn_domain::bot_content::Content;
use sn_domain::bot_content::Value::{self, Html, Text};
use sn_domain::client::{IdentityKind, Locale};
use sn_domain::ids::ClientId;
use sn_domain::jobs::{JobPayload, retry_delay};
use sn_domain::lang;
use sn_domain::mail_letters::{LetterId, Values};
use sn_domain::money::{Money, Stars};
use sn_domain::settings::CoreSettings;
use sn_modules::catalog::{contests, gifts, limits, oauth, referral};
use sn_services::Services;
use sn_services::mail::Undelivered;
use sn_store::{Db, StoreError};
use sn_worker::{CoreHandlers, JobHandler, JobResult};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub mod templates {
    pub const TEXT: &str = "text";
    pub const PAYMENT_PAID: &str = "payment_paid";
    pub const TOPUP_DONE: &str = sn_services::inbox::TOPUP_TEMPLATE;
    pub const SUBSCRIPTION_READY: &str = "subscription_ready";
    pub const SUBSCRIPTION_RENEWED: &str = "subscription_renewed";
    pub const SUBSCRIPTION_EXPIRING: &str = "subscription_expiring";
    pub const AUTORENEW_UPCOMING: &str = "autorenew_upcoming";
    pub const AUTORENEW_FAILED: &str = "autorenew_failed";
    pub const AUTORENEW_DISABLED: &str = "autorenew_disabled";
    pub const SUPPORT_REPLY: &str = "support_reply";
    pub const ORDER_FULFILLED: &str = "order.fulfilled";
    pub const AUTORENEW_REMINDER: &str = "autorenew.reminder";
    pub const AUTORENEW_CHARGE_FAILED: &str = "autorenew.failed";
    pub const AUTORENEW_TURNED_OFF: &str = "autorenew.disabled";
    pub const REFERRAL_CREDIT: &str = sn_modules::catalog::referral::TEMPLATE_CREDIT;
    pub const WITHDRAWAL_PAID: &str = sn_modules::catalog::referral::TEMPLATE_WITHDRAWAL_PAID;
    pub const WITHDRAWAL_REJECTED: &str =
        sn_modules::catalog::referral::TEMPLATE_WITHDRAWAL_REJECTED;
    pub const REPEAT_CREDITED: &str = sn_services::inbox::REPEAT_CREDITED_TEMPLATE;
    pub const REPEAT_REVIEW: &str = sn_services::inbox::REPEAT_REVIEW_TEMPLATE;
    pub const REFUNDED_BLOCKED: &str = sn_services::inbox::BLOCKED_REFUND_TEMPLATE;
    pub const STARS_REFUNDED: &str = sn_services::stars_ledger::REFUNDED_TEMPLATE;
    pub const AUTORENEW_AFTER_OFF: &str = sn_services::autorenew::CHARGED_AFTER_OFF_TEMPLATE;
    pub const PAYMENT_CREDITED: &str = sn_services::inbox::PAYMENT_CREDITED_TEMPLATE;
    pub const PAYMENT_REVIEW: &str = sn_services::inbox::PAYMENT_REVIEW_TEMPLATE;
    pub const SUBSCRIPTION_LINK: &str = sn_services::remna_transfer::LINK_TEMPLATE;
    pub const EMAIL_SET: &str = sn_services::email_admin::TEMPLATE_EMAIL_SET;
    pub const EMAIL_REMOVED: &str = sn_services::email_admin::TEMPLATE_EMAIL_REMOVED;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub keyboard: Keyboard,
}

fn raw(params: &serde_json::Value, key: &str) -> Option<String> {
    match params.get(key)? {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        value @ serde_json::Value::Object(_) => serde_json::from_value::<Money>(value.clone())
            .ok()
            .map(text::money),
        _ => None,
    }
}

fn exact(params: &serde_json::Value, key: &str) -> Option<String> {
    serde_json::from_value::<Money>(params.get(key)?.clone())
        .ok()
        .map(text::money_exact)
}

fn param(params: &serde_json::Value, key: &str) -> Option<String> {
    raw(params, key).map(|v| escape(&v))
}

fn raw_text(params: &serde_json::Value) -> Option<String> {
    match lang::current() {
        Locale::En => raw(params, "text_en").or_else(|| raw(params, "text")),
        Locale::Ru => raw(params, "text"),
    }
}

fn text_param(params: &serde_json::Value) -> Option<String> {
    raw_text(params).map(|v| escape(&v))
}

fn raw_label(params: &serde_json::Value) -> Option<String> {
    match lang::current() {
        Locale::En => raw(params, "label_en").or_else(|| raw(params, "label")),
        Locale::Ru => raw(params, "label"),
    }
}

fn label_param(params: &serde_json::Value) -> Option<String> {
    raw_label(params).map(|v| escape(&v))
}

fn raw_stars(params: &serde_json::Value) -> Option<String> {
    let stars = params
        .get("stars")
        .and_then(serde_json::Value::as_i64)
        .filter(|stars| *stars > 0)?;
    Some(text::stars(Stars(stars)))
}

fn stars_param(params: &serde_json::Value) -> Option<String> {
    raw_stars(params).map(|v| escape(&v))
}

fn raw_term(params: &serde_json::Value) -> Option<String> {
    match params.get("days").and_then(serde_json::Value::as_u64) {
        Some(days) => Some(gifts::term(u32::try_from(days).unwrap_or(u32::MAX))),
        None => raw(params, "term"),
    }
}

fn term_param(params: &serde_json::Value) -> Option<String> {
    raw_term(params).map(|v| escape(&v))
}

fn raw_bytes(params: &serde_json::Value, bytes: &str, shown: &str) -> Option<String> {
    match params.get(bytes).and_then(serde_json::Value::as_u64) {
        Some(value) => Some(text::traffic(value)),
        None => raw(params, shown),
    }
}

fn bytes_param(params: &serde_json::Value, bytes: &str, shown: &str) -> Option<String> {
    raw_bytes(params, bytes, shown).map(|v| escape(&v))
}

fn limits_period(params: &serde_json::Value) -> Option<(String, String)> {
    let when = params.get("when")?.as_str()?.to_string();
    let per = params.get("per")?.as_str()?.to_string();
    if lang::current() == Locale::Ru {
        return Some((when, per));
    }
    Some(
        match params.get("period").and_then(serde_json::Value::as_str) {
            Some("week") => ("This week".into(), "per week".into()),
            _ => ("Today".into(), "per day".into()),
        },
    )
}

fn raw_until(params: &serde_json::Value, offset: FixedOffset) -> Option<String> {
    if lang::current() == Locale::En
        && let Some(at) = params
            .get("until_at")
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
    {
        let at = at.with_timezone(&Utc);
        let local = at.with_timezone(&offset);
        return Some(format!(
            "{}, {}",
            text::day_month(at, offset),
            local.format("%H:%M")
        ));
    }
    raw(params, "until")
}

fn until_param(params: &serde_json::Value, offset: FixedOffset) -> Option<String> {
    raw_until(params, offset).map(|v| escape(&v))
}

fn prize_param(params: &serde_json::Value) -> Option<String> {
    params
        .get("prize_data")
        .and_then(|raw| {
            serde_json::from_value::<sn_modules::catalog::prize::Prize>(raw.clone()).ok()
        })
        .map(|prize| escape(&prize.label()))
        .or_else(|| param(params, "prize"))
}

fn place_param(params: &serde_json::Value) -> Option<String> {
    if !params
        .get("tiered")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    params
        .get("place")
        .and_then(serde_json::Value::as_u64)
        .and_then(|place| u32::try_from(place).ok())
        .filter(|place| *place > 0)
        .map(contests::place_word)
}

fn note_param(params: &serde_json::Value) -> Option<String> {
    params
        .get("note")
        .and_then(serde_json::Value::as_str)
        .map(|note| escape(&contests::shown_note(note.trim())))
        .filter(|note| !note.is_empty())
}

fn canonical<'a>(template: &'a str, params: &serde_json::Value) -> &'a str {
    match template {
        templates::ORDER_FULFILLED => {
            match params.get("order_kind").and_then(serde_json::Value::as_str) {
                Some("renewal") => templates::SUBSCRIPTION_RENEWED,
                _ => templates::SUBSCRIPTION_READY,
            }
        }
        templates::AUTORENEW_REMINDER | templates::AUTORENEW_CHARGE_FAILED => {
            templates::AUTORENEW_FAILED
        }
        templates::AUTORENEW_TURNED_OFF => templates::AUTORENEW_DISABLED,
        other => other,
    }
}

fn date_param(params: &serde_json::Value, key: &str, offset: FixedOffset) -> Option<String> {
    let raw = params.get(key)?.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|at| text::date(at.with_timezone(&Utc), offset))
}

fn keyboard(content: &Content, button: Option<(&str, &str)>) -> Keyboard {
    let Some((key, data)) = button else {
        return Keyboard::new();
    };
    match content.button(key, &[]) {
        Some(label) => Keyboard::new().row(vec![
            Button::callback(label.text, data)
                .look(label.emoji_id, label.style.map(|s| s.as_str())),
        ]),
        None => Keyboard::new(),
    }
}

fn ticket_button(
    content: &Content,
    settings: &CoreSettings,
    params: &serde_json::Value,
) -> Option<Button> {
    let id = params
        .get("ticket_id")
        .and_then(serde_json::Value::as_i64)
        .filter(|id| *id > 0)?;
    let app = settings.support_app_url()?;
    let label = content.button("notify.btn.ticket", &[])?;
    Some(
        Button::web_app(label.text, format!("{app}/{id}"))
            .look(label.emoji_id, label.style.map(|s| s.as_str())),
    )
}

pub fn render(
    content: &Content,
    template: &str,
    params: &serde_json::Value,
    offset: FixedOffset,
) -> Option<Rendered> {
    let template = canonical(template, params);
    let amount = param(params, "amount");
    let date =
        date_param(params, "date", offset).or_else(|| date_param(params, "expire_at", offset));
    let t = |key: &str, vars: &[(&str, Value<'_>)]| content.text(key, vars);
    let reply = match params.get("ticket_id").and_then(serde_json::Value::as_i64) {
        Some(id) if id > 0 => format!("sup:{id}"),
        _ => "sup".to_string(),
    };
    let (text, button): (String, Option<(&str, &str)>) = match template {
        templates::PAYMENT_PAID => (
            match &amount {
                Some(amount) => t("notify.payment_paid", &[("amount", Html(amount))]),
                None => t("notify.payment_paid_plain", &[]),
            },
            Some(("notify.btn.subscription", "subs")),
        ),
        templates::TOPUP_DONE => (
            match (&amount, exact(params, "bonus"), exact(params, "total")) {
                (Some(amount), Some(bonus), Some(total)) => t(
                    "notify.topup_done_bonus",
                    &[
                        ("amount", Html(amount)),
                        ("bonus", Text(&bonus)),
                        ("total", Text(&total)),
                    ],
                ),
                (Some(amount), ..) => t("notify.topup_done", &[("amount", Html(amount))]),
                (None, ..) => t("notify.topup_done_plain", &[]),
            },
            Some(("notify.btn.balance", "bal")),
        ),
        templates::SUBSCRIPTION_READY => (
            match &date {
                Some(date) => t("notify.subscription_ready", &[("date", Text(date))]),
                None => t("notify.subscription_ready_plain", &[]),
            },
            Some(("notify.btn.subscription", "subs")),
        ),
        templates::SUBSCRIPTION_RENEWED => (
            match &date {
                Some(date) => t("notify.subscription_renewed", &[("date", Text(date))]),
                None => t("notify.subscription_renewed_plain", &[]),
            },
            Some(("notify.btn.subscription", "subs")),
        ),
        templates::SUBSCRIPTION_EXPIRING => (
            match &date {
                Some(date) => t("notify.subscription_expiring", &[("date", Text(date))]),
                None => t("notify.subscription_expiring_plain", &[]),
            },
            Some(("notify.btn.extend", "buy")),
        ),
        templates::AUTORENEW_UPCOMING => (
            match (&date, &amount) {
                (Some(date), Some(amount)) => t(
                    "notify.autorenew_upcoming",
                    &[("date", Text(date)), ("amount", Html(amount))],
                ),
                _ => t("notify.autorenew_upcoming_plain", &[]),
            },
            Some(("notify.btn.balance", "bal")),
        ),
        templates::AUTORENEW_FAILED if params["balance_off"].as_bool() == Some(true) => (
            match &date {
                Some(date) => t("notify.autorenew_pay", &[("date", Text(date))]),
                None => t("notify.autorenew_pay_plain", &[]),
            },
            Some(("notify.btn.extend", "buy")),
        ),
        templates::AUTORENEW_FAILED => (
            match param(params, "missing") {
                Some(missing) => t("notify.autorenew_failed", &[("missing", Html(&missing))]),
                None => t("notify.autorenew_failed_plain", &[]),
            },
            Some(("notify.btn.balance", "bal")),
        ),
        templates::AUTORENEW_DISABLED => (
            t("notify.autorenew_disabled", &[]),
            Some(("notify.btn.extend", "buy")),
        ),
        templates::AUTORENEW_AFTER_OFF => (
            match (&amount, &date) {
                (Some(amount), Some(date)) => t(
                    "notify.autorenew_after_off",
                    &[("amount", Html(amount)), ("date", Text(date))],
                ),
                _ => t("notify.autorenew_after_off_plain", &[]),
            },
            Some(("notify.btn.support", "sup")),
        ),
        templates::REFUNDED_BLOCKED => (
            match &amount {
                Some(amount) => t(
                    "notify.payment_refunded_blocked",
                    &[("amount", Html(amount))],
                ),
                None => t("notify.payment_refunded_blocked_plain", &[]),
            },
            None,
        ),
        templates::STARS_REFUNDED => {
            let stars = stars_param(params)?;
            (
                match label_param(params) {
                    Some(label) => t(
                        "notify.stars_refunded",
                        &[("label", Html(&label)), ("stars", Html(&stars))],
                    ),
                    None => t("notify.stars_refunded_plain", &[("stars", Html(&stars))]),
                },
                None,
            )
        }
        templates::SUBSCRIPTION_LINK => (
            t("sub.reissued", &[("url", Text(&raw(params, "url")?))]),
            Some(("notify.btn.subscription", "subs")),
        ),
        templates::SUPPORT_REPLY => {
            let body = match param(params, "body") {
                Some(body) => body,
                None if !crate::support_files::reply_files(params).is_empty() => String::new(),
                None => return None,
            };
            (
                t("notify.support_reply", &[("body", Html(&body))])
                    .trim_end()
                    .to_string(),
                Some(("notify.btn.reply", reply.as_str())),
            )
        }
        templates::REPEAT_CREDITED | templates::PAYMENT_CREDITED => {
            (text_param(params)?, Some(("notify.btn.balance", "bal")))
        }
        templates::REPEAT_REVIEW | templates::PAYMENT_REVIEW => {
            (text_param(params)?, Some(("notify.btn.support", "sup")))
        }
        gifts::TEMPLATE_READY => {
            let tariff = param(params, "tariff")?;
            let term = term_param(params)?;
            let code = param(params, "code")?;
            let mut lines = vec![t(
                "notify.gift_ready",
                &[
                    ("tariff", Html(&tariff)),
                    ("term", Html(&term)),
                    ("code", Html(&code)),
                ],
            )];
            if let Some(link) = param(params, "bot_link") {
                lines.push(t("notify.gift_ready_link", &[("link", Html(&link))]));
            }
            if let Some(url) = param(params, "cabinet_link") {
                lines.push(t("notify.gift_ready_cabinet", &[("url", Html(&url))]));
            }
            if let Some(until) = date_param(params, "expire_at", offset) {
                lines.push(t("notify.gift_ready_until", &[("date", Text(&until))]));
            }
            (
                lines
                    .into_iter()
                    .filter(|line| !line.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(("notify.btn.gifts", "gl")),
            )
        }
        gifts::TEMPLATE_REDEEMED => (
            t(
                "notify.gift_redeemed",
                &[
                    ("code", Html(&param(params, "code")?)),
                    ("tariff", Html(&param(params, "tariff")?)),
                    ("term", Html(&term_param(params)?)),
                ],
            ),
            Some(("notify.btn.gifts", "gl")),
        ),
        referral::TEMPLATE_CREDIT => (
            t(
                "notify.referral_credit",
                &[("amount", Html(&amount.clone()?))],
            ),
            Some(("notify.btn.referral", "x:referral")),
        ),
        limits::TEMPLATE_WARNING => {
            let (when, per) = limits_period(params)?;
            (
                t(
                    "notify.traffic_warning",
                    &[
                        ("when", Text(&when)),
                        ("used", Html(&bytes_param(params, "used_bytes", "used")?)),
                        ("limit", Html(&bytes_param(params, "limit_bytes", "limit")?)),
                        ("per", Text(&per)),
                    ],
                ),
                Some(("notify.btn.subscription", "subs")),
            )
        }
        limits::TEMPLATE_RESTRICTED => {
            let (when, per) = limits_period(params)?;
            (
                t(
                    "notify.traffic_restricted",
                    &[
                        ("when", Text(&when)),
                        ("used", Html(&bytes_param(params, "used_bytes", "used")?)),
                        ("limit", Html(&bytes_param(params, "limit_bytes", "limit")?)),
                        ("per", Text(&per)),
                        ("until", Html(&until_param(params, offset)?)),
                    ],
                ),
                Some(("notify.btn.subscription", "subs")),
            )
        }
        limits::TEMPLATE_LIFTED => (
            t("notify.traffic_lifted", &[]),
            Some(("notify.btn.subscription", "subs")),
        ),
        oauth::TEMPLATE_UNLINKED => (
            t("notify.oauth_unlinked", &[]),
            Some(("notify.btn.support", "sup")),
        ),
        oauth::TEMPLATE_PASSWORD_ISSUED => (
            t("notify.password_issued", &[]),
            Some(("notify.btn.support", "sup")),
        ),
        templates::EMAIL_SET => (
            t(
                "notify.email_set",
                &[("email", Text(params.get("email")?.as_str()?))],
            ),
            Some(("notify.btn.email", "em")),
        ),
        templates::EMAIL_REMOVED => (
            t(
                "notify.email_removed",
                &[("email", Text(params.get("email")?.as_str()?))],
            ),
            Some(("notify.btn.support", "sup")),
        ),
        oauth::TEMPLATE_LINKED => (
            t(
                "notify.oauth_linked",
                &[
                    ("provider", Text(params.get("provider")?.as_str()?)),
                    ("account", Text(params.get("account")?.as_str()?)),
                ],
            ),
            Some(("notify.btn.support", "sup")),
        ),
        contests::TEMPLATE_WON => {
            let title = param(params, "title")?;
            let prize = prize_param(params)?;
            let result = note_param(params).unwrap_or_default();
            let mut text = match place_param(params) {
                Some(place) => t(
                    "notify.contest_won_place",
                    &[
                        ("place", Html(&place)),
                        ("title", Html(&title)),
                        ("prize", Html(&prize)),
                        ("result", Html(&result)),
                    ],
                ),
                None => t(
                    "notify.contest_won",
                    &[
                        ("title", Html(&title)),
                        ("prize", Html(&prize)),
                        ("result", Html(&result)),
                    ],
                ),
            };
            if let Some(code) = param(params, "code") {
                text.push('\n');
                text.push_str(&t("notify.contest_won_code", &[("code", Html(&code))]));
            }
            (text, Some(("notify.btn.contests", "x:contests")))
        }
        contests::TEMPLATE_WAITING => {
            let title = param(params, "title")?;
            let prize = prize_param(params)?;
            let text = match place_param(params) {
                Some(place) => t(
                    "notify.contest_waiting_place",
                    &[
                        ("place", Html(&place)),
                        ("title", Html(&title)),
                        ("prize", Html(&prize)),
                    ],
                ),
                None => t(
                    "notify.contest_waiting",
                    &[("title", Html(&title)), ("prize", Html(&prize))],
                ),
            };
            (text, Some(("notify.btn.contests", "x:contests")))
        }
        referral::TEMPLATE_WITHDRAWAL_PAID => (
            t(
                "notify.withdrawal_paid",
                &[("amount", Html(&amount.clone()?))],
            ),
            None,
        ),
        referral::TEMPLATE_WITHDRAWAL_REJECTED => (
            match param(params, "comment") {
                Some(comment) => t(
                    "notify.withdrawal_rejected",
                    &[
                        ("amount", Html(&amount.clone()?)),
                        ("comment", Html(&comment)),
                    ],
                ),
                None => t(
                    "notify.withdrawal_rejected_plain",
                    &[("amount", Html(&amount.clone()?))],
                ),
            },
            Some(("notify.btn.referral", "x:referral")),
        ),
        _ => (param(params, "text")?, None),
    };
    Some(Rendered {
        text,
        keyboard: keyboard(content, button),
    })
}

pub struct NotifyHandler {
    api_base: String,
    send: crate::SendLimits,
    cache: Mutex<Option<Cached>>,
}

type Cached = (String, crate::telegram::Proxy, Arc<TelegramApi>);

fn waiting() -> JobResult {
    JobResult::Wait {
        after: sn_services::egress::WAIT,
        error: "прокси не отвечает, отправим, когда он заработает".into(),
    }
}

impl Default for NotifyHandler {
    fn default() -> Self {
        Self::new(&BotConfig::default())
    }
}

impl NotifyHandler {
    pub fn new(config: &BotConfig) -> Self {
        NotifyHandler {
            api_base: config.api_base.clone(),
            send: config.send,
            cache: Mutex::new(None),
        }
    }

    fn api(&self, token: &str, proxy: &crate::telegram::Proxy) -> Arc<TelegramApi> {
        let mut cache = match self.cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((cached, via, api)) = cache.as_ref()
            && cached == token
            && via == proxy
        {
            return api.clone();
        }
        let api =
            Arc::new(TelegramApi::paced(&self.api_base, token, self.send, proxy.clone()).bulk());
        *cache = Some((token.to_string(), proxy.clone(), api.clone()));
        api
    }

    pub async fn deliver(
        &self,
        services: &Services,
        client_id: ClientId,
        template: &str,
        params: &serde_json::Value,
        attempt: u32,
    ) -> JobResult {
        let retry = |error: String| JobResult::Retry {
            after: retry_delay(attempt),
            error,
        };
        let mut conn = match services.store().conn().await {
            Ok(conn) => conn,
            Err(err) => return retry(err.to_string()),
        };
        let client = match conn.clients().get(client_id).await {
            Ok(client) => client,
            Err(StoreError::NotFound(_)) => return JobResult::Fail("клиент не найден".into()),
            Err(err) => return retry(err.to_string()),
        };
        let identities = match conn.clients().identities(client_id).await {
            Ok(identities) => identities,
            Err(err) => return retry(err.to_string()),
        };
        drop(conn);
        let locale = services.locale_of(client.locale);
        lang::scope(
            locale,
            self.deliver_to(services, &client, &identities, template, params, attempt),
        )
        .await
    }

    async fn deliver_to(
        &self,
        services: &Services,
        client: &sn_domain::client::Client,
        identities: &[sn_domain::client::Identity],
        template: &str,
        params: &serde_json::Value,
        attempt: u32,
    ) -> JobResult {
        let client_id = client.id;
        let retry = |error: String| JobResult::Retry {
            after: retry_delay(attempt),
            error,
        };
        let chat = identities
            .iter()
            .find(|i| i.kind == IdentityKind::Telegram)
            .and_then(|i| i.value.parse::<i64>().ok())
            .filter(|_| !client.bot_unreachable);
        let Some(chat_id) = chat else {
            return match identities.iter().find(|i| i.takes_letters()) {
                Some(email) => mail_notice(services, &email.value, template, params, attempt).await,
                None => {
                    tracing::debug!(client = %client_id, template, "клиенту не написать ни в Telegram, ни на почту");
                    JobResult::Done
                }
            };
        };
        let Some(token) = services.bot_token() else {
            return retry("токен бота не задан".into());
        };
        let proxy = services
            .egress()
            .proxy_for(sn_egress::Route::Telegram, None);
        let sent = if template == gram_wallet::TEMPLATE {
            let Some(payment) = params
                .get("payment_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
            else {
                return JobResult::Fail("в задании нет счёта".into());
            };
            let widget = match gram_wallet::compose_for(services, payment).await {
                Ok(Some(widget)) => widget,
                Ok(None) => return JobResult::Done,
                Err(err) => return retry(err.to_string()),
            };
            self.api(&token, &proxy)
                .send_widget(chat_id, &widget)
                .await
                .map(|_| ())
        } else {
            let content = services.bot_content_in(lang::current());
            let settings = services.settings();
            let Some(mut rendered) =
                render(&content.content, template, params, settings.remna_offset())
            else {
                return JobResult::Fail(format!("уведомление «{template}» без нужных параметров"));
            };
            if template == templates::SUPPORT_REPLY
                && !client.is_blocked()
                && let Some(button) = ticket_button(&content.content, &settings, params)
            {
                rendered.keyboard = Keyboard::new()
                    .row(vec![button])
                    .rows(rendered.keyboard.inline_keyboard);
            }
            let files = if template == templates::SUPPORT_REPLY {
                crate::support_files::reply_files(params)
            } else {
                Vec::new()
            };
            if files.is_empty() {
                self.api(&token, &proxy)
                    .send_message(chat_id, &rendered.text, Some(&rendered.keyboard))
                    .await
                    .map(|_| ())
            } else {
                crate::support_files::send_reply(
                    services,
                    &self.api(&token, &proxy),
                    chat_id,
                    &rendered.text,
                    &rendered.keyboard,
                    &files,
                )
                .await
            }
        };
        match sent {
            Ok(()) => {
                also_mail(services, client_id, template, params, attempt).await;
                JobResult::Done
            }
            Err(err) if err.is_forbidden() || chat_missing(&err) => {
                match services.store().conn().await {
                    Ok(mut conn) => {
                        if let Err(store) =
                            conn.clients().set_bot_unreachable(client_id, true).await
                        {
                            return retry(store.to_string());
                        }
                    }
                    Err(store) => return retry(store.to_string()),
                }
                tracing::info!(client = %client_id, "клиент закрыл бота, уведомления остановлены");
                JobResult::Done
            }
            Err(err) => match err.retry_after() {
                Some(seconds) => JobResult::Retry {
                    after: chrono::Duration::seconds(i64::try_from(seconds).unwrap_or(60)),
                    error: err.to_string(),
                },
                None if matches!(err, TelegramError::Transport(_))
                    && services.egress().down_for(sn_egress::Route::Telegram) =>
                {
                    waiting()
                }
                None if err.is_retryable() => retry(err.to_string()),
                None => JobResult::Fail(err.to_string()),
            },
        }
    }
}

fn reply_with_files(params: &serde_json::Value) -> Option<String> {
    let names: Vec<String> = params
        .get("files")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| f.get("name").and_then(serde_json::Value::as_str))
        .map(|name| match lang::current() {
            Locale::Ru => format!("[файл: {name}]"),
            _ => format!("[file: {name}]"),
        })
        .collect();
    let body = raw(params, "body").filter(|b| !b.trim().is_empty());
    if names.is_empty() {
        return body;
    }
    Some(body.into_iter().chain(names).collect::<Vec<_>>().join("\n"))
}

pub fn letter_of(
    template: &str,
    params: &serde_json::Value,
    offset: FixedOffset,
) -> Option<(LetterId, Values)> {
    let date =
        date_param(params, "date", offset).or_else(|| date_param(params, "expire_at", offset));
    let balance_off = params
        .get("balance_off")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let amount = || raw(params, "amount");
    Some(match template {
        templates::ORDER_FULFILLED
        | templates::SUBSCRIPTION_READY
        | templates::SUBSCRIPTION_RENEWED => {
            let id = if canonical(template, params) == templates::SUBSCRIPTION_RENEWED {
                LetterId::SubscriptionRenewed
            } else {
                LetterId::SubscriptionReady
            };
            (
                id,
                Values::new()
                    .maybe("date", date)
                    .maybe("tariff", raw(params, "tariff")),
            )
        }
        templates::TOPUP_DONE => (
            LetterId::TopupDone,
            Values::new()
                .maybe("amount", amount())
                .maybe("bonus", exact(params, "bonus"))
                .maybe("total", exact(params, "total")),
        ),
        templates::PAYMENT_CREDITED | templates::REPEAT_CREDITED => (
            LetterId::PaymentCredited,
            Values::new()
                .maybe("amount", amount())
                .maybe("details", raw_text(params)),
        ),
        templates::PAYMENT_REVIEW | templates::REPEAT_REVIEW => (
            LetterId::PaymentReview,
            Values::new()
                .maybe("amount", amount())
                .maybe("details", raw_text(params)),
        ),
        templates::REFUNDED_BLOCKED => (
            LetterId::PaymentRefunded,
            Values::new().maybe("amount", amount()),
        ),
        templates::STARS_REFUNDED => (
            LetterId::StarsRefunded,
            Values::new()
                .maybe("stars", raw_stars(params))
                .maybe("purchase", raw_label(params)),
        ),
        templates::AUTORENEW_AFTER_OFF => (
            LetterId::ChargedAfterOff,
            Values::new().maybe("amount", amount()).maybe("date", date),
        ),
        gifts::TEMPLATE_READY => (
            LetterId::GiftReady,
            Values::new()
                .maybe("tariff", raw(params, "tariff"))
                .maybe("term", raw_term(params))
                .maybe("code", raw(params, "code"))
                .maybe("link", raw(params, "bot_link"))
                .maybe("url", raw(params, "cabinet_link"))
                .maybe("date", date_param(params, "expire_at", offset)),
        ),
        gifts::TEMPLATE_REDEEMED => (
            LetterId::GiftRedeemed,
            Values::new()
                .maybe("code", raw(params, "code"))
                .maybe("tariff", raw(params, "tariff"))
                .maybe("term", raw_term(params)),
        ),
        templates::AUTORENEW_REMINDER if balance_off => {
            (LetterId::RenewalManual, Values::new().maybe("date", date))
        }
        templates::AUTORENEW_REMINDER => (
            LetterId::RenewalShort,
            Values::new()
                .maybe("date", date)
                .maybe("missing", raw(params, "missing"))
                .maybe("amount", amount()),
        ),
        templates::AUTORENEW_CHARGE_FAILED | templates::AUTORENEW_FAILED if balance_off => {
            (LetterId::RenewalManual, Values::new().maybe("date", date))
        }
        templates::AUTORENEW_CHARGE_FAILED | templates::AUTORENEW_FAILED => (
            LetterId::RenewalFailed,
            Values::new().maybe("date", date).maybe("amount", amount()),
        ),
        templates::AUTORENEW_TURNED_OFF | templates::AUTORENEW_DISABLED => {
            (LetterId::RenewalOff, Values::new().maybe("date", date))
        }
        templates::SUPPORT_REPLY => (
            LetterId::SupportReply,
            Values::new()
                .maybe(
                    "ticket",
                    params
                        .get("ticket_id")
                        .and_then(serde_json::Value::as_i64)
                        .filter(|id| *id > 0)
                        .map(|id| id.to_string()),
                )
                .maybe("reply", reply_with_files(params)),
        ),
        oauth::TEMPLATE_UNLINKED => (LetterId::LoginsUnlinked, Values::new()),
        oauth::TEMPLATE_PASSWORD_ISSUED => (LetterId::PasswordIssued, Values::new()),
        oauth::TEMPLATE_LINKED => (
            LetterId::LoginLinked,
            Values::new()
                .maybe("provider", raw(params, "provider"))
                .maybe("account", raw(params, "account")),
        ),
        limits::TEMPLATE_WARNING | limits::TEMPLATE_RESTRICTED => {
            let (when, per) = limits_period(params)?;
            let values = Values::new()
                .with("when", when)
                .with("per", per)
                .maybe("used", raw_bytes(params, "used_bytes", "used"))
                .maybe("limit", raw_bytes(params, "limit_bytes", "limit"));
            if template == limits::TEMPLATE_RESTRICTED {
                (
                    LetterId::TrafficRestricted,
                    values.maybe("until", raw_until(params, offset)),
                )
            } else {
                (LetterId::TrafficWarning, values)
            }
        }
        limits::TEMPLATE_LIFTED => (LetterId::TrafficLifted, Values::new()),
        templates::SUBSCRIPTION_LINK => (
            LetterId::SubscriptionLink,
            Values::new().maybe("url", raw(params, "url")),
        ),
        _ => return None,
    })
}

async fn mail_notice(
    services: &Services,
    email: &str,
    template: &str,
    params: &serde_json::Value,
    attempt: u32,
) -> JobResult {
    let mail = services.mail();
    let Some(letters) = mail.letters() else {
        return JobResult::Done;
    };
    let Some((id, values)) = letter_of(template, params, services.settings().remna_offset()) else {
        return JobResult::Done;
    };
    if !letters.allows(id.spec().group) {
        return JobResult::Done;
    }
    let letter = match services.letters().letter(id, email, values).await {
        Ok(Some(letter)) => letter,
        Ok(None) => {
            return JobResult::Fail(format!(
                "письмо «{}» по уведомлению «{template}» без нужных данных",
                id.spec().title
            ));
        }
        Err(err) => {
            return JobResult::Retry {
                after: retry_delay(attempt),
                error: err.to_string(),
            };
        }
    };
    match mail.deliver(&letter).await {
        Ok(()) => JobResult::Done,
        Err(Undelivered::Off(_) | Undelivered::Failed(_))
            if services.egress().down_for(sn_egress::Route::Mail) && mail.via_resend() =>
        {
            waiting()
        }
        Err(Undelivered::Off(_)) => JobResult::Done,
        Err(Undelivered::Address(reason)) => {
            tracing::info!(template, reason = %reason, "письмо клиенту не отправлено: адрес не принят");
            JobResult::Done
        }
        Err(Undelivered::Limited(wait)) => JobResult::Retry {
            after: chrono::Duration::from_std(wait)
                .unwrap_or_else(|_| chrono::Duration::seconds(60)),
            error: "почтовый сервис просит подождать".into(),
        },
        Err(Undelivered::Failed(error)) => JobResult::Retry {
            after: retry_delay(attempt),
            error,
        },
    }
}

async fn also_mail(
    services: &Services,
    client_id: ClientId,
    template: &str,
    params: &serde_json::Value,
    attempt: u32,
) {
    if params.get("also_mail").and_then(serde_json::Value::as_bool) != Some(true) {
        return;
    }
    let email = match services.store().conn().await {
        Ok(mut conn) => match conn.clients().identities(client_id).await {
            Ok(identities) => identities
                .into_iter()
                .find(|i| i.takes_letters())
                .map(|i| i.value),
            Err(err) => {
                tracing::warn!(client = %client_id, error = %err, "почта клиента для копии уведомления не прочитана");
                None
            }
        },
        Err(err) => {
            tracing::warn!(client = %client_id, error = %err, "почта клиента для копии уведомления не прочитана");
            None
        }
    };
    let Some(email) = email else {
        return;
    };
    if let JobResult::Retry { error, .. } | JobResult::Fail(error) =
        mail_notice(services, &email, template, params, attempt).await
    {
        tracing::warn!(client = %client_id, template, error = %error, "копия уведомления на почту не отправлена");
    }
}

fn chat_missing(err: &TelegramError) -> bool {
    matches!(err, TelegramError::Api { code: 400, description, .. } if description.contains("chat not found"))
}

#[async_trait]
impl JobHandler for NotifyHandler {
    async fn handle(&self, services: &Services, payload: &JobPayload, attempt: u32) -> JobResult {
        match payload {
            JobPayload::Notify {
                client_id,
                template,
                params,
            } => {
                self.deliver(services, *client_id, template, params, attempt)
                    .await
            }
            other => CoreHandlers.handle(services, other, attempt).await,
        }
    }
}
