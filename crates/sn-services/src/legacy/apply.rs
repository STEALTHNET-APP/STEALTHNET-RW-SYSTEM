use super::check::{SubIssues, identity_owners, legacy_map, links};
use super::matcher::{Matched, RemnaIndex};
use super::model::*;
use super::runs::{self, IssueRow, Outcome, Progress, Stage};
use crate::Services;
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use sn_domain::client::{IdentityKind, NewIdentity, RegistrationSource};
use sn_domain::ids::{AdminId, CategoryId, ClientId, SquadId, SubscriptionId, TariffId};
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Currency, Money, Stars};
use sn_domain::order::{OrderKind, PurchaseIntent};
use sn_domain::pricing::{Quote, StarsTotal};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices, check_device_discounts};
use sn_domain::traffic::TrafficPolicy;
use sn_store::{Db, NewCategory, NewPeriod, NewTariff, StoreError, Tx};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const BATCH: usize = 400;
const PAYMENT_BATCH: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    Created,
    Merged,
    Updated,
}

pub struct Applier<'a> {
    services: &'a Services,
    legacy: &'a Legacy,
    index: &'a RemnaIndex,
    matched: Vec<Matched>,
    run_id: i64,
    run_key: Uuid,
    admin: Option<AdminId>,
    switch_currency: bool,
    issues: Vec<IssueRow>,
    outcome: Outcome,
    categories: HashMap<String, i64>,
    tariffs: HashMap<String, i64>,
    tariff_names: HashMap<i64, String>,
    periods: HashMap<String, i64>,
    periods_by_days: HashMap<(i64, u32), i64>,
    clients: HashMap<String, i64>,
    subs: HashMap<String, i64>,
    notify: Option<super::Notify>,
}

fn src_entity(source: Source, kind: &str) -> String {
    source.entity(kind)
}

async fn alive(tx: &mut Tx, table: &str, id: i64) -> ServiceResult<bool> {
    let sql = match table {
        "tariff_categories" => {
            "SELECT EXISTS (SELECT 1 FROM tariff_categories WHERE id = $1 AND deleted_at IS NULL)"
        }
        "tariffs" => "SELECT EXISTS (SELECT 1 FROM tariffs WHERE id = $1 AND deleted_at IS NULL)",
        "clients" => "SELECT EXISTS (SELECT 1 FROM clients WHERE id = $1 AND deleted_at IS NULL)",
        "subscriptions" => "SELECT EXISTS (SELECT 1 FROM subscriptions WHERE id = $1)",
        "tickets" => "SELECT EXISTS (SELECT 1 FROM tickets WHERE id = $1)",
        "promo_codes" => "SELECT EXISTS (SELECT 1 FROM promo_codes WHERE id = $1)",
        _ => return Ok(false),
    };
    Ok(sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(tx.conn())
        .await?)
}

async fn remember(
    tx: &mut Tx,
    entity: &str,
    rows: &[(String, i64, serde_json::Value)],
    run_id: i64,
) -> ServiceResult<()> {
    for chunk in rows.chunks(2000) {
        let old: Vec<&str> = chunk.iter().map(|r| r.0.as_str()).collect();
        let new: Vec<i64> = chunk.iter().map(|r| r.1).collect();
        let data: Vec<serde_json::Value> = chunk.iter().map(|r| r.2.clone()).collect();
        sqlx::query(
            "INSERT INTO legacy_ids (entity, old_id, new_id, data, run_id, updated_at) \
             SELECT $1, o, n, d, $5, now() FROM unnest($2::text[], $3::bigint[], $4::jsonb[]) AS t(o, n, d) \
             ON CONFLICT (entity, old_id) DO UPDATE SET new_id = EXCLUDED.new_id, data = EXCLUDED.data, \
             run_id = EXCLUDED.run_id, updated_at = now()",
        )
        .bind(entity)
        .bind(&old)
        .bind(&new)
        .bind(&data)
        .bind(run_id)
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

fn trimmed(value: &str, max: usize) -> String {
    value.trim().chars().take(max).collect()
}

fn utm_value(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(|v| trimmed(v, 64))
        .filter(|v| !v.is_empty())
}

fn referral_code(raw: &str) -> Option<String> {
    let code: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (4..=32).contains(&code.len()).then_some(code)
}

impl<'a> Applier<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        services: &'a Services,
        legacy: &'a Legacy,
        index: &'a RemnaIndex,
        matched: Vec<Matched>,
        run_id: i64,
        run_key: Uuid,
        admin: Option<AdminId>,
        switch_currency: bool,
        notify: Option<super::Notify>,
    ) -> Self {
        Applier {
            services,
            legacy,
            index,
            matched,
            run_id,
            run_key,
            admin,
            switch_currency,
            issues: Vec::new(),
            outcome: Outcome::default(),
            categories: HashMap::new(),
            tariffs: HashMap::new(),
            tariff_names: HashMap::new(),
            periods: HashMap::new(),
            periods_by_days: HashMap::new(),
            clients: HashMap::new(),
            subs: HashMap::new(),
            notify,
        }
    }

    fn source(&self) -> Source {
        self.legacy.source
    }

    fn issue(
        &mut self,
        code: &str,
        entity: &str,
        old_id: Option<&str>,
        who: Option<String>,
        details: impl Into<String>,
    ) {
        self.issues.push(IssueRow {
            stage: "apply",
            code: code.to_string(),
            entity: entity.to_string(),
            old_id: old_id.map(str::to_string),
            who,
            details: details.into(),
        });
    }

    async fn progress(&self, stage: Stage, done: usize, total: usize) -> ServiceResult<()> {
        let progress = Progress {
            stage,
            done: done as u64,
            total: total as u64,
        };
        if let Some(notify) = &self.notify {
            notify(&progress);
        }
        let mut conn = self.services.store().conn().await?;
        runs::set_progress(&mut conn, self.run_id, &progress).await?;
        drop(conn);
        tokio::task::yield_now().await;
        Ok(())
    }

    async fn flush_issues(&mut self) -> ServiceResult<()> {
        if self.issues.is_empty() {
            return Ok(());
        }
        let issues = std::mem::take(&mut self.issues);
        self.outcome.issues += issues.len() as u64;
        let mut conn = self.services.store().conn().await?;
        runs::add_issues(&mut conn, self.run_id, &issues).await
    }

    pub async fn run(mut self) -> ServiceResult<Outcome> {
        {
            let mut conn = self.services.store().conn().await?;
            sqlx::query(
                "DELETE FROM import_issues WHERE run_id = $1 AND (stage = 'apply' \
                 OR code IN ('subscription_missing', 'subscription_ambiguous', 'subscription_taken', 'subscription_shared'))",
            )
            .bind(self.run_id)
            .execute(conn.conn())
            .await?;
        }
        if self.switch_currency {
            self.switch().await?;
        }
        self.tariffs().await?;
        self.flush_issues().await?;
        self.clients().await?;
        self.flush_issues().await?;
        self.subscriptions().await?;
        self.flush_issues().await?;
        self.payments().await?;
        self.flush_issues().await?;
        self.promos().await?;
        self.flush_issues().await?;
        self.referrals().await?;
        self.flush_issues().await?;
        self.tickets().await?;
        self.flush_issues().await?;
        self.extras().await?;
        self.flush_issues().await?;
        self.finish().await?;
        self.flush_issues().await?;
        Ok(self.outcome)
    }

    async fn switch(&mut self) -> ServiceResult<()> {
        let currency = self.legacy.currency;
        if self.services.settings().currency == currency {
            return Ok(());
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(sn_domain::settings::keys::CURRENCY, &currency, self.admin)
            .await?;
        let (actor_kind, actor_id) = match self.admin {
            Some(admin) => (sn_store::ActorKind::Admin, Some(admin.0)),
            None => (sn_store::ActorKind::System, None),
        };
        tx.audit()
            .record(&sn_store::AuditEntry {
                actor_kind,
                actor_id,
                action: "settings.currency".into(),
                entity_kind: Some("import".into()),
                entity_id: Some(self.run_id),
                details: serde_json::json!({ "currency": currency, "reason": "import" }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        self.services.reload().await?;
        self.services.notify_settings_changed().await?;
        self.outcome.currency_switched = true;
        Ok(())
    }

    async fn known_squads(&self) -> Option<HashSet<uuid::Uuid>> {
        let api = self.services.remna().ok()?;
        let squads = api.internal_squads().await.ok()?;
        Some(squads.internal_squads.iter().map(|s| s.uuid).collect())
    }

    async fn tariffs(&mut self) -> ServiceResult<()> {
        let source = self.source();
        let total = self.legacy.tariffs.len();
        self.progress(Stage::Tariffs, 0, total).await?;
        let cat_map = legacy_map(self.services, &src_entity(source, "category")).await?;
        let tariff_map = legacy_map(self.services, &src_entity(source, "tariff")).await?;
        let period_map = legacy_map(self.services, &src_entity(source, "period")).await?;
        let squads = self.known_squads().await;
        let mut tx = self.services.store().begin().await?;
        let mut cat_rows = Vec::new();
        for category in &self.legacy.categories {
            let limit = self.legacy.category_limits.get(&category.old_id).copied();
            if let Some((id, data)) = cat_map.get(&category.old_id)
                && alive(&mut tx, "tariff_categories", *id).await?
            {
                self.categories.insert(category.old_id.clone(), *id);
                if let Some(limit) = limit
                    && data.get("limit").is_none()
                {
                    let current = tx.tariffs().category(CategoryId(*id)).await?;
                    if current.subscription_limit.is_none() {
                        tx.tariffs()
                            .set_subscription_limit(CategoryId(*id), Some(limit))
                            .await?;
                    }
                    cat_rows.push((
                        category.old_id.clone(),
                        *id,
                        serde_json::json!({ "limit": limit }),
                    ));
                }
                continue;
            }
            let created = tx
                .tariffs()
                .create_category(&NewCategory {
                    name: trimmed(&category.name, 64),
                    sort: category.sort,
                    visible: true,
                })
                .await?;
            if let Some(limit) = limit {
                tx.tariffs()
                    .set_subscription_limit(created.id, Some(limit))
                    .await?;
            }
            self.categories
                .insert(category.old_id.clone(), created.id.0);
            let data = match limit {
                Some(limit) => serde_json::json!({ "limit": limit }),
                None => serde_json::json!({}),
            };
            cat_rows.push((category.old_id.clone(), created.id.0, data));
        }
        remember(
            &mut tx,
            &src_entity(source, "category"),
            &cat_rows,
            self.run_id,
        )
        .await?;
        let mut tariff_rows = Vec::new();
        let mut period_rows = Vec::new();
        let mut fallback: Option<i64> = None;
        for (done, tariff) in self.legacy.tariffs.iter().enumerate() {
            let ladder = self.legacy.device_discounts.get(&tariff.old_id);
            if let Some((id, data)) = tariff_map.get(&tariff.old_id)
                && alive(&mut tx, "tariffs", *id).await?
            {
                if let Some(ladder) = ladder
                    && data.get("discounts").is_none()
                {
                    let current = tx.tariffs().get(TariffId(*id)).await?;
                    let max = current.devices.extra.as_ref().map_or(0, |e| e.max);
                    if current.device_discounts.is_empty()
                        && check_device_discounts(ladder, max).is_ok()
                    {
                        tx.tariffs()
                            .set_device_discounts(TariffId(*id), ladder)
                            .await?;
                    }
                    let mut marked = data.clone();
                    if let Some(object) = marked.as_object_mut() {
                        object.insert("discounts".into(), serde_json::Value::Bool(true));
                    }
                    tariff_rows.push((tariff.old_id.clone(), *id, marked));
                }
                let periods: Vec<(i64, i32)> =
                    sqlx::query_as("SELECT id, days FROM tariff_periods WHERE tariff_id = $1")
                        .bind(id)
                        .fetch_all(tx.conn())
                        .await?;
                for (period, days) in periods {
                    self.periods_by_days.insert((*id, days as u32), period);
                }
                for period in &tariff.periods {
                    if let Some(old) = &period.old_id
                        && let Some((pid, _)) = period_map.get(old)
                    {
                        self.periods.insert(old.clone(), *pid);
                    }
                }
                self.tariffs.insert(tariff.old_id.clone(), *id);
                self.tariff_names.insert(*id, tariff.name.clone());
                continue;
            }
            let category_id = match self.categories.get(&tariff.category_old_id) {
                Some(id) => *id,
                None => match fallback {
                    Some(id) => id,
                    None => {
                        let created = tx
                            .tariffs()
                            .create_category(&NewCategory {
                                name: "Тарифы".into(),
                                sort: 0,
                                visible: true,
                            })
                            .await?;
                        fallback = Some(created.id.0);
                        created.id.0
                    }
                },
            };
            let mut on_sale = tariff.on_sale;
            if let Some(known) = &squads {
                let unknown: Vec<String> = tariff
                    .squads
                    .iter()
                    .filter(|s| !known.contains(s))
                    .map(|s| s.to_string())
                    .collect();
                if !unknown.is_empty() {
                    on_sale = false;
                    self.issue(
                        "tariff_squads",
                        "tariff",
                        Some(&tariff.old_id),
                        Some(tariff.name.clone()),
                        format!(
                            "Тариф «{}»: в Remnawave нет сквадов {} — тариф снят с продажи",
                            tariff.name,
                            unknown.join(", ")
                        ),
                    );
                }
            }
            let price = |minor: Option<i64>| {
                let mut prices = CurrencyPrices::default();
                prices.set(tariff.currency, minor);
                prices
            };
            let extra = (tariff.extra_devices_max > 0)
                .then_some(tariff.extra_device_monthly_minor)
                .flatten()
                .map(|monthly| ExtraDevices {
                    max: tariff.extra_devices_max,
                    monthly: price(Some(monthly)),
                    monthly_stars: None,
                });
            let new = NewTariff {
                traffic_addon: None,
                category_id: sn_domain::ids::CategoryId(category_id),
                name: trimmed(&tariff.name, 64),
                description: tariff.description.as_deref().map(|d| trimmed(d, 2000)),
                on_sale: on_sale && tariff.periods.iter().any(|p| p.price_minor.is_some()),
                sort: tariff.sort,
                devices: DevicePolicy {
                    included: tariff.devices_included,
                    extra,
                },
                traffic: TrafficPolicy {
                    limit_bytes: tariff.traffic_limit_bytes,
                    reset: tariff.traffic_reset,
                },
                squads: tariff.squads.iter().map(|s| SquadId(*s)).collect(),
                external_squad: tariff.external_squad.map(SquadId),
                periods: tariff
                    .periods
                    .iter()
                    .map(|p| NewPeriod {
                        days: p.days,
                        prices: price(p.price_minor),
                        stars: None,
                        sort: p.sort,
                        active: p.price_minor.is_some(),
                    })
                    .collect(),
            };
            let created = tx.tariffs().create(&new).await?;
            let id = created.id.0;
            let max = created.devices.extra.as_ref().map_or(0, |e| e.max);
            let data = match ladder {
                Some(ladder) if check_device_discounts(ladder, max).is_ok() => {
                    tx.tariffs()
                        .set_device_discounts(created.id, ladder)
                        .await?;
                    serde_json::json!({ "discounts": true })
                }
                _ => serde_json::json!({}),
            };
            self.tariffs.insert(tariff.old_id.clone(), id);
            self.tariff_names.insert(id, created.name.clone());
            tariff_rows.push((tariff.old_id.clone(), id, data));
            for period in &created.periods {
                self.periods_by_days.insert((id, period.days), period.id.0);
            }
            for period in &tariff.periods {
                if let Some(old) = &period.old_id
                    && let Some(new_id) = self.periods_by_days.get(&(id, period.days))
                {
                    self.periods.insert(old.clone(), *new_id);
                    period_rows.push((old.clone(), *new_id, serde_json::json!({})));
                }
            }
            self.outcome.counts.tariffs += 1;
            if done > 0 && done % 20 == 0 {
                self.progress(Stage::Tariffs, done, total).await?;
            }
        }
        remember(
            &mut tx,
            &src_entity(source, "tariff"),
            &tariff_rows,
            self.run_id,
        )
        .await?;
        remember(
            &mut tx,
            &src_entity(source, "period"),
            &period_rows,
            self.run_id,
        )
        .await?;
        tx.commit().await?;
        self.progress(Stage::Tariffs, total, total).await
    }

    async fn clients(&mut self) -> ServiceResult<()> {
        let source = self.source();
        let entity = src_entity(source, "client");
        let map = legacy_map(self.services, &entity).await?;
        let (by_tg, by_email) = identity_owners(self.services, self.legacy).await?;
        let with_subs: HashSet<&str> = self
            .legacy
            .subscriptions
            .iter()
            .map(|s| s.client_old_id.as_str())
            .collect();
        let total = self.legacy.clients.len();
        let currency = self.legacy.currency;
        let legacy = self.legacy;
        for (batch_no, chunk) in legacy.clients.chunks(BATCH).enumerate() {
            let mut tx = self.services.store().begin().await?;
            let mut rows = Vec::with_capacity(chunk.len());
            for client in chunk {
                let mapped = match map.get(&client.old_id) {
                    Some((id, data)) if alive(&mut tx, "clients", *id).await? => Some((
                        *id,
                        data.get("balance")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                    )),
                    _ => None,
                };
                let owner = client
                    .telegram_id
                    .and_then(|t| by_tg.get(&t).copied())
                    .or_else(|| client.email.as_ref().and_then(|e| by_email.get(e).copied()));
                let (id, previous, fate) = match (mapped, owner) {
                    (Some((id, balance)), _) => (id, balance, Fate::Updated),
                    (None, Some(owner)) => (owner, 0, Fate::Merged),
                    (None, None) => (self.create_client(&mut tx, client).await?, 0, Fate::Created),
                };
                match fate {
                    Fate::Created => self.outcome.created += 1,
                    Fate::Merged => self.outcome.merged += 1,
                    Fate::Updated => self.outcome.updated += 1,
                }
                self.identities(&mut tx, id, client).await?;
                if client.trial_used || with_subs.contains(client.old_id.as_str()) {
                    self.trial_claims(&mut tx, id, client).await?;
                }
                let credited = self
                    .balance(&mut tx, id, client, previous, currency)
                    .await?;
                if let Some(utm) = &client.utm {
                    self.utm(&mut tx, id, utm).await?;
                }
                self.clients.insert(client.old_id.clone(), id);
                rows.push((
                    client.old_id.clone(),
                    id,
                    serde_json::json!({ "balance": credited }),
                ));
            }
            remember(&mut tx, &entity, &rows, self.run_id).await?;
            tx.commit().await?;
            self.outcome.counts.clients += chunk.len() as u64;
            self.progress(
                Stage::Clients,
                (batch_no * BATCH + chunk.len()).min(total),
                total,
            )
            .await?;
        }
        Ok(())
    }

    async fn create_client(&mut self, tx: &mut Tx, client: &LClient) -> ServiceResult<i64> {
        let status = if client.blocked { "blocked" } else { "active" };
        Ok(sqlx::query_scalar(
            "INSERT INTO clients (status, block_reason, blocked_at, locale, registration_source, bot_unreachable, created_at) \
             VALUES ($1, $2, CASE WHEN $1 = 'blocked' THEN now() END, $3, 'migration', $4, COALESCE($5, now())) RETURNING id",
        )
        .bind(status)
        .bind(client.block_reason.as_deref().filter(|_| client.blocked).map(|r| trimmed(r, 500)))
        .bind(client.locale.as_str())
        .bind(client.bot_unreachable)
        .bind(client.created_at)
        .fetch_one(tx.conn())
        .await?)
    }

    async fn identities(&mut self, tx: &mut Tx, id: i64, client: &LClient) -> ServiceResult<()> {
        let mut wanted: Vec<NewIdentity> = Vec::new();
        if let Some(tg) = client.telegram_id {
            wanted.push(NewIdentity {
                kind: IdentityKind::Telegram,
                value: tg.to_string(),
                verified: true,
                meta: serde_json::json!({
                    "username": client.username,
                    "first_name": client.first_name,
                    "last_name": client.last_name,
                }),
            });
        }
        if let Some(email) = &client.email {
            wanted.push(NewIdentity {
                kind: IdentityKind::Email,
                value: email.clone(),
                verified: client.email_verified,
                meta: serde_json::json!({}),
            });
        }
        for (kind, value) in [
            (IdentityKind::Google, &client.google_id),
            (IdentityKind::Apple, &client.apple_id),
        ] {
            if let Some(value) = value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty() && v.len() <= 255)
            {
                wanted.push(NewIdentity {
                    kind,
                    value: value.to_string(),
                    verified: true,
                    meta: serde_json::json!({}),
                });
            }
        }
        let mut has_email = false;
        for identity in wanted {
            let added = tx
                .clients()
                .add_identity_if_free(ClientId(id), &identity)
                .await?;
            if added {
                has_email |= identity.kind == IdentityKind::Email;
                continue;
            }
            let owner = tx
                .clients()
                .find_by_identity(identity.kind, &identity.value)
                .await?;
            let migrated = owner
                .as_ref()
                .is_some_and(|c| c.registration_source == RegistrationSource::Migration);
            if owner.map(|c| c.id.0) == Some(id) {
                has_email |= identity.kind == IdentityKind::Email;
                if identity.kind == IdentityKind::Telegram {
                    let meta = merged_meta(tx, &identity).await?;
                    tx.clients()
                        .update_identity_meta(identity.kind, &identity.value, &meta)
                        .await?;
                }
            } else if matches!(identity.kind, IdentityKind::Telegram | IdentityKind::Email) {
                self.issue(
                    "identity_taken",
                    "client",
                    Some(&client.old_id),
                    Some(client.who()),
                    match (identity.kind, migrated) {
                        (IdentityKind::Email, true) => format!(
                            "Почта {} уже у другого перенесённого клиента: в старой системе с ней два аккаунта, например с разным регистром букв. Этому клиенту вход по почте не добавлен",
                            identity.value
                        ),
                        (IdentityKind::Email, false) => format!(
                            "Почта {} уже у другого клиента этой установки — этому клиенту не добавлена",
                            identity.value
                        ),
                        (_, true) => format!(
                            "Telegram {} уже у другого перенесённого клиента — этому клиенту не добавлен",
                            identity.value
                        ),
                        _ => format!(
                            "Telegram {} уже у другого клиента этой установки — этому клиенту не добавлен",
                            identity.value
                        ),
                    },
                );
            }
        }
        if has_email && let Some(hash) = &client.password_hash {
            sqlx::query(
                "INSERT INTO client_passwords (client_id, password_hash) VALUES ($1, $2) ON CONFLICT (client_id) DO NOTHING",
            )
            .bind(id)
            .bind(hash)
            .execute(tx.conn())
            .await?;
        }
        Ok(())
    }

    async fn trial_claims(&mut self, tx: &mut Tx, id: i64, client: &LClient) -> ServiceResult<()> {
        let mut claims: Vec<(&str, String)> = Vec::new();
        if let Some(tg) = client.telegram_id {
            claims.push(("telegram", tg.to_string()));
        }
        if let Some(email) = &client.email {
            claims.push(("email", email.clone()));
        }
        for (kind, value) in claims {
            sqlx::query(
                "INSERT INTO trial_claims (kind, value, client_id) VALUES ($1, $2, $3) ON CONFLICT (kind, value) DO NOTHING",
            )
            .bind(kind)
            .bind(value)
            .bind(id)
            .execute(tx.conn())
            .await?;
        }
        Ok(())
    }

    async fn balance(
        &mut self,
        tx: &mut Tx,
        id: i64,
        client: &LClient,
        previous: i64,
        currency: Currency,
    ) -> ServiceResult<i64> {
        let target = client.balance_minor;
        let delta = target - previous;
        if delta == 0 {
            return Ok(previous);
        }
        let key = format!(
            "import:{}:{}:{}",
            self.source().as_str(),
            client.old_id,
            self.run_key
        );
        if delta > 0 {
            let entry = NewLedgerEntry {
                client_id: ClientId(id),
                wallet: Wallet::Main,
                amount: Money::new(delta, currency),
                kind: LedgerKind::MigrationOpening,
                order_id: None,
                payment_id: None,
                admin_id: None,
                comment: Some(if previous == 0 {
                    format!("Перенос из {}: баланс", self.source().title())
                } else {
                    format!(
                        "Перенос из {}: баланс пополнился в старой системе",
                        self.source().title()
                    )
                }),
                idempotency_key: Some(key),
            };
            tx.ledger().append(&entry).await?;
            return Ok(target);
        }
        let Some(admin) = self.admin else {
            self.issue(
                "balance_down",
                "client",
                Some(&client.old_id),
                Some(client.who()),
                format!(
                    "Баланс в старой системе уменьшился на {}, а списать некому: запустите перенос из админки",
                    super::stealthnet::format_minor(-delta)
                ),
            );
            return Ok(previous);
        };
        let current = tx
            .ledger()
            .balance(ClientId(id), Wallet::Main, currency)
            .await?
            .minor;
        let take = (-delta).min(current);
        if take < -delta {
            self.issue(
                "balance_spent",
                "client",
                Some(&client.old_id),
                Some(client.who()),
                format!(
                    "Баланс в старой системе уменьшился на {}, а здесь осталось {} — списали сколько было",
                    super::stealthnet::format_minor(-delta),
                    super::stealthnet::format_minor(current)
                ),
            );
        }
        if take > 0 {
            let entry = NewLedgerEntry {
                client_id: ClientId(id),
                wallet: Wallet::Main,
                amount: Money::new(-take, currency),
                kind: LedgerKind::AdminAdjust,
                order_id: None,
                payment_id: None,
                admin_id: Some(admin),
                comment: Some(format!(
                    "Перенос из {}: баланс уменьшился в старой системе",
                    self.source().title()
                )),
                idempotency_key: Some(key),
            };
            match tx.ledger().append(&entry).await {
                Ok(_) => {}
                Err(StoreError::InsufficientFunds) => return Ok(previous),
                Err(err) => return Err(err.into()),
            }
        }
        Ok(target)
    }

    async fn utm(&mut self, tx: &mut Tx, id: i64, utm: &Utm) -> ServiceResult<()> {
        let values = [
            utm_value(&utm.source),
            utm_value(&utm.medium),
            utm_value(&utm.campaign),
            utm_value(&utm.content),
            utm_value(&utm.term),
        ];
        if values.iter().all(Option::is_none) {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO client_utm (client_id, source, medium, campaign, content, term, start_param, registration_source) \
             VALUES ($1, $2, $3, $4, $5, $6, 'import', 'migration') ON CONFLICT (client_id) DO NOTHING",
        )
        .bind(id)
        .bind(&values[0])
        .bind(&values[1])
        .bind(&values[2])
        .bind(&values[3])
        .bind(&values[4])
        .execute(tx.conn())
        .await?;
        Ok(())
    }

    fn autorenew_period(&self, sub: &LSubscription, tariff: Option<i64>) -> Option<i64> {
        let tariff = tariff?;
        if let Some(old) = &sub.autorenew_period_old_id
            && let Some(id) = self.periods.get(old)
        {
            return Some(*id);
        }
        if let Some(days) = sub.autorenew_days
            && let Some(id) = self.periods_by_days.get(&(tariff, days))
        {
            return Some(*id);
        }
        self.periods_by_days
            .iter()
            .filter(|((t, _), _)| *t == tariff)
            .min_by_key(|((_, days), _)| *days)
            .map(|(_, id)| *id)
    }

    async fn subscriptions(&mut self) -> ServiceResult<()> {
        let source = self.source();
        let entity = src_entity(source, "subscription");
        let map = legacy_map(self.services, &entity).await?;
        let user_ids: Vec<i64> = self
            .matched
            .iter()
            .filter_map(|m| match m {
                Matched::Found(i, _) => Some(self.index.users[*i].id.0),
                _ => None,
            })
            .collect();
        let linked = links(self.services, &user_ids).await?;
        let total = self.legacy.subscriptions.len();
        let now = self.services.now();
        let legacy = self.legacy;
        let sub_issues = SubIssues::new(legacy, self.index);
        let matched = std::mem::take(&mut self.matched);
        for (batch_no, chunk) in legacy.subscriptions.chunks(BATCH).enumerate() {
            let mut tx = self.services.store().begin().await?;
            let mut rows = Vec::new();
            for (offset, sub) in chunk.iter().enumerate() {
                let at = batch_no * BATCH + offset;
                let m = &matched[at];
                let Matched::Found(i, _) = m else {
                    if let Some(mut issue) = sub_issues.issue(at, m) {
                        issue.stage = "apply";
                        self.issues.push(issue);
                    }
                    continue;
                };
                let Some(client_id) = self.clients.get(&sub.client_old_id).copied() else {
                    self.issue(
                        "subscription_orphan",
                        "subscription",
                        Some(&sub.old_id),
                        None,
                        "Клиента этой подписки нет в выгрузке — подписка не перенесена",
                    );
                    continue;
                };
                let user = &self.index.users[*i];
                let tariff = sub
                    .tariff_old_id
                    .as_ref()
                    .and_then(|t| self.tariffs.get(t))
                    .copied();
                let period = self.autorenew_period(sub, tariff);
                let mapped = map.get(&sub.old_id).map(|(id, _)| *id);
                let existing = linked.get(&user.id.0).copied();
                let sub_id = match (mapped, existing) {
                    (Some(mid), Some((eid, _))) if mid == eid => Some(eid),
                    (_, Some((eid, owner))) if owner == client_id => Some(eid),
                    (_, Some(_)) => {
                        self.issue(
                            "subscription_taken",
                            "subscription",
                            Some(&sub.old_id),
                            None,
                            format!(
                                "Пользователь Remnawave {} уже привязан к другому клиенту этой установки",
                                user.username
                            ),
                        );
                        None
                    }
                    (Some(mid), None) if alive(&mut tx, "subscriptions", mid).await? => {
                        tx.subscriptions()
                            .link_remna(
                                SubscriptionId(mid),
                                &sn_domain::subscription::RemnaLink {
                                    user_id: user.id,
                                    username: user.username.clone(),
                                    short_uuid: Some(user.short_uuid.clone()),
                                },
                            )
                            .await?;
                        Some(mid)
                    }
                    _ => {
                        let created: Option<i64> = sqlx::query_scalar(
                            "INSERT INTO subscriptions (client_id, tariff_id, kind, extra_devices, autorenew_enabled, \
                             autorenew_period_id, remna_user_id, remna_username, short_uuid, created_at) \
                             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, now())) \
                             ON CONFLICT DO NOTHING RETURNING id",
                        )
                        .bind(client_id)
                        .bind(tariff)
                        .bind(sub.kind.as_str())
                        .bind(i32::try_from(sub.extra_devices.min(1000)).unwrap_or(0))
                        .bind(sub.autorenew && period.is_some())
                        .bind(period)
                        .bind(user.id.0)
                        .bind(&user.username)
                        .bind(&user.short_uuid)
                        .bind(sub.created_at)
                        .fetch_optional(tx.conn())
                        .await?;
                        if created.is_none() {
                            self.issue(
                                "subscription_taken",
                                "subscription",
                                Some(&sub.old_id),
                                None,
                                format!(
                                    "Имя {} уже занято другой подпиской этой установки",
                                    user.username
                                ),
                            );
                        }
                        created
                    }
                };
                let Some(sub_id) = sub_id else { continue };
                tx.subscriptions()
                    .apply_mirror(SubscriptionId(sub_id), &user.mirror(None, now))
                    .await?;
                self.subs.insert(sub.old_id.clone(), sub_id);
                rows.push((
                    sub.old_id.clone(),
                    sub_id,
                    serde_json::json!({ "remna": user.id.0 }),
                ));
                self.outcome.counts.subscriptions += 1;
            }
            remember(&mut tx, &entity, &rows, self.run_id).await?;
            tx.commit().await?;
            self.progress(
                Stage::Subscriptions,
                (batch_no * BATCH + chunk.len()).min(total),
                total,
            )
            .await?;
        }
        Ok(())
    }

    fn payment_label(&self, payment: &LPayment) -> String {
        if let Some(label) = &payment.label {
            return label.clone();
        }
        match payment.kind {
            PaymentKind::TopUp => "Пополнение баланса".into(),
            PaymentKind::Subscription => {
                let name = payment
                    .tariff_old_id
                    .as_ref()
                    .and_then(|t| self.tariffs.get(t))
                    .and_then(|id| self.tariff_names.get(id));
                match (name, payment.days) {
                    (Some(name), Some(days)) => format!("Тариф «{name}», {days} дн."),
                    (Some(name), None) => format!("Тариф «{name}»"),
                    (None, Some(days)) => format!("Подписка, {days} дн."),
                    (None, None) => "Подписка".into(),
                }
            }
            PaymentKind::Other => "Покупка".into(),
        }
    }

    async fn payments(&mut self) -> ServiceResult<()> {
        let source = self.source();
        let entity = src_entity(source, "payment");
        let map = legacy_map(self.services, &entity).await?;
        let install = self.services.settings().currency;
        let mut first: HashMap<&str, (Option<DateTime<Utc>>, &str)> = HashMap::new();
        for payment in &self.legacy.payments {
            if payment.kind != PaymentKind::Subscription {
                continue;
            }
            let entry = first
                .entry(payment.client_old_id.as_str())
                .or_insert((payment.paid_at, payment.old_id.as_str()));
            if payment.paid_at < entry.0 {
                *entry = (payment.paid_at, payment.old_id.as_str());
            }
        }
        let firsts: HashSet<&str> = first.values().map(|(_, id)| *id).collect();
        let total = self.legacy.payments.len();
        let legacy = self.legacy;
        for (batch_no, chunk) in legacy.payments.chunks(PAYMENT_BATCH).enumerate() {
            let mut tx = self.services.store().begin().await?;
            let mut rows = Vec::new();
            for payment in chunk {
                if map.contains_key(&payment.old_id) {
                    continue;
                }
                let Some(client_id) = self.clients.get(&payment.client_old_id).copied() else {
                    continue;
                };
                let subscription = payment
                    .subscription_old_id
                    .as_ref()
                    .and_then(|s| self.subs.get(s))
                    .copied();
                let kind = match payment.kind {
                    PaymentKind::TopUp => OrderKind::TopUp,
                    PaymentKind::Subscription if firsts.contains(payment.old_id.as_str()) => {
                        OrderKind::New
                    }
                    PaymentKind::Subscription => OrderKind::Renewal,
                    PaymentKind::Other => OrderKind::Module,
                };
                let (order_currency, total_minor, stars, pay_currency) = match payment.currency {
                    PayCurrencyLegacy::Money(c) => (c, payment.amount_minor, None, c.code()),
                    PayCurrencyLegacy::Stars => (install, 0, Some(payment.amount_minor), "XTR"),
                };
                let label = self.payment_label(payment);
                let total = Money::new(total_minor, order_currency);
                let quote = Quote {
                    currency: order_currency,
                    lines: Vec::new(),
                    subtotal: total,
                    discount: Money::zero(order_currency),
                    total,
                    stars: stars.map(|s| StarsTotal {
                        subtotal: Stars(s),
                        discount: Stars(0),
                        total: Stars(s),
                    }),
                    days: payment.days.unwrap_or(0),
                    extra_devices: 0,
                };
                let intent = PurchaseIntent::Imported {
                    source: source.as_str().to_string(),
                    old_id: payment.old_id.clone(),
                    label: Some(label),
                };
                let paid_at = payment.paid_at.or(payment.created_at);
                let order_status = if payment.refunded {
                    "refunded"
                } else {
                    "fulfilled"
                };
                let order_id: i64 = sqlx::query_scalar(
                    "INSERT INTO orders (client_id, subscription_id, kind, status, intent, quote, currency, total_minor, \
                     stars_total, created_at, paid_at, fulfilled_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, now()), $11, $11) RETURNING id",
                )
                .bind(client_id)
                .bind(subscription)
                .bind(kind.as_str())
                .bind(order_status)
                .bind(sqlx::types::Json(&intent))
                .bind(sqlx::types::Json(&quote))
                .bind(order_currency.code())
                .bind(total_minor)
                .bind(stars)
                .bind(payment.created_at.or(paid_at))
                .bind(paid_at)
                .fetch_one(tx.conn())
                .await?;
                let payload = serde_json::json!({
                    "imported": {
                        "source": source.as_str(),
                        "old_id": payment.old_id,
                        "external_id": payment.external_id,
                        "provider": payment.provider,
                    }
                });
                let payment_id: i64 = sqlx::query_scalar(
                    "INSERT INTO payments (order_id, client_id, provider, amount_minor, currency, status, provider_payload, \
                     refunded_minor, paid_at, created_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, now())) RETURNING id",
                )
                .bind(order_id)
                .bind(client_id)
                .bind(trimmed(&payment.provider, 40))
                .bind(payment.amount_minor)
                .bind(pay_currency)
                .bind(if payment.refunded { "refunded" } else { "paid" })
                .bind(&payload)
                .bind(if payment.refunded { payment.amount_minor } else { 0 })
                .bind(paid_at)
                .bind(payment.created_at.or(paid_at))
                .fetch_one(tx.conn())
                .await?;
                rows.push((
                    payment.old_id.clone(),
                    payment_id,
                    serde_json::json!({ "order": order_id }),
                ));
                self.outcome.counts.payments += 1;
            }
            remember(&mut tx, &entity, &rows, self.run_id).await?;
            tx.commit().await?;
            self.progress(
                Stage::Payments,
                (batch_no * PAYMENT_BATCH + chunk.len()).min(total),
                total,
            )
            .await?;
        }
        Ok(())
    }

    async fn promos(&mut self) -> ServiceResult<()> {
        let source = self.source();
        let entity = src_entity(source, "promo");
        let map = legacy_map(self.services, &entity).await?;
        let uses_entity = src_entity(source, "promo_use");
        let uses_map = legacy_map(self.services, &uses_entity).await?;
        let total = self.legacy.promos.len();
        self.progress(Stage::Promo, 0, total).await?;
        let currency = self.legacy.currency;
        let mut tx = self.services.store().begin().await?;
        let mut ids: HashMap<String, i64> = HashMap::new();
        let mut rows = Vec::new();
        let legacy = self.legacy;
        for promo in &legacy.promos {
            if let Some((id, _)) = map.get(&promo.old_id)
                && alive(&mut tx, "promo_codes", *id).await?
            {
                ids.insert(promo.old_id.clone(), *id);
                continue;
            }
            let existing: Option<i64> =
                sqlx::query_scalar("SELECT id FROM promo_codes WHERE code = $1")
                    .bind(&promo.code)
                    .fetch_optional(tx.conn())
                    .await?;
            if let Some(id) = existing {
                self.issue(
                    "promo_exists",
                    "promo",
                    Some(&promo.old_id),
                    Some(promo.code.clone()),
                    format!(
                        "Промокод {} уже есть в этой установке — оставили ваш",
                        promo.code
                    ),
                );
                ids.insert(promo.old_id.clone(), id);
                rows.push((promo.old_id.clone(), id, serde_json::json!({})));
                continue;
            }
            let (kind, percent, amount, money_currency, days) = match promo.value {
                PromoValue::Percent(p) => ("percent", Some(i32::from(p)), None, None, None),
                PromoValue::Fixed(minor) => {
                    ("fixed", None, Some(minor), Some(currency.code()), None)
                }
                PromoValue::Days(d) => ("days", None, None, None, Some(d as i32)),
            };
            let valid_from = promo
                .valid_from
                .filter(|from| promo.valid_until.is_none_or(|until| until > *from));
            let tariff_ids: Vec<i64> = promo
                .tariff_old_id
                .as_ref()
                .and_then(|t| self.tariffs.get(t))
                .copied()
                .into_iter()
                .collect();
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO promo_codes (code, kind, percent, amount_minor, currency, days, max_uses, per_client, \
                 valid_from, valid_until, first_purchase_only, tariff_ids, active, note, disabled_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, CASE WHEN $13 THEN NULL ELSE now() END) \
                 RETURNING id",
            )
            .bind(&promo.code)
            .bind(kind)
            .bind(percent)
            .bind(amount)
            .bind(money_currency)
            .bind(days)
            .bind(promo.max_uses.map(|m| m.min(i32::MAX as u32) as i32))
            .bind(promo.per_client.clamp(1, 1000) as i32)
            .bind(valid_from)
            .bind(promo.valid_until)
            .bind(promo.first_purchase_only)
            .bind(&tariff_ids)
            .bind(promo.active)
            .bind(
                [promo.note.clone(), Some(format!("Перенесён из {}", source.title()))]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(". "),
            )
            .fetch_one(tx.conn())
            .await?;
            ids.insert(promo.old_id.clone(), id);
            rows.push((promo.old_id.clone(), id, serde_json::json!({})));
            self.outcome.counts.promo_codes += 1;
        }
        remember(&mut tx, &entity, &rows, self.run_id).await?;
        let mut use_rows = Vec::new();
        for usage in &legacy.promo_uses {
            if uses_map.contains_key(&usage.old_id) {
                continue;
            }
            let (Some(promo), Some(client)) = (
                ids.get(&usage.promo_old_id),
                self.clients.get(&usage.client_old_id),
            ) else {
                continue;
            };
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO promo_redemptions (promo_id, client_id, status, used_at, created_at) \
                 VALUES ($1, $2, 'used', COALESCE($3, now()), COALESCE($3, now())) RETURNING id",
            )
            .bind(promo)
            .bind(client)
            .bind(usage.used_at)
            .fetch_one(tx.conn())
            .await?;
            use_rows.push((usage.old_id.clone(), id, serde_json::json!({})));
        }
        remember(&mut tx, &uses_entity, &use_rows, self.run_id).await?;
        tx.commit().await?;
        self.progress(Stage::Promo, total, total).await
    }

    async fn referrals(&mut self) -> ServiceResult<()> {
        let legacy = self.legacy;
        let total = legacy.clients.len();
        for (batch_no, chunk) in legacy.clients.chunks(BATCH * 2).enumerate() {
            let mut tx = self.services.store().begin().await?;
            for client in chunk {
                let Some(id) = self.clients.get(&client.old_id).copied() else {
                    continue;
                };
                if let Some(code) = client.referral_code.as_deref().and_then(referral_code) {
                    let owner: Option<i64> =
                        sqlx::query_scalar("SELECT client_id FROM referral_codes WHERE code = $1")
                            .bind(&code)
                            .fetch_optional(tx.conn())
                            .await?;
                    match owner {
                        Some(owner) if owner != id => self.issue(
                            "referral_code_taken",
                            "client",
                            Some(&client.old_id),
                            Some(client.who()),
                            format!(
                                "Реферальный код {code} уже у другого клиента — у этого будет новый"
                            ),
                        ),
                        Some(_) => {}
                        None => {
                            sqlx::query(
                                "INSERT INTO referral_codes (client_id, code, percent) VALUES ($1, $2, $3) \
                                 ON CONFLICT DO NOTHING",
                            )
                            .bind(id)
                            .bind(&code)
                            .bind(client.referral_percent)
                            .execute(tx.conn())
                            .await?;
                        }
                    }
                }
                let Some(referrer_old) = client.referrer_old_id.as_ref() else {
                    continue;
                };
                let Some(referrer) = self.clients.get(referrer_old).copied() else {
                    self.issue(
                        "referral_skipped",
                        "client",
                        Some(&client.old_id),
                        Some(client.who()),
                        "Пригласившего нет среди перенесённых клиентов — связь не перенесена",
                    );
                    continue;
                };
                if referrer == id {
                    self.issue(
                        "referral_skipped",
                        "client",
                        Some(&client.old_id),
                        Some(client.who()),
                        "Клиент указан пригласившим сам себя — связь не перенесена",
                    );
                    continue;
                }
                let loops: bool = sqlx::query_scalar(
                    "WITH RECURSIVE up(id, depth) AS (SELECT $1::bigint, 0 UNION ALL \
                     SELECT l.referrer_id, up.depth + 1 FROM referral_links l JOIN up ON l.client_id = up.id WHERE up.depth < 32) \
                     SELECT EXISTS (SELECT 1 FROM up WHERE id = $2)",
                )
                .bind(referrer)
                .bind(id)
                .fetch_one(tx.conn())
                .await?;
                if loops {
                    self.issue(
                        "referral_skipped",
                        "client",
                        Some(&client.old_id),
                        Some(client.who()),
                        "Клиент по цепочке приглашений сам пригласил своего пригласившего — связь не перенесена, чтобы не замкнуть круг",
                    );
                    continue;
                }
                let linked = sqlx::query(
                    "INSERT INTO referral_links (client_id, referrer_id, created_at) VALUES ($1, $2, COALESCE($3, now())) \
                     ON CONFLICT (client_id) DO NOTHING",
                )
                .bind(id)
                .bind(referrer)
                .bind(client.created_at)
                .execute(tx.conn())
                .await?
                .rows_affected();
                self.outcome.counts.referrals += linked;
            }
            tx.commit().await?;
            self.progress(
                Stage::Referrals,
                (batch_no * BATCH * 2 + chunk.len()).min(total),
                total,
            )
            .await?;
        }
        Ok(())
    }

    async fn tickets(&mut self) -> ServiceResult<()> {
        let source = self.source();
        let entity = src_entity(source, "ticket");
        let message_entity = src_entity(source, "ticket_message");
        let map = legacy_map(self.services, &entity).await?;
        let messages_map = legacy_map(self.services, &message_entity).await?;
        let legacy = self.legacy;
        let total = legacy.tickets.len();
        for (batch_no, chunk) in legacy.tickets.chunks(200).enumerate() {
            let mut tx = self.services.store().begin().await?;
            let mut rows = Vec::new();
            let mut message_rows = Vec::new();
            for ticket in chunk {
                let Some(client) = self.clients.get(&ticket.client_old_id).copied() else {
                    continue;
                };
                let ticket_id = match map.get(&ticket.old_id) {
                    Some((id, _)) if alive(&mut tx, "tickets", *id).await? => *id,
                    _ => {
                        let created = ticket.created_at;
                        let id: i64 = sqlx::query_scalar(
                            "INSERT INTO tickets (client_id, channel, status, subject, last_author, created_at, updated_at, closed_at) \
                             VALUES ($1, 'unknown', $2, $3, 'client', COALESCE($4, now()), COALESCE($4, now()), $5) RETURNING id",
                        )
                        .bind(client)
                        .bind(if ticket.open { "open" } else { "closed" })
                        .bind(ticket.subject.as_deref().map(|s| trimmed(s, 200)))
                        .bind(created)
                        .bind(if ticket.open { None } else { ticket.closed_at.or(created) })
                        .fetch_one(tx.conn())
                        .await?;
                        rows.push((ticket.old_id.clone(), id, serde_json::json!({})));
                        self.outcome.counts.tickets += 1;
                        id
                    }
                };
                let mut added = false;
                for message in &ticket.messages {
                    if messages_map.contains_key(&message.old_id) {
                        continue;
                    }
                    let id: i64 = sqlx::query_scalar(
                        "INSERT INTO ticket_messages (ticket_id, author, body, created_at, read_at) \
                         VALUES ($1, $2, $3, COALESCE($4, now()), CASE WHEN $5 THEN COALESCE($4, now()) END) RETURNING id",
                    )
                    .bind(ticket_id)
                    .bind(if message.from_support { "support" } else { "client" })
                    .bind(trimmed(&message.body, 20_000))
                    .bind(message.created_at)
                    .bind(message.read)
                    .fetch_one(tx.conn())
                    .await?;
                    message_rows.push((message.old_id.clone(), id, serde_json::json!({})));
                    added = true;
                }
                if added {
                    sqlx::query(
                        "UPDATE tickets t SET last_message_id = m.last_id, last_author = m.author, \
                         last_message_at = (SELECT max(x.created_at) FROM ticket_messages x WHERE x.ticket_id = t.id), \
                         waiting_message_id = CASE WHEN m.author = 'client' AND t.status = 'open' THEN \
                           (SELECT min(x.id) FROM ticket_messages x WHERE x.ticket_id = t.id AND x.author = 'client' \
                            AND x.id > COALESCE((SELECT max(r.id) FROM ticket_messages r WHERE r.ticket_id = t.id AND r.author <> 'client'), 0)) \
                         ELSE NULL END \
                         FROM (SELECT id AS last_id, author FROM ticket_messages WHERE ticket_id = $1 ORDER BY id DESC LIMIT 1) m \
                         WHERE t.id = $1",
                    )
                    .bind(ticket_id)
                    .execute(tx.conn())
                    .await?;
                }
            }
            remember(&mut tx, &entity, &rows, self.run_id).await?;
            remember(&mut tx, &message_entity, &message_rows, self.run_id).await?;
            tx.commit().await?;
            self.progress(
                Stage::Tickets,
                (batch_no * 200 + chunk.len()).min(total),
                total,
            )
            .await?;
        }
        Ok(())
    }

    async fn extras(&mut self) -> ServiceResult<()> {
        self.progress(Stage::Extras, 0, 1).await?;
        let mut extras = super::extras::Extras {
            services: self.services,
            legacy: self.legacy,
            admin: self.admin,
            run_id: self.run_id,
            clients: &self.clients,
            tariffs: &self.tariffs,
            tariff_names: &self.tariff_names,
            periods_by_days: &self.periods_by_days,
            periods: &self.periods,
            subs: &self.subs,
            issues: Vec::new(),
            done: super::extras::Done::default(),
        };
        extras.run().await?;
        let (issues, done) = (extras.issues, extras.done);
        self.issues.extend(issues);
        let counts = &mut self.outcome.counts;
        counts.settings = done.settings;
        counts.admins = done.admins;
        counts.gifts = done.gifts;
        counts.auto_rules = done.rules;
        counts.withdrawals = done.withdrawals;
        self.outcome.providers = done.providers;
        self.outcome.mail = done.mail;
        self.outcome.multi = done.multi;
        self.progress(Stage::Extras, 1, 1).await
    }

    async fn finish(&mut self) -> ServiceResult<()> {
        self.progress(Stage::Finish, 0, 1).await?;
        let source = self.source();
        let expected: i64 = self
            .legacy
            .clients
            .iter()
            .filter(|c| self.clients.contains_key(&c.old_id))
            .map(|c| c.balance_minor)
            .sum();
        let mut conn = self.services.store().conn().await?;
        let moved: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(delta_minor), 0)::bigint FROM balance_entries \
             WHERE kind IN ('migration_opening', 'admin_adjust') AND idempotency_key LIKE $1",
        )
        .bind(format!("import:{}:%", source.as_str()))
        .fetch_one(conn.conn())
        .await?;
        self.outcome.balance_total_minor = moved;
        if moved != expected {
            self.issues.push(IssueRow {
                stage: "apply",
                code: "balance_total".into(),
                entity: "client".into(),
                old_id: None,
                who: None,
                details: format!(
                    "Сумма балансов не сошлась: в выгрузке {}, перенесено {} — разница в строках выше",
                    super::stealthnet::format_minor(expected),
                    super::stealthnet::format_minor(moved)
                ),
            });
        }
        let (actor_kind, actor_id) = match self.admin {
            Some(admin) => (sn_store::ActorKind::Admin, Some(admin.0)),
            None => (sn_store::ActorKind::System, None),
        };
        let mut details = serde_json::to_value(&self.outcome)
            .map_err(|e| ServiceError::Internal(e.to_string()))?;
        details["source"] = serde_json::json!(source.as_str());
        conn.audit()
            .record(&sn_store::AuditEntry {
                actor_kind,
                actor_id,
                action: "import.done".into(),
                entity_kind: Some("import".into()),
                entity_id: Some(self.run_id),
                details,
                ip: None,
            })
            .await?;
        drop(conn);
        self.progress(Stage::Finish, 1, 1).await
    }
}

async fn merged_meta(tx: &mut Tx, identity: &NewIdentity) -> ServiceResult<serde_json::Value> {
    let current: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT meta FROM client_identities WHERE kind = $1 AND value = $2")
            .bind(identity.kind.as_str())
            .bind(&identity.value)
            .fetch_optional(tx.conn())
            .await?;
    let mut meta = current.unwrap_or_else(|| serde_json::json!({}));
    if let (Some(target), Some(source)) = (meta.as_object_mut(), identity.meta.as_object()) {
        for (key, value) in source {
            if !value.is_null() && target.get(key).is_none_or(serde_json::Value::is_null) {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(meta)
}
