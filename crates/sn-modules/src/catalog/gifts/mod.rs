mod routes;
pub mod store;
#[cfg(test)]
mod tests;

use crate::catalog::start_param;
use crate::catalog::{db_now, installation_currency, module_config};
use crate::context::{
    GrantCurrent, GrantRefusal, GrantSpec, GrantTarget, ModuleContext, Notification,
    SubscriptionGrant,
};
use crate::error::ModuleError;
use crate::module::{BotEntry, ItemHook, Module, ModuleConfig};
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::Locale;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, CategoryId, ClientId, OrderId, PeriodId, TariffId};
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money, Stars};
use sn_domain::order::{Order, OrderStatus, PurchaseIntent};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::plan::PlanKind;
use sn_domain::pricing::{Quote, QuoteLine, QuoteLineKind, StarsTotal};
use sn_domain::promotion::{
    PromotionTerms, discounted, discounted_stars, line_label, running_for, until_text,
};
use sn_domain::settings::keys;
use sn_domain::tariff::{DevicePolicy, PeriodOffer, Tariff, period_offers};
use sn_domain::traffic::TrafficPolicy;
use sn_domain::{format, lang};
use sn_store::{Db, StoreError};
use uuid::Uuid;

pub use store::{AdminGift, Counts, ListFilter};

pub const BOT_ENTRY: &str = "gifts";
pub const ITEM_GIFT: &str = "gift";
pub const START_PREFIX: &str = "gift_";
pub const TEMPLATE_READY: &str = "gift.ready";
pub const TEMPLATE_REDEEMED: &str = "gift.redeemed";
pub const DEFAULT_CODE_DAYS: u32 = 90;
pub const MAX_CODE_DAYS: u32 = 3_650;
pub const MINE_LIMIT: i64 = 50;

const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_ATTEMPTS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GiftRequest {
    pub tariff_id: TariffId,
    pub period_id: PeriodId,
}

impl GiftRequest {
    pub fn payload(&self) -> serde_json::Value {
        serde_json::json!({ "tariff_id": self.tariff_id, "period_id": self.period_id })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GiftSettings {
    pub tariffs: Vec<TariffId>,
    pub code_days: u32,
}

impl Default for GiftSettings {
    fn default() -> Self {
        GiftSettings {
            tariffs: Vec::new(),
            code_days: DEFAULT_CODE_DAYS,
        }
    }
}

impl GiftSettings {
    pub fn parse(config: &ModuleConfig) -> Result<Self, String> {
        let mut settings = GiftSettings::default();
        match config.get("tariffs") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(items)) => {
                for item in items {
                    let id = item
                        .as_i64()
                        .filter(|id| *id > 0)
                        .ok_or_else(|| "тарифы для подарков указаны неверно".to_string())?;
                    if !settings.tariffs.contains(&TariffId(id)) {
                        settings.tariffs.push(TariffId(id));
                    }
                }
            }
            Some(_) => return Err("тарифы для подарков указаны неверно".into()),
        }
        if let Some(days) = crate::catalog::config_number(config, "code_days") {
            settings.code_days = u32::try_from(days)
                .ok()
                .filter(|d| *d <= MAX_CODE_DAYS)
                .ok_or_else(|| format!("срок действия кода — от 0 до {MAX_CODE_DAYS} дней"))?;
        } else if config.get("code_days").is_some_and(|v| !v.is_null()) {
            return Err(format!("срок действия кода — от 0 до {MAX_CODE_DAYS} дней"));
        }
        Ok(settings)
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::json!({ "tariffs": self.tariffs, "code_days": self.code_days })
    }

    pub fn allows(&self, tariff: TariffId) -> bool {
        self.tariffs.is_empty() || self.tariffs.contains(&tariff)
    }
}

pub async fn settings(db: &mut impl Db) -> Result<GiftSettings, ModuleError> {
    let config = module_config(db, ModuleId::Gifts).await?;
    GiftSettings::parse(&config).map_err(ModuleError::Invalid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GiftStatus {
    Active,
    Redeemed,
    Expired,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gift {
    pub id: i64,
    pub public_id: Uuid,
    pub code: String,
    pub order_id: OrderId,
    pub giver_id: ClientId,
    pub tariff_id: TariffId,
    pub period_id: PeriodId,
    pub days: u32,
    pub tariff_name: String,
    pub status: GiftStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub redeemed_by: Option<ClientId>,
    pub redeemed_at: Option<DateTime<Utc>>,
    pub grant_order_id: Option<OrderId>,
    pub canceled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Gift {
    pub fn term(&self) -> String {
        term(self.days)
    }

    pub fn label(&self) -> String {
        format!("«{}» на {}", self.tariff_name, term_ru(self.days))
    }
}

pub fn term(days: u32) -> String {
    format::term_in(days, lang::current())
}

pub fn term_ru(days: u32) -> String {
    format::term_in(days, Locale::Ru)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GiftLinks {
    pub bot: Option<String>,
    pub cabinet: Option<String>,
}

pub fn bot_link(bot_username: &str, code: &str) -> String {
    format!(
        "https://t.me/{}?start={START_PREFIX}{code}",
        bot_username.trim().trim_start_matches('@')
    )
}

pub fn cabinet_link(public_url: &str, code: &str) -> String {
    format!(
        "{}/cabinet/gifts?code={code}",
        public_url.trim().trim_end_matches('/')
    )
}

pub async fn links(db: &mut impl Db, code: &str) -> Result<GiftLinks, ModuleError> {
    let text = |value: Option<serde_json::Value>| {
        value
            .and_then(|v| v.as_str().map(str::trim).map(str::to_string))
            .filter(|v| !v.is_empty())
    };
    let bot = text(db.settings().get(keys::BOT_USERNAME).await?);
    let site = text(db.settings().get(keys::PUBLIC_URL).await?)
        .or_else(|| {
            std::env::var("SN_PUBLIC_URL")
                .ok()
                .filter(|u| !u.trim().is_empty())
        })
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"));
    Ok(GiftLinks {
        bot: bot.map(|b| bot_link(&b, code)),
        cabinet: site.map(|s| cabinet_link(&s, code)),
    })
}

pub fn generate_code() -> String {
    let bytes = Uuid::new_v4();
    let chars: Vec<char> = bytes
        .as_bytes()
        .iter()
        .take(12)
        .map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char)
        .collect();
    format!(
        "{}-{}-{}",
        chars[..4].iter().collect::<String>(),
        chars[4..8].iter().collect::<String>(),
        chars[8..].iter().collect::<String>()
    )
}

pub fn normalize_code(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let raw = match raw.find(START_PREFIX) {
        Some(at) => &raw[at + START_PREFIX.len()..],
        None => raw,
    };
    let raw = raw.split(start_param::SEPARATOR).next().unwrap_or(raw);
    let compact: String = raw
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | '\u{a0}'))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if compact.len() != 12 || !compact.bytes().all(|b| CODE_ALPHABET.contains(&b)) {
        return None;
    }
    Some(format!(
        "{}-{}-{}",
        &compact[..4],
        &compact[4..8],
        &compact[8..]
    ))
}

pub fn start_code(raw: &str) -> Option<String> {
    start_param::segments(raw)
        .find_map(|segment| segment.strip_prefix(START_PREFIX))
        .and_then(normalize_code)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum GiftRejection {
    NotGiftable,
    NoPeriod,
    NotFound,
    Redeemed,
    RedeemedByYou,
    Expired,
    Canceled,
    RefundPending,
    Grant(GrantRefusal),
}

impl GiftRejection {
    pub fn code(&self) -> &'static str {
        match self {
            GiftRejection::NotGiftable => "gifts.not_giftable",
            GiftRejection::NoPeriod => "gifts.no_period",
            GiftRejection::NotFound => "gifts.not_found",
            GiftRejection::Redeemed => "gifts.redeemed",
            GiftRejection::RedeemedByYou => "gifts.redeemed_by_you",
            GiftRejection::Expired => "gifts.expired",
            GiftRejection::Canceled => "gifts.canceled",
            GiftRejection::RefundPending => "gifts.refund_pending",
            GiftRejection::Grant(refusal) => refusal.code,
        }
    }

    pub fn message(&self) -> String {
        match self {
            GiftRejection::NotGiftable => lang::pick(
                "Этот тариф сейчас нельзя подарить",
                "This plan can't be gifted right now",
            )
            .into(),
            GiftRejection::NoPeriod => lang::pick(
                "Такого срока у тарифа нет, выберите другой",
                "This plan has no such period, choose another one",
            )
            .into(),
            GiftRejection::NotFound => lang::pick(
                "Подарка с таким кодом нет. Проверьте код.",
                "There's no gift with this code. Check the code.",
            )
            .into(),
            GiftRejection::Redeemed => lang::pick(
                "Этот подарок уже активирован",
                "This gift has already been activated",
            )
            .into(),
            GiftRejection::RedeemedByYou => lang::pick(
                "Вы уже активировали этот подарок",
                "You've already activated this gift",
            )
            .into(),
            GiftRejection::Expired => {
                lang::pick("Срок действия кода закончился", "This code has expired").into()
            }
            GiftRejection::Canceled => lang::pick(
                "Подарок отменён: за него вернули деньги",
                "This gift was cancelled and refunded",
            )
            .into(),
            GiftRejection::RefundPending => lang::pick(
                "Подарок пока нельзя активировать: по его оплате оформляется возврат",
                "This gift can't be activated yet: its payment is being refunded",
            )
            .into(),
            GiftRejection::Grant(refusal) => refusal.message.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GiftError {
    #[error("{}", .0.message())]
    Rejected(GiftRejection),
    #[error(transparent)]
    Module(#[from] ModuleError),
}

impl From<StoreError> for GiftError {
    fn from(err: StoreError) -> Self {
        GiftError::Module(ModuleError::Store(err))
    }
}

impl From<GiftError> for ModuleError {
    fn from(err: GiftError) -> Self {
        match err {
            GiftError::Rejected(rejection) => ModuleError::Invalid(rejection.message()),
            GiftError::Module(err) => err,
        }
    }
}

fn rejected<T>(rejection: GiftRejection) -> Result<T, GiftError> {
    Err(GiftError::Rejected(rejection))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GiftTariff {
    pub id: TariffId,
    pub category_id: CategoryId,
    pub name: String,
    pub description: Option<String>,
    pub devices: DevicePolicy,
    pub traffic: TrafficPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GiftOffer {
    pub tariff: GiftTariff,
    pub periods: Vec<PeriodOffer>,
}

pub async fn offers(
    db: &mut impl Db,
    currency: Currency,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Result<Vec<GiftOffer>, ModuleError> {
    let settings = settings(db).await?;
    let tariffs = db.tariffs().list(false).await?;
    let running = db.promotions().running(now).await?;
    let locale = lang::current();
    Ok(tariffs
        .into_iter()
        .filter(|t| settings.allows(t.id) && !t.squads.is_empty())
        .filter_map(|tariff| {
            let promotion = running_for(&running, tariff.id, now);
            let mut periods = period_offers(&tariff, currency, promotion.as_ref());
            for period in &mut periods {
                if let Some(promotion) = period.promotion.as_mut() {
                    promotion.until = until_text(promotion.ends_at, offset, locale, now);
                }
            }
            (!periods.is_empty()).then_some(GiftOffer {
                tariff: GiftTariff {
                    id: tariff.id,
                    category_id: tariff.category_id,
                    name: tariff.name,
                    description: tariff.description,
                    devices: tariff.devices,
                    traffic: tariff.traffic,
                },
                periods,
            })
        })
        .collect())
}

pub fn gift_quote(
    tariff: &Tariff,
    period_id: PeriodId,
    currency: Currency,
    promotion: Option<&PromotionTerms>,
) -> Result<Quote, GiftError> {
    let period = tariff
        .sellable_period(period_id, currency)
        .map_err(|_| GiftError::Rejected(GiftRejection::NoPeriod))?;
    let Some(amount) = period.price(currency) else {
        return rejected(GiftRejection::NoPeriod);
    };
    let stars = period.stars;
    let mut lines = vec![QuoteLine {
        kind: QuoteLineKind::Period {
            tariff_id: tariff.id,
            period_id: period.id,
            days: period.days,
            months: period.months(),
        },
        label: format!("Подарок: «{}» на {}", tariff.name, term_ru(period.days)),
        amount,
        stars,
    }];
    let mut total = amount;
    let mut stars_total = stars;
    if let Some(promotion) = promotion {
        let price = discounted(amount, promotion.percent);
        let stars_price = stars.map(|s| discounted_stars(s, promotion.percent));
        if price != amount || stars_price != stars {
            lines.push(QuoteLine {
                kind: QuoteLineKind::Promotion {
                    promotion_id: promotion.id,
                    percent: promotion.percent,
                },
                label: line_label(&promotion.name, promotion.percent),
                amount: Money::new(price.minor - amount.minor, currency),
                stars: match (stars, stars_price) {
                    (Some(full), Some(now)) => Some(Stars(now.0 - full.0)),
                    _ => None,
                },
            });
            total = price;
            stars_total = stars_price;
        }
    }
    Ok(Quote {
        currency,
        lines,
        subtotal: amount,
        discount: Money::new(amount.minor - total.minor, currency),
        total,
        stars: match (stars, stars_total) {
            (Some(subtotal), Some(total)) => Some(StarsTotal {
                subtotal,
                discount: Stars(subtotal.0 - total.0),
                total,
            }),
            _ => None,
        },
        days: period.days,
        extra_devices: 0,
    })
}

async fn giftable(db: &mut impl Db, tariff_id: TariffId) -> Result<Tariff, GiftError> {
    let settings = settings(db).await?;
    let tariff = match db.tariffs().get(tariff_id).await {
        Ok(tariff) => tariff,
        Err(StoreError::NotFound(_)) => return rejected(GiftRejection::NotGiftable),
        Err(err) => return Err(err.into()),
    };
    if !tariff.on_sale || !settings.allows(tariff.id) || tariff.squads.is_empty() {
        return rejected(GiftRejection::NotGiftable);
    }
    Ok(tariff)
}

pub async fn quote(
    db: &mut impl Db,
    request: GiftRequest,
    currency: Currency,
    now: DateTime<Utc>,
) -> Result<Quote, GiftError> {
    let tariff = giftable(db, request.tariff_id).await?;
    let running = db.promotions().running(now).await?;
    let promotion = running_for(&running, tariff.id, now);
    gift_quote(&tariff, request.period_id, currency, promotion.as_ref())
}

fn parse_request(kind: &str, payload: &serde_json::Value) -> Result<GiftRequest, ModuleError> {
    if kind != ITEM_GIFT {
        return Err(ModuleError::Invalid(format!("неизвестный товар: {kind}")));
    }
    serde_json::from_value(payload.clone())
        .map_err(|e| ModuleError::Invalid(format!("подарок не разобран: {e}")))
}

pub async fn issue(ctx: &ModuleContext, order: &Order) -> Result<Gift, ModuleError> {
    let PurchaseIntent::Module {
        module: ModuleId::Gifts,
        kind,
        payload,
    } = &order.intent
    else {
        return Err(ModuleError::Invalid("заказ не на подарок".into()));
    };
    if !matches!(
        order.status,
        OrderStatus::Paid | OrderStatus::Review | OrderStatus::Fulfilled
    ) {
        return Err(ModuleError::Invalid("заказ не оплачен".into()));
    }
    let request = parse_request(kind, payload)?;
    let days = order
        .quote
        .lines
        .iter()
        .find_map(|line| match line.kind {
            QuoteLineKind::Period { days, .. } => Some(days),
            _ => None,
        })
        .filter(|d| *d > 0)
        .ok_or_else(|| ModuleError::Invalid("в заказе подарка нет срока".into()))?;
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    if let Some(existing) = store::by_order(&mut conn, order.id, now).await? {
        return Ok(existing);
    }
    let settings = settings(&mut conn).await?;
    let tariff_name = match conn.tariffs().get(request.tariff_id).await {
        Ok(tariff) => tariff.name,
        Err(StoreError::NotFound(_)) => order
            .quote
            .lines
            .first()
            .map(|l| l.label.clone())
            .unwrap_or_else(|| "Подписка".into()),
        Err(err) => return Err(err.into()),
    };
    drop(conn);
    let expires_at =
        (settings.code_days > 0).then(|| now + Duration::days(i64::from(settings.code_days)));
    for _ in 0..CODE_ATTEMPTS {
        let code = generate_code();
        let mut tx = ctx.store.begin().await?;
        let inserted = store::insert(
            &mut tx,
            &store::NewGift {
                code: &code,
                order_id: order.id,
                giver_id: order.client_id,
                tariff_id: request.tariff_id,
                period_id: request.period_id,
                days,
                tariff_name: &tariff_name,
                expires_at,
            },
        )
        .await;
        match inserted {
            Ok(Some(_)) => {
                let gift = store::by_order(&mut tx, order.id, now)
                    .await?
                    .ok_or(ModuleError::NotFound("подарок"))?;
                let links = links(&mut tx, &gift.code).await?;
                ctx.core
                    .notify_in(
                        &mut tx,
                        order.client_id,
                        Notification {
                            template: TEMPLATE_READY.into(),
                            params: serde_json::json!({
                                "code": gift.code,
                                "tariff": gift.tariff_name,
                                "term": gift.term(),
                                "days": gift.days,
                                "bot_link": links.bot,
                                "cabinet_link": links.cabinet,
                                "expire_at": gift.expires_at,
                            }),
                        },
                    )
                    .await?;
                tx.commit().await?;
                return Ok(gift);
            }
            Ok(None) => {
                tx.rollback().await?;
                let mut conn = ctx.store.conn().await?;
                return store::by_order(&mut conn, order.id, now)
                    .await?
                    .ok_or(ModuleError::NotFound("подарок"));
            }
            Err(StoreError::Conflict { .. }) => {
                tx.rollback().await?;
            }
            Err(err) => return Err(err.into()),
        }
    }
    Err(ModuleError::Core(
        "не удалось подобрать свободный код подарка".into(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MyGift {
    #[serde(flatten)]
    pub gift: Gift,
    pub term: String,
    pub links: GiftLinks,
}

pub async fn mine(ctx: &ModuleContext, client_id: ClientId) -> Result<Vec<MyGift>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    let gifts = store::by_giver(&mut conn, client_id, MINE_LIMIT, now).await?;
    let mut out = Vec::with_capacity(gifts.len());
    for gift in gifts {
        let links = if gift.status == GiftStatus::Active {
            links(&mut conn, &gift.code).await?
        } else {
            GiftLinks {
                bot: None,
                cabinet: None,
            }
        };
        out.push(MyGift {
            term: gift.term(),
            gift,
            links,
        });
    }
    Ok(out)
}

fn grant_request(gift: &Gift, client_id: ClientId, target: GrantTarget) -> SubscriptionGrant {
    SubscriptionGrant {
        client_id,
        module: ModuleId::Gifts,
        reference: format!("gift:{}", gift.id),
        label: format!("Подарок: {}", gift.label()),
        spec: GrantSpec::Tariff {
            tariff_id: gift.tariff_id,
            period_id: gift.period_id,
            days: gift.days,
        },
        target,
    }
}

fn check_open(gift: &Gift, client_id: ClientId) -> Result<(), GiftError> {
    match gift.status {
        GiftStatus::Active => Ok(()),
        GiftStatus::Redeemed if gift.redeemed_by == Some(client_id) => {
            rejected(GiftRejection::RedeemedByYou)
        }
        GiftStatus::Redeemed => rejected(GiftRejection::Redeemed),
        GiftStatus::Expired => rejected(GiftRejection::Expired),
        GiftStatus::Canceled => rejected(GiftRejection::Canceled),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GiftCard {
    pub code: String,
    pub tariff: String,
    pub days: u32,
    pub term: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub own: bool,
}

impl GiftCard {
    fn of(gift: &Gift, client_id: ClientId) -> Self {
        GiftCard {
            code: gift.code.clone(),
            tariff: gift.tariff_name.clone(),
            days: gift.days,
            term: gift.term(),
            expires_at: gift.expires_at,
            own: gift.giver_id == client_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RedeemPreview {
    pub gift: GiftCard,
    pub plan: PlanKind,
    pub multi: bool,
    pub current: Option<GrantCurrent>,
    pub choices: Vec<GrantCurrent>,
    pub new_allowed: bool,
}

pub async fn preview(
    ctx: &ModuleContext,
    client_id: ClientId,
    raw_code: &str,
    target: GrantTarget,
) -> Result<RedeemPreview, GiftError> {
    let Some(code) = normalize_code(raw_code) else {
        return rejected(GiftRejection::NotFound);
    };
    let gift = {
        let mut conn = ctx.store.conn().await?;
        let now = db_now(&mut conn).await?;
        let Some(gift) = store::by_code(&mut conn, &code, now).await? else {
            return rejected(GiftRejection::NotFound);
        };
        check_open(&gift, client_id)?;
        let (status, pending) = {
            let mut tx = ctx.store.begin().await?;
            let state = store::order_state(&mut tx, gift.order_id).await?;
            tx.rollback().await?;
            state
        };
        if status == OrderStatus::Refunded.as_str() {
            return rejected(GiftRejection::Canceled);
        }
        if pending {
            return rejected(GiftRejection::RefundPending);
        }
        gift
    };
    let preview = ctx
        .core
        .grant_preview(&grant_request(&gift, client_id, target))
        .await?;
    if let Some(refusal) = preview.refusal {
        return rejected(GiftRejection::Grant(refusal));
    }
    Ok(RedeemPreview {
        gift: GiftCard::of(&gift, client_id),
        plan: preview.plan,
        multi: preview.multi,
        current: preview.current,
        choices: preview.choices,
        new_allowed: preview.new_allowed,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Redeemed {
    pub gift: GiftCard,
    pub order_id: Uuid,
    pub plan: PlanKind,
    pub subscription_id: Option<sn_domain::ids::SubscriptionId>,
}

pub async fn redeem(
    ctx: &ModuleContext,
    client_id: ClientId,
    raw_code: &str,
    target: GrantTarget,
) -> Result<Redeemed, GiftError> {
    let Some(code) = normalize_code(raw_code) else {
        return rejected(GiftRejection::NotFound);
    };
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let Some(gift) = store::lock_by_code(&mut tx, &code, now).await? else {
        return rejected(GiftRejection::NotFound);
    };
    check_open(&gift, client_id)?;
    let (status, pending) = store::order_state(&mut tx, gift.order_id).await?;
    if status == OrderStatus::Refunded.as_str() {
        store::cancel_refunded(&mut tx, gift.order_id).await?;
        tx.commit().await?;
        return rejected(GiftRejection::Canceled);
    }
    if pending {
        return rejected(GiftRejection::RefundPending);
    }
    let granted = match ctx
        .core
        .grant_subscription(&mut tx, &grant_request(&gift, client_id, target))
        .await?
    {
        Ok(granted) => granted,
        Err(refusal) => {
            tx.rollback().await?;
            return rejected(GiftRejection::Grant(refusal));
        }
    };
    store::mark_redeemed(&mut tx, gift.id, client_id, granted.order_id).await?;
    if gift.giver_id != client_id {
        ctx.core
            .notify_in(
                &mut tx,
                gift.giver_id,
                Notification {
                    template: TEMPLATE_REDEEMED.into(),
                    params: serde_json::json!({
                        "code": gift.code,
                        "tariff": gift.tariff_name,
                        "term": gift.term(),
                        "days": gift.days,
                    }),
                },
            )
            .await?;
    }
    tx.commit().await?;
    Ok(Redeemed {
        gift: GiftCard::of(&gift, client_id),
        order_id: granted.order_public_id,
        plan: granted.plan,
        subscription_id: granted.subscription_id,
    })
}

pub async fn admin_list(
    ctx: &ModuleContext,
    filter: ListFilter,
    search: Option<&str>,
    page: PageRequest,
) -> Result<Page<AdminGift>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    Ok(store::list(&mut conn, filter, search, page, now).await?)
}

pub async fn counts(ctx: &ModuleContext) -> Result<Counts, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    Ok(store::counts(&mut conn, now).await?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsTariff {
    pub id: TariffId,
    pub name: String,
    pub on_sale: bool,
    pub giftable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    pub tariffs: Vec<TariffId>,
    pub code_days: u32,
    pub available: Vec<SettingsTariff>,
}

pub async fn settings_view(ctx: &ModuleContext) -> Result<SettingsView, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let settings = settings(&mut conn).await?;
    let available = conn
        .tariffs()
        .list(true)
        .await?
        .into_iter()
        .map(|t| SettingsTariff {
            giftable: t.is_offered(currency) && !t.squads.is_empty(),
            id: t.id,
            name: t.name,
            on_sale: t.on_sale,
        })
        .collect();
    Ok(SettingsView {
        tariffs: settings.tariffs,
        code_days: settings.code_days,
        available,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SettingsUpdate {
    #[serde(default)]
    pub tariffs: Vec<TariffId>,
    pub code_days: u32,
}

pub async fn save_settings(
    ctx: &ModuleContext,
    update: &SettingsUpdate,
    admin: Option<AdminId>,
) -> Result<SettingsView, ModuleError> {
    if update.code_days > MAX_CODE_DAYS {
        return Err(ModuleError::Invalid(format!(
            "срок действия кода — от 0 до {MAX_CODE_DAYS} дней"
        )));
    }
    let mut tx = ctx.store.begin().await?;
    let mut config = module_config(&mut tx, ModuleId::Gifts).await?;
    let mut tariffs = Vec::new();
    for id in &update.tariffs {
        match tx.tariffs().get(*id).await {
            Ok(_) => {
                if !tariffs.contains(id) {
                    tariffs.push(*id);
                }
            }
            Err(StoreError::NotFound(_)) => {
                return Err(ModuleError::Invalid("такого тарифа нет".into()));
            }
            Err(err) => return Err(err.into()),
        }
    }
    let settings = GiftSettings {
        tariffs,
        code_days: update.code_days,
    };
    if let serde_json::Value::Object(map) = settings.to_value() {
        for (key, value) in map {
            config.0.insert(key, value);
        }
    }
    sqlx::query(
        "INSERT INTO module_states (module, enabled, config, updated_by) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (module) DO UPDATE SET config = EXCLUDED.config, updated_by = EXCLUDED.updated_by, \
         updated_at = now()",
    )
    .bind(ModuleId::Gifts.as_str())
    .bind(ModuleId::Gifts.info().enabled_by_default)
    .bind(config.to_value())
    .bind(admin.map(|a| a.0))
    .execute(tx.conn())
    .await
    .map_err(StoreError::from)?;
    tx.audit()
        .record(&sn_store::AuditEntry {
            actor_kind: match admin {
                Some(_) => sn_store::ActorKind::Admin,
                None => sn_store::ActorKind::System,
            },
            actor_id: admin.map(|a| a.0),
            action: "gifts.settings".into(),
            entity_kind: None,
            entity_id: None,
            details: settings.to_value(),
            ip: None,
        })
        .await?;
    tx.commit().await?;
    settings_view(ctx).await
}

pub struct GiftsModule;

#[async_trait]
impl Module for GiftsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Gifts
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        GiftSettings::parse(config)
            .map(|_| ())
            .map_err(ModuleError::Invalid)
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn client_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::client(ctx))
    }

    fn bot_entries(&self) -> Vec<BotEntry> {
        vec![BotEntry {
            key: BOT_ENTRY,
            label: "Подарить",
            order: 45,
        }]
    }

    async fn quote_item(&self, ctx: &ModuleContext, hook: &ItemHook) -> Result<Quote, ModuleError> {
        let request = parse_request(&hook.kind, &hook.payload)?;
        let mut conn = ctx.store.conn().await?;
        Ok(quote(&mut conn, request, hook.currency, ctx.core.now()).await?)
    }

    async fn fulfill(&self, ctx: &ModuleContext, order: &Order) -> Result<(), ModuleError> {
        issue(ctx, order).await.map(|_| ())
    }

    async fn on_event(&self, ctx: &ModuleContext, event: &DomainEvent) -> Result<(), ModuleError> {
        if let DomainEvent::PaymentRefunded { order_id, .. } = event {
            let mut conn = ctx.store.conn().await?;
            store::cancel_refunded(&mut conn, *order_id).await?;
        }
        Ok(())
    }
}
