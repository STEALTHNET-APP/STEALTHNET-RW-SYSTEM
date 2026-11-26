use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Datelike, FixedOffset, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::DomainError;
use sn_domain::format::utc_label;
use sn_domain::ids::{AdminId, TariffId};
use sn_domain::lang;
use sn_domain::pricing::{Quote, promotion_line};
use sn_domain::promotion::{
    Promotion, PromotionDraft, PromotionState, PromotionTerms, running_for, until_text,
};
use sn_domain::tariff::{PeriodOffer, Tariff};
use sn_store::Db;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromotionNote {
    pub id: i64,
    pub name: String,
    pub percent: u8,
    pub ends_at: DateTime<Utc>,
    pub until: String,
    pub applied: bool,
    pub code_skipped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromotionView {
    #[serde(flatten)]
    pub promotion: Promotion,
    pub state: PromotionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromotionsView {
    pub promotions: Vec<PromotionView>,
    pub offset_minutes: i32,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PromotionInput {
    pub name: String,
    pub percent: u8,
    pub tariff_ids: Vec<TariffId>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

const fn enabled_by_default() -> bool {
    true
}

fn rejected(err: DomainError) -> ServiceError {
    match err {
        DomainError::InvalidValue { reason, .. } => {
            ServiceError::rejected("promotion.invalid", reason)
        }
        other => other.into(),
    }
}

fn stamp(at: DateTime<Utc>, offset: FixedOffset) -> String {
    let local = at.with_timezone(&offset);
    format!(
        "{:02}.{:02}.{} {:02}:{:02} ({})",
        local.day(),
        local.month(),
        local.year(),
        local.hour(),
        local.minute(),
        utc_label(offset)
    )
}

fn details(promotion: &Promotion, tariffs: &[Tariff], offset: FixedOffset) -> serde_json::Value {
    let names: Vec<String> = promotion
        .tariff_ids
        .iter()
        .map(|id| {
            tariffs
                .iter()
                .find(|t| t.id == *id)
                .map_or_else(|| format!("№{}", id.0), |t| t.name.clone())
        })
        .collect();
    serde_json::json!({
        "name": promotion.name,
        "percent": promotion.percent,
        "tariffs": names,
        "starts_at": stamp(promotion.starts_at, offset),
        "ends_at": stamp(promotion.ends_at, offset),
        "enabled": promotion.enabled,
    })
}

pub async fn running(db: &mut impl Db, now: DateTime<Utc>) -> ServiceResult<Vec<Promotion>> {
    Ok(db.promotions().running(now).await?)
}

pub async fn running_terms(
    db: &mut impl Db,
    tariff_id: TariffId,
    now: DateTime<Utc>,
) -> ServiceResult<Option<PromotionTerms>> {
    let list = running(db, now).await?;
    Ok(running_for(&list, tariff_id, now))
}

pub fn note(
    services: &Services,
    terms: Option<&PromotionTerms>,
    quote: &Quote,
    code_competed: bool,
) -> Option<PromotionNote> {
    let terms = terms?;
    let applied = promotion_line(quote).is_some();
    if !applied && !code_competed {
        return None;
    }
    Some(PromotionNote {
        id: terms.id,
        name: terms.name.clone(),
        percent: terms.percent,
        ends_at: terms.ends_at,
        until: until_text(
            terms.ends_at,
            services.settings().remna_offset(),
            lang::current(),
            services.now(),
        ),
        applied,
        code_skipped: applied && code_competed,
    })
}

pub fn label_offers(services: &Services, offers: &mut [PeriodOffer]) {
    let offset = services.settings().remna_offset();
    let locale = lang::current();
    let now = services.now();
    for offer in offers {
        if let Some(promotion) = offer.promotion.as_mut() {
            promotion.until = until_text(promotion.ends_at, offset, locale, now);
        }
    }
}

pub struct Promotions<'a> {
    services: &'a Services,
}

impl<'a> Promotions<'a> {
    pub fn new(services: &'a Services) -> Self {
        Promotions { services }
    }

    fn view(&self, promotion: Promotion) -> PromotionView {
        let state = promotion.state(self.services.now());
        PromotionView { promotion, state }
    }

    pub async fn list(&self) -> ServiceResult<PromotionsView> {
        let mut conn = self.services.store().conn().await?;
        let promotions = conn.promotions().list().await?;
        Ok(PromotionsView {
            promotions: promotions.into_iter().map(|p| self.view(p)).collect(),
            offset_minutes: self.services.settings().remna_utc_offset_minutes,
            now: self.services.now(),
        })
    }

    fn offset(&self) -> FixedOffset {
        self.services.settings().remna_offset()
    }

    async fn draft(
        &self,
        db: &mut impl Db,
        input: &PromotionInput,
    ) -> ServiceResult<(PromotionDraft, Vec<Tariff>)> {
        let draft = PromotionDraft {
            name: input.name.clone(),
            percent: input.percent,
            tariff_ids: input.tariff_ids.clone(),
            starts_at: input.starts_at,
            ends_at: input.ends_at,
            enabled: input.enabled,
        }
        .checked(self.services.now())
        .map_err(rejected)?;
        let tariffs = db.tariffs().list(true).await?;
        if let Some(missing) = draft
            .tariff_ids
            .iter()
            .find(|id| !tariffs.iter().any(|t| t.id == **id))
        {
            return Err(ServiceError::rejected(
                "promotion.invalid",
                format!(
                    "тарифа №{} нет: обновите страницу и выберите тарифы заново",
                    missing.0
                ),
            ));
        }
        Ok((draft, tariffs))
    }

    pub async fn create(
        &self,
        input: &PromotionInput,
        admin: AdminId,
    ) -> ServiceResult<PromotionView> {
        let mut tx = self.services.store().begin().await?;
        let (draft, tariffs) = self.draft(&mut tx, input).await?;
        let created = tx.promotions().create(&draft).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.promotion_created",
                "promotion",
                Some(created.id),
                details(&created, &tariffs, self.offset()),
            ))
            .await?;
        tx.commit().await?;
        Ok(self.view(created))
    }

    pub async fn update(
        &self,
        id: i64,
        input: &PromotionInput,
        admin: AdminId,
    ) -> ServiceResult<PromotionView> {
        let mut tx = self.services.store().begin().await?;
        let before = tx.promotions().get(id).await?;
        let (draft, tariffs) = self.draft(&mut tx, input).await?;
        let updated = tx.promotions().update(id, &draft).await?;
        let offset = self.offset();
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.promotion_updated",
                "promotion",
                Some(id),
                serde_json::json!({
                    "name": updated.name,
                    "from": details(&before, &tariffs, offset),
                    "to": details(&updated, &tariffs, offset),
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(self.view(updated))
    }

    pub async fn set_enabled(
        &self,
        id: i64,
        enabled: bool,
        admin: AdminId,
    ) -> ServiceResult<PromotionView> {
        let mut tx = self.services.store().begin().await?;
        let updated = tx.promotions().set_enabled(id, enabled).await?;
        let tariffs = tx.tariffs().list(true).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                if enabled {
                    "tariff.promotion_enabled"
                } else {
                    "tariff.promotion_disabled"
                },
                "promotion",
                Some(id),
                details(&updated, &tariffs, self.offset()),
            ))
            .await?;
        tx.commit().await?;
        Ok(self.view(updated))
    }

    pub async fn delete(&self, id: i64, admin: AdminId) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        let deleted = tx.promotions().delete(id).await?;
        let tariffs = tx.tariffs().list(true).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.promotion_deleted",
                "promotion",
                Some(id),
                details(&deleted, &tariffs, self.offset()),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
