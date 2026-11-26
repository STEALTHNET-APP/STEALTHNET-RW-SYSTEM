use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::TariffId;
use sn_domain::promotion::{Promotion, PromotionDraft};
use sqlx::PgConnection;

const COLUMNS: &str = "id, name, percent, tariff_ids, starts_at, ends_at, enabled";
const LIST_LIMIT: i64 = 200;

#[derive(sqlx::FromRow)]
struct PromotionRow {
    id: i64,
    name: String,
    percent: i16,
    tariff_ids: Vec<i64>,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    enabled: bool,
}

fn promotion_from(row: PromotionRow) -> StoreResult<Promotion> {
    Ok(Promotion {
        id: row.id,
        name: row.name,
        percent: u8::try_from(row.percent)
            .map_err(|_| StoreError::Decode("percent: вне диапазона".into()))?,
        tariff_ids: row.tariff_ids.into_iter().map(TariffId).collect(),
        starts_at: row.starts_at,
        ends_at: row.ends_at,
        enabled: row.enabled,
    })
}

fn ids(draft: &PromotionDraft) -> Vec<i64> {
    draft.tariff_ids.iter().map(|id| id.0).collect()
}

pub struct PromotionsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> PromotionsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        PromotionsRepo { conn }
    }

    pub async fn list(&mut self) -> StoreResult<Vec<Promotion>> {
        let rows: Vec<PromotionRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM tariff_promotions ORDER BY starts_at DESC, id DESC LIMIT $1"
        ))
        .bind(LIST_LIMIT)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(promotion_from).collect()
    }

    pub async fn get(&mut self, id: i64) -> StoreResult<Promotion> {
        let row: PromotionRow = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM tariff_promotions WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("акция"))?;
        promotion_from(row)
    }

    pub async fn running(&mut self, now: DateTime<Utc>) -> StoreResult<Vec<Promotion>> {
        let rows: Vec<PromotionRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM tariff_promotions \
             WHERE enabled AND starts_at <= $1 AND ends_at > $1 ORDER BY id"
        ))
        .bind(now)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(promotion_from).collect()
    }

    pub async fn create(&mut self, draft: &PromotionDraft) -> StoreResult<Promotion> {
        let row: PromotionRow = sqlx::query_as(&format!(
            "INSERT INTO tariff_promotions (name, percent, tariff_ids, starts_at, ends_at, enabled) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {COLUMNS}"
        ))
        .bind(&draft.name)
        .bind(i16::from(draft.percent))
        .bind(ids(draft))
        .bind(draft.starts_at)
        .bind(draft.ends_at)
        .bind(draft.enabled)
        .fetch_one(&mut *self.conn)
        .await?;
        promotion_from(row)
    }

    pub async fn update(&mut self, id: i64, draft: &PromotionDraft) -> StoreResult<Promotion> {
        let row: PromotionRow = sqlx::query_as(&format!(
            "UPDATE tariff_promotions SET name = $2, percent = $3, tariff_ids = $4, \
             starts_at = $5, ends_at = $6, enabled = $7, updated_at = now() \
             WHERE id = $1 RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(&draft.name)
        .bind(i16::from(draft.percent))
        .bind(ids(draft))
        .bind(draft.starts_at)
        .bind(draft.ends_at)
        .bind(draft.enabled)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("акция"))?;
        promotion_from(row)
    }

    pub async fn set_enabled(&mut self, id: i64, enabled: bool) -> StoreResult<Promotion> {
        let row: PromotionRow = sqlx::query_as(&format!(
            "UPDATE tariff_promotions SET enabled = $2, updated_at = now() \
             WHERE id = $1 RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(enabled)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("акция"))?;
        promotion_from(row)
    }

    pub async fn delete(&mut self, id: i64) -> StoreResult<Promotion> {
        let row: PromotionRow = sqlx::query_as(&format!(
            "DELETE FROM tariff_promotions WHERE id = $1 RETURNING {COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("акция"))?;
        promotion_from(row)
    }
}
