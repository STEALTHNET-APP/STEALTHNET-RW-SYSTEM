use super::client_filter::{SqlArg, SqlArgs};
use super::clients::{
    COLUMNS, ClientFilter, ClientListItem, ClientRow, ClientSegment, ClientsRepo, display_name,
    fold_case,
};
use super::settings::SettingsRepo;
use super::subscriptions::{PAID_LIKE, SubscriptionsRepo, effective_status};
use crate::error::StoreResult;
use chrono::{DateTime, Utc};
use sn_domain::client::Client;
use sn_domain::ids::SubscriptionId;
use sn_domain::money::Currency;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::settings::keys;
use sn_domain::subscription::Subscription;
use std::collections::HashMap;
use uuid::Uuid;

const MAX_QUERY_CHARS: usize = 120;
const MAX_WORDS: usize = 5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct SegmentCounts {
    pub total: i64,
    pub active: i64,
    pub trial: i64,
    pub no_subscription: i64,
    pub expiring_soon: i64,
    pub never_connected: i64,
    pub expired: i64,
    pub limited: i64,
    pub blocked: i64,
}

impl SegmentCounts {
    pub const fn get(&self, segment: Option<ClientSegment>) -> i64 {
        match segment {
            None => self.total,
            Some(ClientSegment::Active) => self.active,
            Some(ClientSegment::Trial) => self.trial,
            Some(ClientSegment::NoSubscription) => self.no_subscription,
            Some(ClientSegment::ExpiringSoon) => self.expiring_soon,
            Some(ClientSegment::NeverConnected) => self.never_connected,
            Some(ClientSegment::Expired) => self.expired,
            Some(ClientSegment::Limited) => self.limited,
            Some(ClientSegment::Blocked) => self.blocked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Needle {
    ShortUuid(String),
    Uuid(Uuid),
    Number { id: i64, telegram: String },
    Username(String),
    Email(String),
    Words { words: Vec<String>, exact: String },
}

pub(super) fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn needle(raw: &str) -> Option<Needle> {
    let query: String = raw.trim().chars().take(MAX_QUERY_CHARS).collect();
    if query.is_empty() {
        return None;
    }
    if query.contains('/') {
        let path = query.split(['?', '#']).next().unwrap_or_default();
        let last = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();
        if !last.is_empty() && !last.contains(':') {
            return Some(Needle::ShortUuid(last.to_string()));
        }
    }
    if let Some(name) = query.strip_prefix('@') {
        let name = name.trim().to_lowercase();
        return (!name.is_empty()).then_some(Needle::Username(name));
    }
    if let Ok(uuid) = Uuid::parse_str(&query) {
        return Some(Needle::Uuid(uuid));
    }
    if query.bytes().all(|b| b.is_ascii_digit())
        && let Ok(id) = query.parse::<i64>()
    {
        return Some(Needle::Number {
            id,
            telegram: id.to_string(),
        });
    }
    if query.contains('@') && !query.contains(char::is_whitespace) {
        return Some(Needle::Email(query.to_lowercase()));
    }
    let lowered = fold_case(&query);
    let words: Vec<String> = lowered
        .split_whitespace()
        .take(MAX_WORDS)
        .map(str::to_string)
        .collect();
    Some(Needle::Words {
        words,
        exact: query,
    })
}

fn needle_condition(needle: &Needle, args: &mut SqlArgs) -> String {
    match needle {
        Needle::ShortUuid(value) => {
            let v = args.push(SqlArg::Text(value.clone()));
            format!("c.id IN (SELECT q.client_id FROM subscriptions q WHERE q.short_uuid = {v})")
        }
        Needle::Uuid(value) => {
            let v = args.push(SqlArg::Text(value.to_string()));
            format!(
                "c.id IN (SELECT q.id FROM clients q WHERE q.public_id = {v}::uuid \
                 UNION ALL SELECT q.client_id FROM subscriptions q WHERE q.public_id = {v}::uuid)"
            )
        }
        Needle::Number { id, telegram } => {
            let n = args.push(SqlArg::Int(*id));
            let t = args.push(SqlArg::Text(telegram.clone()));
            format!(
                "c.id IN (SELECT {n}::bigint UNION ALL SELECT q.client_id FROM client_identities q \
                 WHERE q.kind = 'telegram' AND q.value = {t})"
            )
        }
        Needle::Username(value) => {
            let v = args.push(SqlArg::Text(format!("{}%", escape_like(value))));
            format!(
                "c.id IN (SELECT q.client_id FROM client_identities q WHERE q.kind = 'telegram' \
                 AND lower(q.meta ->> 'username') LIKE {v})"
            )
        }
        Needle::Email(value) => {
            let v = args.push(SqlArg::Text(format!("{}%", escape_like(value))));
            format!(
                "c.id IN (SELECT q.client_id FROM client_identities q WHERE q.kind = 'email' \
                 AND q.value LIKE {v})"
            )
        }
        Needle::Words { words, exact } => {
            let w = args.push(SqlArg::Texts(words.clone()));
            let e = args.push(SqlArg::Text(exact.clone()));
            format!(
                "c.id IN (SELECT q.client_id FROM subscriptions q WHERE q.short_uuid = {e} OR q.remna_username = {e} \
                 UNION ALL SELECT q.client_id FROM client_identities q \
                 WHERE NOT EXISTS (SELECT 1 FROM unnest({w}::text[]) w WHERE strpos(q.search_text, w) = 0))"
            )
        }
    }
}

fn filter_conditions(
    filter: &ClientFilter,
    args: &mut SqlArgs,
    now: DateTime<Utc>,
    currency: Currency,
) -> StoreResult<Vec<String>> {
    let mut conditions = vec!["c.deleted_at IS NULL".to_string()];
    conditions.extend(filter.conditions.conditions(args, now, currency)?);
    if let Some(needle) = filter.query.as_deref().and_then(needle) {
        conditions.push(needle_condition(&needle, args));
    }
    Ok(conditions)
}

pub(crate) fn priority(alias: &str) -> String {
    format!(
        "CASE {} WHEN 'active' THEN 0 WHEN 'limited' THEN 1 WHEN 'disabled' THEN 2 WHEN 'expired' THEN 3 ELSE 4 END, \
         {alias}.expire_at DESC NULLS LAST, {alias}.id DESC",
        effective_status(alias, "$1")
    )
}

fn subscription_condition(segment: ClientSegment, alias: &str) -> String {
    let est = effective_status(alias, "$1");
    match segment {
        ClientSegment::Active => format!("{est} = 'active' AND {alias}.kind <> 'trial'"),
        ClientSegment::Trial => format!("{est} = 'active' AND {alias}.kind = 'trial'"),
        ClientSegment::ExpiringSoon => {
            format!("{est} = 'active' AND {alias}.expire_at > $1 AND {alias}.expire_at <= $2")
        }
        ClientSegment::NeverConnected => {
            format!("{est} = 'active' AND {alias}.first_connected_at IS NULL")
        }
        ClientSegment::Expired => format!("{est} = 'expired'"),
        ClientSegment::Limited => format!("{est} = 'limited'"),
        ClientSegment::Blocked => format!("{est} = 'disabled'"),
        ClientSegment::NoSubscription => format!("{est} IS NULL"),
    }
}

fn client_condition(segment: ClientSegment, alias: &str) -> String {
    let own = subscription_condition(segment, alias);
    match segment {
        ClientSegment::Blocked => format!("(c.status = 'blocked' OR {own})"),
        _ => format!("c.status = 'active' AND {own}"),
    }
}

pub(super) fn segment_condition(segment: ClientSegment) -> String {
    let primary = client_condition(segment, "s");
    match segment {
        ClientSegment::Blocked | ClientSegment::NoSubscription => primary,
        _ => format!(
            "{primary} AND EXISTS (SELECT 1 FROM subscriptions x2 WHERE x2.client_id = c.id \
             AND x2.ended_at IS NULL AND x2.remna_missing_at IS NULL AND {})",
            subscription_condition(segment, "x2")
        ),
    }
}

#[derive(sqlx::FromRow)]
struct ListRow {
    #[sqlx(flatten)]
    client: ClientRow,
    subscription_id: Option<i64>,
    telegram_id: Option<String>,
    telegram_username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    email: Option<String>,
    balance_minor: i64,
    paid_total_minor: i64,
}

impl<'c> ClientsRepo<'c> {
    pub async fn search(
        &mut self,
        filter: &ClientFilter,
        page: PageRequest,
    ) -> StoreResult<Page<ClientListItem>> {
        let currency: Currency = SettingsRepo::new(&mut *self.conn)
            .get_as(keys::CURRENCY)
            .await?
            .unwrap_or_default();
        self.search_at(filter, page, Utc::now(), currency).await
    }

    pub async fn search_at(
        &mut self,
        filter: &ClientFilter,
        page: PageRequest,
        now: DateTime<Utc>,
        currency: Currency,
    ) -> StoreResult<Page<ClientListItem>> {
        let mut args = SqlArgs::with_clock(now)?;
        args.push(SqlArg::Text(currency.code().into()));
        let after = args.push(SqlArg::Int(page.after.unwrap_or(0)));
        let limit = args.push(SqlArg::Int(page.fetch_limit()));
        let conditions = filter_conditions(filter, &mut args, now, currency)?;
        let sql = format!(
            "SELECT {COLUMNS}, s.id AS subscription_id, tg.value AS telegram_id, \
             tg.meta ->> 'username' AS telegram_username, tg.meta ->> 'first_name' AS first_name, \
             tg.meta ->> 'last_name' AS last_name, em.value AS email, \
             COALESCE((SELECT b.amount_minor FROM client_balances b WHERE b.client_id = c.id \
                 AND b.wallet = 'main' AND b.currency = $3), 0) AS balance_minor, \
             COALESCE((SELECT sum(p.amount_minor - p.refunded_minor) FROM payments p WHERE p.client_id = c.id \
                 AND p.currency = $3 AND p.status IN {PAID_LIKE} AND p.provider <> 'balance'), 0)::bigint AS paid_total_minor \
             FROM clients c \
             LEFT JOIN LATERAL (SELECT x.* FROM subscriptions x WHERE x.client_id = c.id AND x.ended_at IS NULL \
                 AND x.remna_missing_at IS NULL ORDER BY {order} LIMIT 1) s ON true \
             LEFT JOIN LATERAL (SELECT i.value, i.meta FROM client_identities i WHERE i.client_id = c.id \
                 AND i.kind = 'telegram' ORDER BY i.id LIMIT 1) tg ON true \
             LEFT JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = c.id \
                 AND i.kind = 'email' ORDER BY i.id LIMIT 1) em ON true \
             WHERE {conditions} AND ({after} = 0 OR (c.created_at, c.id) < \
                 (SELECT k.created_at, k.id FROM clients k WHERE k.id = {after})) \
             ORDER BY c.created_at DESC, c.id DESC LIMIT {limit}",
            order = priority("x"),
            conditions = conditions.join(" AND "),
        );
        let rows = args
            .bind_as(sqlx::query_as::<_, ListRow>(&sql).persistent(false))
            .fetch_all(&mut *self.conn)
            .await?;
        let ids: Vec<SubscriptionId> = rows
            .iter()
            .filter_map(|row| row.subscription_id.map(SubscriptionId))
            .collect();
        let mut subscriptions: HashMap<SubscriptionId, Subscription> =
            SubscriptionsRepo::new(&mut *self.conn)
                .by_ids(&ids)
                .await?
                .into_iter()
                .map(|s| (s.id, s))
                .collect();
        let items = rows
            .into_iter()
            .map(|row| {
                Ok(ClientListItem {
                    client: Client::try_from(row.client)?,
                    name: display_name(row.first_name.as_deref(), row.last_name.as_deref()),
                    telegram_id: row.telegram_id,
                    telegram_username: row.telegram_username,
                    email: row.email,
                    subscription: row
                        .subscription_id
                        .and_then(|id| subscriptions.remove(&SubscriptionId(id))),
                    balance_minor: row.balance_minor,
                    paid_total_minor: row.paid_total_minor,
                })
            })
            .collect::<StoreResult<Vec<ClientListItem>>>()?;
        Ok(Page::from_overfetch(items, page, |item| item.client.id.0))
    }

    pub async fn segment_counts(&mut self, now: DateTime<Utc>) -> StoreResult<SegmentCounts> {
        self.segment_counts_where(&ClientFilter::default(), now, Currency::default())
            .await
    }

    pub async fn segment_counts_where(
        &mut self,
        filter: &ClientFilter,
        now: DateTime<Utc>,
        currency: Currency,
    ) -> StoreResult<SegmentCounts> {
        let mut args = SqlArgs::with_clock(now)?;
        let mut scope = filter.clone();
        scope.conditions.status = None;
        let conditions = filter_conditions(&scope, &mut args, now, currency)?;
        let count = |segment: ClientSegment| {
            format!(
                "count(*) FILTER (WHERE {}) AS {}",
                client_condition(segment, "s"),
                segment.as_str()
            )
        };
        let sql = format!(
            "SELECT count(*) AS total, {} FROM clients c \
             LEFT JOIN (SELECT DISTINCT ON (x.client_id) x.* FROM subscriptions x \
                 WHERE x.ended_at IS NULL AND x.remna_missing_at IS NULL \
                 ORDER BY x.client_id, {order}) s ON s.client_id = c.id \
             WHERE {conditions}",
            ClientSegment::ALL.map(count).join(", "),
            order = priority("x"),
            conditions = conditions.join(" AND "),
        );
        let row: SegmentCountsRow = args
            .bind_as(sqlx::query_as(&sql).persistent(false))
            .fetch_one(&mut *self.conn)
            .await?;
        Ok(SegmentCounts {
            total: row.total,
            active: row.active,
            trial: row.trial,
            no_subscription: row.no_subscription,
            expiring_soon: row.expiring_soon,
            never_connected: row.never_connected,
            expired: row.expired,
            limited: row.limited,
            blocked: row.blocked,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SegmentCountsRow {
    total: i64,
    active: i64,
    trial: i64,
    no_subscription: i64,
    expiring_soon: i64,
    never_connected: i64,
    expired: i64,
    limited: i64,
    blocked: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn строка_поиска_разбирается_по_виду() {
        assert_eq!(
            needle("https://sub.example.com/sub/AbC123xyz/?x=1"),
            Some(Needle::ShortUuid("AbC123xyz".into()))
        );
        assert_eq!(
            needle("  @Morozov_A "),
            Some(Needle::Username("morozov_a".into()))
        );
        assert_eq!(needle("@"), None);
        assert_eq!(needle("   "), None);
        assert_eq!(
            needle("5093118422"),
            Some(Needle::Number {
                id: 5_093_118_422,
                telegram: "5093118422".into()
            })
        );
        assert_eq!(
            needle("A.Morozov@Example.com"),
            Some(Needle::Email("a.morozov@example.com".into()))
        );
        let id = Uuid::from_u128(42);
        assert_eq!(needle(&id.to_string()), Some(Needle::Uuid(id)));
        assert_eq!(
            needle("Алексей  МОРОЗОВ"),
            Some(Needle::Words {
                words: vec!["алексей".into(), "морозов".into()],
                exact: "Алексей  МОРОЗОВ".into()
            })
        );
        assert!(matches!(
            needle("99999999999999999999"),
            Some(Needle::Words { .. })
        ));
    }

    #[test]
    fn буква_ё_и_регистр_не_мешают_поиску() {
        assert_eq!(
            needle("Алёна ЁЛКИНА"),
            Some(Needle::Words {
                words: vec!["алена".into(), "елкина".into()],
                exact: "Алёна ЁЛКИНА".into()
            })
        );
    }

    #[test]
    fn сегмент_пишется_и_читается_одинаково() {
        for segment in ClientSegment::ALL {
            assert_eq!(serde_json::to_value(segment).unwrap(), segment.as_str());
            assert_eq!(ClientSegment::parse(segment.as_str()).unwrap(), segment);
        }
        assert!(ClientSegment::parse("все").is_err());
        let filter: ClientFilter =
            serde_json::from_value(serde_json::json!({"segment": "expiring_soon"})).unwrap();
        assert_eq!(filter.conditions.status, Some(ClientSegment::ExpiringSoon));
        assert_eq!(filter.query, None);
    }

    #[test]
    fn спецсимволы_like_экранируются() {
        assert_eq!(escape_like("a_b%c\\d"), "a\\_b\\%c\\\\d");
    }

    #[test]
    fn имя_клиента_из_telegram() {
        assert_eq!(
            display_name(Some("Алексей"), Some(" Морозов ")),
            Some("Алексей Морозов".into())
        );
        assert_eq!(display_name(Some("  "), None), None);
    }
}
