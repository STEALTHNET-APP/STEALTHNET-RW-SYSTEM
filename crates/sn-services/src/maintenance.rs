use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sn_domain::client::Locale;
use sn_domain::ids::AdminId;
use sn_domain::maintenance::{
    MESSAGE_MAX, Maintenance, NOTICE_EVERY_MINUTES, OFFSET_MAX_MINUTES, Texts, clean_message,
    standard_texts, too_long,
};
use sn_domain::settings::keys;
use sn_store::Db;

pub const CODE: &str = "maintenance";
pub const INVALID: &str = "maintenance.invalid";
pub const TELEGRAM_INVALID: &str = "admins.telegram_invalid";
pub const TELEGRAM_MAX: i64 = 1 << 52;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MaintenanceView {
    pub settings: Maintenance,
    pub standard: Texts,
    pub message_max: usize,
    pub notice_minutes: i32,
    pub english: bool,
    pub telegram_id: Option<i64>,
    pub testers: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MaintenanceUpdate {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub message_ru: Option<String>,
    #[serde(default)]
    pub message_en: Option<String>,
    #[serde(default)]
    pub until: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct TelegramUpdate {
    #[serde(default)]
    pub telegram_id: Option<i64>,
}

fn snapshot(mode: &Maintenance) -> Value {
    json!({
        "message_ru": mode.message_ru,
        "message_en": mode.message_en,
        "until": mode.until_label(),
    })
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::rejected(INVALID, message)
}

fn checked_message(text: Option<&str>, language: &str) -> ServiceResult<Option<String>> {
    let text = clean_message(text);
    if text.as_deref().is_some_and(too_long) {
        return Err(invalid(format!(
            "Сообщение {language} длиннее {MESSAGE_MAX} символов"
        )));
    }
    Ok(text)
}

pub struct MaintenanceOps<'a> {
    services: &'a Services,
}

impl<'a> MaintenanceOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        MaintenanceOps { services }
    }

    pub fn current(&self) -> Maintenance {
        self.services.settings().maintenance.clone()
    }

    pub fn active(&self) -> bool {
        self.services.settings().maintenance.enabled
    }

    pub fn text(&self, locale: Locale) -> String {
        self.services
            .settings()
            .maintenance
            .text(locale, self.services.now())
    }

    pub fn public(&self) -> Option<Value> {
        let settings = self.services.settings();
        let mode = &settings.maintenance;
        mode.enabled.then(|| {
            json!({
                "message": mode.messages(),
                "until": mode.ahead(self.services.now()),
            })
        })
    }

    pub async fn view(&self, admin: AdminId) -> ServiceResult<MaintenanceView> {
        let mut conn = self.services.store().conn().await?;
        let telegram_id = conn.admins().get(admin).await?.telegram_id;
        let testers = conn.admins().count_telegram_besides(admin).await?;
        let settings = self.services.settings();
        Ok(MaintenanceView {
            settings: settings.maintenance.clone(),
            standard: standard_texts(),
            message_max: MESSAGE_MAX,
            notice_minutes: NOTICE_EVERY_MINUTES,
            english: settings.languages.several(),
            telegram_id,
            testers,
        })
    }

    pub async fn update(
        &self,
        update: &MaintenanceUpdate,
        admin: AdminId,
    ) -> ServiceResult<MaintenanceView> {
        let current = self.current();
        let now = self.services.now();
        let message_ru = checked_message(update.message_ru.as_deref(), "на русском")?;
        let message_en = checked_message(update.message_en.as_deref(), "на английском")?;
        let until = update.until.map(|until| until.with_timezone(&Utc));
        let offset_minutes = match update.until {
            Some(until) => until.offset().local_minus_utc() / 60,
            None => 0,
        };
        if offset_minutes.abs() > OFFSET_MAX_MINUTES {
            return Err(invalid("Часовой пояс времени окончания не распознан"));
        }
        if until != current.until && until.is_some_and(|until| until <= now) {
            return Err(invalid(
                "Время окончания уже прошло. Укажите время в будущем или очистите поле",
            ));
        }
        let since = match (current.enabled, update.enabled) {
            (false, true) => Some(now),
            (true, true) => current.since.or(Some(now)),
            (_, false) => None,
        };
        let next = Maintenance {
            enabled: update.enabled,
            message_ru,
            message_en,
            until,
            offset_minutes: if until.is_some() { offset_minutes } else { 0 },
            since,
        };
        if next == current {
            return self.view(admin).await;
        }
        let details = json!({ "from": snapshot(&current), "to": snapshot(&next) });
        let action = match (current.enabled, next.enabled) {
            (false, true) => "settings.maintenance_on",
            (true, false) => "settings.maintenance_off",
            _ => "settings.maintenance",
        };
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::MAINTENANCE, &next, Some(admin))
            .await?;
        if current.enabled != next.enabled {
            sqlx::query("DELETE FROM bot_maintenance_notices")
                .execute(tx.conn())
                .await?;
        }
        tx.audit()
            .record(&Actor::Admin(admin).audit(action, "settings", None, details))
            .await?;
        tx.commit().await?;
        self.services.reload().await?;
        self.services.notify_settings_changed().await?;
        self.view(admin).await
    }

    pub async fn set_telegram(
        &self,
        admin: AdminId,
        update: &TelegramUpdate,
    ) -> ServiceResult<Option<i64>> {
        let next = update.telegram_id;
        if next.is_some_and(|id| id <= 0 || id >= TELEGRAM_MAX) {
            return Err(ServiceError::rejected(
                TELEGRAM_INVALID,
                "Telegram ID — это число из профиля Telegram, например 123456789",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let before = tx.admins().get(admin).await?.telegram_id;
        if before == next {
            tx.commit().await?;
            return Ok(next);
        }
        tx.admins().set_telegram(admin, next).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "admin.telegram",
                "admin",
                Some(admin.0),
                json!({ "from": { "telegram_id": before }, "to": { "telegram_id": next } }),
            ))
            .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn lets_through(&self, telegram_id: i64) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.admins().is_active_telegram(telegram_id).await?)
    }

    pub async fn release_notice(&self, chat_id: i64) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        sqlx::query("DELETE FROM bot_maintenance_notices WHERE chat_id = $1")
            .bind(chat_id)
            .execute(conn.conn())
            .await?;
        Ok(())
    }

    pub async fn claim_notice(&self, chat_id: i64) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        let claimed: Option<bool> = sqlx::query_scalar(
            "INSERT INTO bot_maintenance_notices AS n (chat_id) VALUES ($1) \
             ON CONFLICT (chat_id) DO UPDATE SET sent_at = now() \
             WHERE n.sent_at <= now() - make_interval(mins => $2) \
             RETURNING true",
        )
        .bind(chat_id)
        .bind(NOTICE_EVERY_MINUTES)
        .fetch_optional(conn.conn())
        .await?;
        Ok(claimed.unwrap_or(false))
    }
}
