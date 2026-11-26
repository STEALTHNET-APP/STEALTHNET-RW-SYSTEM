use chrono::{DateTime, Utc};
use sn_domain::money::Currency;
use sn_store::{Db, StoreResult};

pub const CRYPTO_PROVIDERS: [&str; 4] = ["cryptopay", "heleket", "cryptomus", "gram"];
pub const CRYPTO_METHODS: [&str; 5] = ["crypto", "usdt", "ton", "btc", "trc20"];

pub fn crypto_cond(a: &str) -> String {
    let providers = CRYPTO_PROVIDERS
        .iter()
        .map(|p| format!("'{p}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let methods = CRYPTO_METHODS
        .iter()
        .map(|m| format!("{a}.method ILIKE '%{m}%'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("({a}.provider IN ({providers}) OR {methods})")
}

const CLIENT_SELECT: &str = "SELECT c.id, c.created_at, c.registration_source, \
       COALESCE((SELECT i.kind FROM client_identities i WHERE i.client_id = c.id ORDER BY i.created_at, i.id LIMIT 1), ''), \
       CASE WHEN u.client_id IS NOT NULL THEN 'utm' WHEN pl.link_id IS NOT NULL THEN 'promo' \
            WHEN r.client_id IS NOT NULL THEN 'ref' WHEN g.id IS NOT NULL THEN 'gift' ELSE 'none' END, \
       CASE WHEN u.client_id IS NOT NULL THEN COALESCE(u.source, '') \
            WHEN pl.link_id IS NOT NULL THEN pl.link_id::text ELSE '' END, \
       COALESCE(u.medium, ''), COALESCE(u.campaign, ''), c.deleted_at IS NOT NULL \
     FROM clients c \
     LEFT JOIN client_utm u ON u.client_id = c.id \
     LEFT JOIN LATERAL (SELECT a.link_id FROM promo_link_activations a \
        WHERE a.client_id = c.id AND a.activated_at < c.created_at + interval '1 day' ORDER BY a.id LIMIT 1) pl ON true \
     LEFT JOIN referral_links r ON r.client_id = c.id \
     LEFT JOIN LATERAL (SELECT gi.id FROM gifts gi \
        WHERE gi.redeemed_by = c.id AND gi.redeemed_at < c.created_at + interval '1 day' ORDER BY gi.id LIMIT 1) g ON true";

const CLIENT_UPSERT: &str = "INSERT INTO analytics_clients \
       (client_id, created_at, reg_channel, login_kind, source_type, source_key, utm_medium, utm_campaign, deleted) ";

const CLIENT_CONFLICT: &str = " ON CONFLICT (client_id) DO UPDATE SET created_at = EXCLUDED.created_at, \
       reg_channel = EXCLUDED.reg_channel, login_kind = EXCLUDED.login_kind, source_type = EXCLUDED.source_type, \
       source_key = EXCLUDED.source_key, utm_medium = EXCLUDED.utm_medium, utm_campaign = EXCLUDED.utm_campaign, \
       deleted = EXCLUDED.deleted";

const PAYMENT_INSERT: &str = "INSERT INTO analytics_payments (payment_id, client_id, order_id, paid_at, kind, item, \
       tariff_id, category_id, period_days, provider, method, channel, stars, amount_minor, refunded_minor, \
       promo_minor, promo_code, discount_minor, addon_minor) \
     SELECT p.id, p.client_id, p.order_id, p.paid_at, o.kind, \
       CASE WHEN o.kind = 'module' THEN COALESCE(o.intent ->> 'module', '') || ':' || COALESCE(o.intent ->> 'kind', '') ELSE '' END, \
       COALESCE(CASE WHEN jsonb_typeof(o.snapshot -> 'tariff_id') = 'number' THEN (o.snapshot ->> 'tariff_id')::bigint END, ln.tariff_id), \
       COALESCE(CASE WHEN jsonb_typeof(o.snapshot -> 'category_id') = 'number' THEN (o.snapshot ->> 'category_id')::bigint END, t.category_id), \
       COALESCE(CASE WHEN jsonb_typeof(o.snapshot -> 'days') = 'number' THEN (o.snapshot ->> 'days')::int END, ln.days), \
       p.provider, COALESCE(p.pay_method, ''), COALESCE(o.channel, ''), p.currency = 'XTR', \
       p.amount_minor, p.refunded_minor, COALESCE(ln.promo, 0), ln.code, COALESCE(ln.other, 0), COALESCE(ln.addon, 0) \
     FROM payments p \
     JOIN orders o ON o.id = p.order_id \
     LEFT JOIN LATERAL ( \
       SELECT \
         max(CASE WHEN l #>> '{kind,type}' = 'period' AND jsonb_typeof(l #> '{kind,tariff_id}') = 'number' \
             THEN (l #>> '{kind,tariff_id}')::bigint END) AS tariff_id, \
         max(CASE WHEN l #>> '{kind,type}' = 'period' AND jsonb_typeof(l #> '{kind,days}') = 'number' \
             THEN (l #>> '{kind,days}')::int END) AS days, \
         sum(CASE WHEN l #>> '{kind,type}' = 'adjustment' AND l #>> '{kind,source,type}' = 'promo' \
             AND jsonb_typeof(l #> '{amount,minor}') = 'number' THEN -(l #>> '{amount,minor}')::bigint END)::bigint AS promo, \
         max(CASE WHEN l #>> '{kind,source,type}' = 'promo' THEN l #>> '{kind,source,code}' END) AS code, \
         sum(CASE WHEN ((l #>> '{kind,type}' = 'adjustment' AND l #>> '{kind,source,type}' <> 'promo') \
             OR l #>> '{kind,type}' IN ('device_discount', 'promotion')) AND jsonb_typeof(l #> '{amount,minor}') = 'number' \
             THEN -(l #>> '{amount,minor}')::bigint END)::bigint AS other, \
         sum(CASE WHEN l #>> '{kind,type}' IN ('extra_devices', 'device_addon', 'traffic_addon') \
             AND jsonb_typeof(l #> '{amount,minor}') = 'number' THEN (l #>> '{amount,minor}')::bigint END)::bigint AS addon \
       FROM jsonb_array_elements(CASE WHEN jsonb_typeof(o.quote -> 'lines') = 'array' THEN o.quote -> 'lines' ELSE '[]'::jsonb END) l \
     ) ln ON true \
     LEFT JOIN tariffs t ON t.id = COALESCE(CASE WHEN jsonb_typeof(o.snapshot -> 'tariff_id') = 'number' \
       THEN (o.snapshot ->> 'tariff_id')::bigint END, ln.tariff_id) \
     WHERE p.status IN ('paid', 'partially_refunded', 'refunded') AND p.paid_at IS NOT NULL \
       AND (p.currency = 'XTR' OR p.currency = $CUR)";

const ATTEMPT_INSERT: &str = "INSERT INTO analytics_attempts (payment_id, client_id, created_at, provider, method, \
       channel, status, expires_at, wait_seconds, amount_minor, refunded_minor, reason) \
     SELECT p.id, p.client_id, p.created_at, p.provider, COALESCE(p.pay_method, ''), COALESCE(o.channel, ''), \
       p.status, p.expires_at, \
       CASE WHEN p.status IN ('paid', 'partially_refunded', 'refunded') AND p.paid_at IS NOT NULL \
         THEN greatest(0, floor(extract(epoch FROM p.paid_at - p.created_at)))::bigint END, \
       CASE WHEN p.currency = $CUR THEN p.amount_minor ELSE 0 END, \
       CASE WHEN p.currency = $CUR THEN p.refunded_minor ELSE 0 END, \
       CASE WHEN p.status = 'failed' THEN left(NULLIF(btrim(p.failure_reason), ''), 120) END \
     FROM payments p \
     LEFT JOIN orders o ON o.id = p.order_id \
     WHERE p.provider NOT IN ('balance', 'manual')";

const TRIALS: &str = "UPDATE analytics_clients ac SET trial_at = t.first \
     FROM (SELECT o.client_id, min(o.paid_at) AS first FROM orders o \
           WHERE o.kind = 'trial' AND o.paid_at IS NOT NULL AND $FILTER GROUP BY 1) t \
     WHERE ac.client_id = t.client_id";

const FIRST_ISLANDS: &str = "UPDATE analytics_clients ac SET first_island_at = f.starts_at, kept30 = f.kept \
     FROM (SELECT c.id AS client_id, i.starts_at, COALESCE(EXISTS (SELECT 1 FROM analytics_islands x \
             WHERE x.client_id = c.id AND x.starts_at <= i.starts_at + interval '30 days' \
               AND x.ends_at > i.starts_at + interval '30 days'), false) AS kept \
           FROM $CLIENTS LEFT JOIN analytics_islands i ON i.client_id = c.id AND i.seq = 1) f \
     WHERE ac.client_id = f.client_id";

const CONNECTED: &str = "UPDATE analytics_clients ac SET connected_at = s.first \
     FROM (SELECT sb.client_id, min(sb.first_connected_at) AS first FROM subscriptions sb \
           WHERE sb.first_connected_at IS NOT NULL AND $FILTER GROUP BY 1) s \
     WHERE ac.client_id = s.client_id AND ac.connected_at IS DISTINCT FROM s.first";

const PAID: &str = "provider <> 'balance' AND kind <> 'trial' AND amount_minor > 0";

pub async fn rebuild_all(db: &mut impl Db, currency: Currency) -> StoreResult<()> {
    sqlx::query("DELETE FROM analytics_clients")
        .execute(db.conn())
        .await?;
    sqlx::query("DELETE FROM analytics_payments")
        .execute(db.conn())
        .await?;
    sqlx::query("DELETE FROM analytics_attempts")
        .execute(db.conn())
        .await?;
    sqlx::query(&format!(
        "{CLIENT_UPSERT}{CLIENT_SELECT} WHERE true{CLIENT_CONFLICT}"
    ))
    .execute(db.conn())
    .await?;
    sqlx::query(&PAYMENT_INSERT.replace("$CUR", "$1"))
        .bind(currency.code())
        .execute(db.conn())
        .await?;
    sqlx::query(&ATTEMPT_INSERT.replace("$CUR", "$1"))
        .bind(currency.code())
        .execute(db.conn())
        .await?;
    sqlx::query(&format!(
        "UPDATE analytics_clients ac SET first_paid_at = f.first \
         FROM (SELECT client_id, min(paid_at) AS first FROM analytics_payments \
               WHERE {PAID} GROUP BY client_id) f \
         WHERE ac.client_id = f.client_id"
    ))
    .execute(db.conn())
    .await?;
    sqlx::query(&TRIALS.replace("$FILTER", "true"))
        .execute(db.conn())
        .await?;
    sqlx::query(&CONNECTED.replace("$FILTER", "true"))
        .execute(db.conn())
        .await?;
    sqlx::query(&FIRST_ISLANDS.replace(
        "$CLIENTS",
        "(SELECT DISTINCT client_id AS id FROM analytics_islands) c",
    ))
    .execute(db.conn())
    .await?;
    sqlx::query("ANALYZE analytics_clients, analytics_payments, analytics_attempts")
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn refresh_since(
    db: &mut impl Db,
    since: DateTime<Utc>,
    currency: Currency,
    islands: &[i64],
) -> StoreResult<usize> {
    sqlx::query(&format!(
        "{CLIENT_UPSERT}{CLIENT_SELECT} WHERE c.created_at > $1 - interval '25 hours' OR c.updated_at > $1{CLIENT_CONFLICT}"
    ))
    .bind(since)
    .execute(db.conn())
    .await?;
    sqlx::query(&TRIALS.replace(
        "$FILTER",
        "o.client_id IN (SELECT client_id FROM orders WHERE kind = 'trial' AND updated_at > $1)",
    ))
    .bind(since)
    .execute(db.conn())
    .await?;
    sqlx::query(&CONNECTED.replace(
        "$FILTER",
        "sb.client_id IN (SELECT client_id FROM subscriptions \
           WHERE first_connected_at > $1 - interval '1 day')",
    ))
    .bind(since)
    .execute(db.conn())
    .await?;
    if !islands.is_empty() {
        sqlx::query(&FIRST_ISLANDS.replace("$CLIENTS", "unnest($1::bigint[]) c(id)"))
            .bind(islands)
            .execute(db.conn())
            .await?;
    }
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM payments WHERE updated_at > $1")
        .bind(since)
        .fetch_all(db.conn())
        .await?;
    if ids.is_empty() {
        return Ok(0);
    }
    sqlx::query("DELETE FROM analytics_payments WHERE payment_id = ANY($1)")
        .bind(&ids)
        .execute(db.conn())
        .await?;
    sqlx::query("DELETE FROM analytics_attempts WHERE payment_id = ANY($1)")
        .bind(&ids)
        .execute(db.conn())
        .await?;
    sqlx::query(&format!(
        "{} AND p.id = ANY($1)",
        ATTEMPT_INSERT.replace("$CUR", "$2")
    ))
    .bind(&ids)
    .bind(currency.code())
    .execute(db.conn())
    .await?;
    sqlx::query(&format!(
        "{} AND p.id = ANY($1)",
        PAYMENT_INSERT.replace("$CUR", "$2")
    ))
    .bind(&ids)
    .bind(currency.code())
    .execute(db.conn())
    .await?;
    sqlx::query(&format!(
        "UPDATE analytics_clients ac SET first_paid_at = f.first \
         FROM (SELECT client_id, min(paid_at) AS first FROM analytics_payments \
               WHERE {PAID} AND client_id IN (SELECT client_id FROM payments WHERE id = ANY($1)) \
               GROUP BY client_id) f \
         WHERE ac.client_id = f.client_id"
    ))
    .bind(&ids)
    .execute(db.conn())
    .await?;
    Ok(ids.len())
}
