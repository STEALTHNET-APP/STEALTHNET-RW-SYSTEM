use super::auto::{self, Delay, Localized, PERCENT_DEFAULT, RuleContent, Trigger};
use super::auto_store;
use super::{BotSection, BroadcastError, ButtonSpec, Channel};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use serde::Serialize;
use sn_domain::ids::AdminId;
use sn_domain::module::ModuleId;
use sn_domain::settings::{TrialSettings, keys};
use sn_store::{Db, StoreResult};

pub const PRESETS_KEY: &str = "broadcast.auto_presets";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub key: &'static str,
    pub trigger: Trigger,
    pub covered_by: &'static [Trigger],
}

pub const PRESETS: [Preset; 4] = [
    Preset {
        key: "no_subscription",
        trigger: Trigger::NoSubscription,
        covered_by: &[Trigger::NoSubscription, Trigger::NoPurchase],
    },
    Preset {
        key: "trial_expiring",
        trigger: Trigger::TrialExpiring,
        covered_by: &[Trigger::TrialExpiring],
    },
    Preset {
        key: "expiring",
        trigger: Trigger::Expiring,
        covered_by: &[Trigger::Expiring],
    },
    Preset {
        key: "traffic_low",
        trigger: Trigger::TrafficLow,
        covered_by: &[Trigger::TrafficLow],
    },
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shop {
    pub trial: bool,
    pub traffic_packs: bool,
}

fn button(text: &str, section: BotSection) -> ButtonSpec {
    ButtonSpec {
        text: text.into(),
        url: None,
        section: Some(section),
        style: None,
        emoji_id: None,
    }
}

fn message(text: &str, buttons: Vec<ButtonSpec>) -> Localized {
    Localized {
        subject: None,
        text: text.into(),
        buttons,
    }
}

fn texts(preset: &Preset, shop: Shop) -> (Localized, Localized) {
    match preset.trigger {
        Trigger::NoSubscription if shop.trial => (
            message(
                "Вы зарегистрировались, но ещё не подключили VPN. Попробуйте бесплатно: пробный период включается одной кнопкой. Или сразу выберите тариф — настройка займёт пару минут.",
                vec![
                    button("Попробовать бесплатно", BotSection::Trial),
                    button("Выбрать тариф", BotSection::Buy),
                ],
            ),
            message(
                "You signed up but haven't connected the VPN yet. Try it for free: the trial starts with one tap. Or pick a plan right away — setup takes a couple of minutes.",
                vec![
                    button("Try for free", BotSection::Trial),
                    button("Choose a plan", BotSection::Buy),
                ],
            ),
        ),
        Trigger::NoSubscription => (
            message(
                "Вы зарегистрировались, но ещё не подключили VPN. Выберите тариф — настройка займёт пару минут.",
                vec![button("Выбрать тариф", BotSection::Buy)],
            ),
            message(
                "You signed up but haven't connected the VPN yet. Pick a plan — setup takes a couple of minutes.",
                vec![button("Choose a plan", BotSection::Buy)],
            ),
        ),
        Trigger::TrialExpiring => (
            message(
                "Пробный период скоро закончится. Чтобы VPN продолжил работать, выберите тариф — подписка начнётся сразу после оплаты.",
                vec![button("Выбрать тариф", BotSection::Buy)],
            ),
            message(
                "Your trial ends soon. Pick a plan to keep the VPN running — the subscription starts right after payment.",
                vec![button("Choose a plan", BotSection::Buy)],
            ),
        ),
        Trigger::Expiring => (
            message(
                "Подписка скоро закончится. Продлите её заранее — VPN не отключится, а новые дни добавятся к текущему сроку.",
                vec![button("Продлить подписку", BotSection::Buy)],
            ),
            message(
                "Your subscription ends soon. Renew it in advance — the VPN won't stop and the new days are added to the current term.",
                vec![button("Renew subscription", BotSection::Buy)],
            ),
        ),
        Trigger::TrafficLow if shop.traffic_packs => (
            message(
                "Трафик по подписке почти закончился. Докупите пакет трафика или выберите тариф с большим объёмом — VPN продолжит работать без ограничений.",
                vec![
                    button("Докупить трафик", BotSection::Options),
                    button("Сменить тариф", BotSection::Buy),
                ],
            ),
            message(
                "Your subscription traffic is almost used up. Buy a traffic pack or switch to a plan with more traffic to keep the VPN running without limits.",
                vec![
                    button("Buy traffic", BotSection::Options),
                    button("Change plan", BotSection::Buy),
                ],
            ),
        ),
        _ => (
            message(
                "Трафик по подписке почти закончился. Выберите тариф с большим объёмом — VPN продолжит работать без ограничений.",
                vec![button("Сменить тариф", BotSection::Buy)],
            ),
            message(
                "Your subscription traffic is almost used up. Switch to a plan with more traffic to keep the VPN running without limits.",
                vec![button("Change plan", BotSection::Buy)],
            ),
        ),
    }
}

pub fn content(preset: &Preset, shop: Shop) -> RuleContent {
    let (ru, en) = texts(preset, shop);
    let (delay, percent) = match preset.trigger {
        Trigger::TrialExpiring => (Delay::hours(6), None),
        Trigger::Expiring => (Delay::days(3), None),
        Trigger::TrafficLow => (Delay::days(0), Some(PERCENT_DEFAULT)),
        _ => (Delay::days(1), None),
    };
    RuleContent {
        trigger: preset.trigger,
        delay,
        percent,
        with_autorenew: false,
        channel: Channel::Bot,
        subject: None,
        text: ru.text,
        image_id: None,
        buttons: ru.buttons,
        en: Some(en),
        audience: None,
    }
}

async fn handled(db: &mut impl Db) -> StoreResult<Vec<String>> {
    Ok(db
        .settings()
        .get(PRESETS_KEY)
        .await?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default())
}

async fn module_on(db: &mut impl Db, id: ModuleId) -> StoreResult<bool> {
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM module_states WHERE module = $1")
            .bind(id.as_str())
            .fetch_optional(db.conn())
            .await?;
    Ok(enabled.unwrap_or(id.info().enabled_by_default))
}

async fn shop(db: &mut impl Db) -> StoreResult<Shop> {
    let trial = db
        .settings()
        .get(keys::TRIAL)
        .await?
        .and_then(|value| serde_json::from_value::<TrialSettings>(value).ok())
        .is_some_and(|trial| trial.enabled);
    let packs: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tariffs WHERE deleted_at IS NULL AND traffic_addon IS NOT NULL)",
    )
    .fetch_one(db.conn())
    .await?;
    Ok(Shop {
        trial,
        traffic_packs: packs && module_on(db, ModuleId::Options).await?,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Seeded {
    pub created: Vec<&'static str>,
    pub covered: Vec<&'static str>,
    pub enabled: bool,
}

pub async fn seed(ctx: &ModuleContext) -> Result<Seeded, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(PRESETS_KEY)
        .execute(tx.conn())
        .await
        .map_err(sn_store::StoreError::from)?;
    let mut done = handled(&mut tx).await?;
    let waiting: Vec<&Preset> = PRESETS
        .iter()
        .filter(|preset| !done.iter().any(|key| key == preset.key))
        .collect();
    if waiting.is_empty() {
        tx.rollback().await?;
        return Ok(Seeded::default());
    }
    let fresh: bool = sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM clients)")
        .fetch_one(tx.conn())
        .await
        .map_err(sn_store::StoreError::from)?;
    let live: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT trigger FROM auto_broadcasts WHERE deleted_at IS NULL")
            .fetch_all(tx.conn())
            .await
            .map_err(sn_store::StoreError::from)?;
    let shop = shop(&mut tx).await?;
    let mut seeded = Seeded {
        enabled: fresh,
        ..Seeded::default()
    };
    for preset in waiting {
        let covered = preset
            .covered_by
            .iter()
            .any(|trigger| live.iter().any(|t| t == trigger.as_str()));
        if covered {
            seeded.covered.push(preset.key);
        } else {
            let content = content(preset, shop);
            let id = auto_store::insert(&mut tx, &content, fresh, Some(preset.key), None).await?;
            tx.audit()
                .record(&auto::audit(
                    "broadcast.auto_preset_added",
                    None,
                    id,
                    serde_json::json!({ "preset": preset.key, "enabled": fresh }),
                ))
                .await?;
            seeded.created.push(preset.key);
        }
        done.push(preset.key.to_string());
    }
    tx.settings()
        .set(PRESETS_KEY, &serde_json::json!(done), None)
        .await?;
    tx.commit().await?;
    Ok(seeded)
}

pub async fn enable(
    ctx: &ModuleContext,
    admin: Option<AdminId>,
) -> Result<Vec<i64>, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, channel FROM auto_broadcasts WHERE preset IS NOT NULL AND deleted_at IS NULL \
         AND NOT enabled ORDER BY id FOR UPDATE",
    )
    .fetch_all(tx.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    let mut ids = Vec::with_capacity(rows.len());
    for (id, channel) in rows {
        let channel = Channel::parse(&channel).unwrap_or_default();
        auto::ready(ctx, channel, true)?;
        auto_store::set_enabled(&mut tx, id, true).await?;
        tx.audit()
            .record(&auto::audit(
                "broadcast.auto_enabled",
                admin,
                id,
                serde_json::json!({ "presets": true }),
            ))
            .await?;
        ids.push(id);
    }
    tx.commit().await?;
    Ok(ids)
}
