use super::*;
use sn_domain::client::IdentityKind;
use sn_domain::ids::{PeriodId, TariffId};
use sn_domain::payment::providers::LAVA;
use sn_payments::{ProviderConfig, RenewalPlan, RenewalState};
use uuid::Uuid;

const ADOPT_BATCH: i64 = 200;
const RECHECK_HOURS: i64 = 24;
const RETRY_HOURS: i64 = 1;
const MISSING_RECHECK_DAYS: i64 = 7;
const AFTER_CHARGE_MINUTES: i64 = 5;
const NEAR_DAYS: i64 = 3;
pub(crate) const NO_PROVIDER: &str = "no_provider";
pub(crate) const NO_API_KEY: &str = "no_api_key";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Linked,
    Known,
    Unbound,
    Conflict,
    Ambiguous,
    Ended,
    Missing,
    Error,
}

impl State {
    const fn as_str(self) -> &'static str {
        match self {
            State::Linked | State::Known => "linked",
            State::Unbound => "unbound",
            State::Conflict => "conflict",
            State::Ambiguous => "ambiguous",
            State::Ended => "ended",
            State::Missing => "missing",
            State::Error => "error",
        }
    }

    fn next_check(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            State::Unbound | State::Conflict | State::Ambiguous => {
                Some(now + Duration::hours(RECHECK_HOURS))
            }
            State::Missing => Some(now + Duration::days(MISSING_RECHECK_DAYS)),
            State::Error => Some(now + Duration::hours(RETRY_HOURS)),
            State::Linked | State::Known | State::Ended => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Adoption {
    pub adopted: usize,
    pub checked: usize,
    pub skipped: Option<&'static str>,
}

enum Pick {
    Found(String, Box<RenewalState>),
    Mine(String),
    Ambiguous,
    None,
}

enum Taken {
    Free,
    Mine,
    Other,
}

enum Buyer {
    Prefix(String),
    Exact(String),
}

impl Buyer {
    fn query(&self) -> &str {
        match self {
            Buyer::Prefix(text) | Buyer::Exact(text) => text,
        }
    }

    fn matches(&self, email: Option<&str>) -> bool {
        let Some(email) = email.map(|e| e.trim().to_lowercase()) else {
            return false;
        };
        match self {
            Buyer::Prefix(prefix) => email.starts_with(&prefix.to_lowercase()),
            Buyer::Exact(exact) => email == exact.to_lowercase(),
        }
    }
}

#[derive(sqlx::FromRow)]
struct Contract {
    id: i64,
    client_id: i64,
    intent: serde_json::Value,
    external_id: Option<String>,
    order_ref: Option<String>,
    order_id: Option<i64>,
    contract_id: Option<String>,
    amount_minor: i64,
    currency: String,
    source: String,
    created_at: DateTime<Utc>,
}

impl Contract {
    fn candidates(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for raw in [&self.contract_id, &self.external_id, &self.order_ref]
            .into_iter()
            .flatten()
        {
            let id = raw.trim();
            if Uuid::parse_str(id).is_ok()
                && !out.iter().any(|known| known.eq_ignore_ascii_case(id))
            {
                out.push(id.to_string());
            }
        }
        out
    }

    fn intent(&self) -> (Option<SubscriptionId>, Option<TariffId>, Option<PeriodId>) {
        match serde_json::from_value::<PurchaseIntent>(self.intent.clone()) {
            Ok(PurchaseIntent::Subscription {
                target,
                tariff_id,
                period_id,
                ..
            }) => (target, Some(tariff_id), Some(period_id)),
            _ => (None, None, None),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Term {
    tariff: Option<TariffId>,
    period: Option<PeriodId>,
    name: Option<String>,
    planned: bool,
}

fn folded(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn plan_for<'p>(plans: &'p [RenewalPlan], state: &RenewalState) -> Option<&'p RenewalPlan> {
    let name = folded(state.name.as_deref()?);
    let periodicity = state.periodicity.as_deref()?.trim();
    let currency = state.amount?.currency;
    let mut found = plans.iter().filter(|plan| {
        plan.periodicity.eq_ignore_ascii_case(periodicity)
            && plan.amount.currency.code() == currency.code()
            && folded(&plan.name) == name
    });
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

fn term(plans: &[RenewalPlan], state: &RenewalState, contract: &Contract) -> Term {
    if let Some(plan) = plan_for(plans, state) {
        return Term {
            tariff: Some(TariffId(plan.binding.tariff_id)),
            period: Some(PeriodId(plan.binding.period_id)),
            name: Some(plan.name.clone()).filter(|name| !name.is_empty()),
            planned: true,
        };
    }
    let (_, tariff, period) = contract.intent();
    Term {
        tariff,
        period,
        name: None,
        planned: false,
    }
}

fn cycle_days(cycle: Cycle) -> u32 {
    match cycle {
        Cycle::Days(days) => days.max(1),
        Cycle::Months(months) => (months.max(1) * 365).div_ceil(12),
    }
}

pub(crate) async fn note_legacy_charge(
    db: &mut impl Db,
    invoice: i64,
    series: &str,
    now: DateTime<Utc>,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE legacy_invoices SET contract_id = $2, contract_check_at = $3 \
         WHERE id = $1 AND recurring AND contract_state IS DISTINCT FROM 'linked'",
    )
    .bind(invoice)
    .bind(series.trim())
    .bind(now + Duration::minutes(AFTER_CHARGE_MINUTES))
    .execute(db.conn())
    .await?;
    Ok(())
}

async fn mark(
    db: &mut impl Db,
    contract: i64,
    state: State,
    series: Option<&str>,
    now: DateTime<Utc>,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE legacy_invoices SET contract_state = $2, contract_id = COALESCE($3, contract_id), \
         contract_check_at = $4, contract_checked_at = $5 WHERE id = $1",
    )
    .bind(contract)
    .bind(state.as_str())
    .bind(series)
    .bind(state.next_check(now))
    .bind(now)
    .execute(db.conn())
    .await?;
    Ok(())
}

async fn usable(
    tx: &mut Tx,
    client: i64,
    id: SubscriptionId,
) -> ServiceResult<Option<Subscription>> {
    match tx.subscriptions().get(id).await {
        Ok(sub)
            if sub.client_id.0 == client && sub.ended_at.is_none() && sub.tariff_id.is_some() =>
        {
            Ok(Some(sub))
        }
        Ok(_) | Err(sn_store::StoreError::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

impl Renewals<'_> {
    async fn waiting(&self, now: DateTime<Utc>) -> ServiceResult<i64> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM legacy_invoices WHERE recurring AND provider = $1 \
             AND (contract_check_at <= $2 OR (contract_check_at IS NULL AND contract_state IS NULL))",
        )
        .bind(LAVA)
        .bind(now)
        .fetch_one(conn.conn())
        .await?)
    }

    async fn skipped(&self, now: DateTime<Utc>, reason: &'static str) -> ServiceResult<Adoption> {
        let waiting = self.waiting(now).await?;
        if waiting == 0 {
            return Ok(Adoption::default());
        }
        let text = if reason == NO_PROVIDER {
            "договоры Lava.top старой панели ждут проверки, но платёжной системы Lava.top в панели нет"
        } else {
            "договоры Lava.top старой панели ждут проверки, но у Lava.top нет API-ключа в «Настройки → Оплата»"
        };
        tracing::warn!(waiting, reason, "{text}");
        Ok(Adoption {
            skipped: Some(reason),
            ..Adoption::default()
        })
    }

    pub(super) async fn adopt_legacy(&self, now: DateTime<Utc>) -> ServiceResult<Adoption> {
        let Some(provider) = self.provider(LAVA) else {
            return self.skipped(now, NO_PROVIDER).await;
        };
        let config = self.services.payments().config(LAVA);
        if config.get("api_key").is_none() {
            return self.skipped(now, NO_API_KEY).await;
        }
        let due: Vec<Contract> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_as(
                "SELECT id, client_id, intent, external_id, order_ref, order_id, contract_id, \
                 amount_minor, currency, source, created_at FROM legacy_invoices \
                 WHERE recurring AND provider = $1 \
                 AND (contract_check_at <= $2 OR (contract_check_at IS NULL AND contract_state IS NULL)) \
                 ORDER BY contract_check_at NULLS FIRST, id LIMIT $3",
            )
            .bind(LAVA)
            .bind(now)
            .bind(ADOPT_BATCH)
            .fetch_all(conn.conn())
            .await?
        };
        if due.is_empty() {
            return Ok(Adoption::default());
        }
        let plans = provider.renewal_plans(&config);
        let mut report = Adoption::default();
        let mut counts: std::collections::BTreeMap<&'static str, usize> =
            std::collections::BTreeMap::new();
        for contract in due {
            report.checked += 1;
            match self
                .adopt(provider.as_ref(), &config, &plans, &contract, now)
                .await
            {
                Ok(state) => {
                    if state == State::Linked {
                        report.adopted += 1;
                    }
                    *counts.entry(state.as_str()).or_default() += 1;
                }
                Err(err) => {
                    let mut conn = self.services.store().conn().await?;
                    mark(&mut conn, contract.id, State::Error, None, now).await?;
                    *counts.entry(State::Error.as_str()).or_default() += 1;
                    tracing::warn!(legacy_invoice = contract.id, error = %err, "договор Lava.top старой панели не проверен, повторим");
                    break;
                }
            }
        }
        tracing::info!(
            checked = report.checked,
            adopted = report.adopted,
            states = ?counts,
            "договоры Lava.top старой панели проверены"
        );
        Ok(report)
    }

    async fn adopt(
        &self,
        provider: &dyn PaymentProvider,
        config: &ProviderConfig,
        plans: &[RenewalPlan],
        contract: &Contract,
        now: DateTime<Utc>,
    ) -> ServiceResult<State> {
        let mut pick = Pick::None;
        for candidate in contract.candidates() {
            if let Some(state) = provider.find_renewal(config, &candidate).await? {
                pick = Pick::Found(candidate, Box::new(state));
                break;
            }
        }
        if matches!(pick, Pick::None) {
            pick = self.by_email(provider, config, contract).await?;
        }
        let (series, state) = match pick {
            Pick::Found(series, state) => (series, state),
            Pick::Mine(series) => {
                let mut conn = self.services.store().conn().await?;
                mark(&mut conn, contract.id, State::Known, Some(&series), now).await?;
                return Ok(State::Known);
            }
            Pick::Ambiguous => {
                let mut conn = self.services.store().conn().await?;
                mark(&mut conn, contract.id, State::Ambiguous, None, now).await?;
                return Ok(State::Ambiguous);
            }
            Pick::None => {
                let mut conn = self.services.store().conn().await?;
                mark(&mut conn, contract.id, State::Missing, None, now).await?;
                return Ok(State::Missing);
            }
        };
        let term = term(plans, &state, contract);
        let mut tx = self.services.store().begin().await?;
        let outcome = match state.status {
            RenewalStatus::Cancelled | RenewalStatus::Failed => State::Ended,
            RenewalStatus::Active | RenewalStatus::Unknown => {
                if lock_series(&mut tx, LAVA, &series).await?.is_some() {
                    State::Known
                } else {
                    match self.bound(&mut tx, contract, term.tariff).await? {
                        None => State::Unbound,
                        Some(sub) if live_link(&mut tx, sub.id).await?.is_some() => State::Conflict,
                        Some(sub) => {
                            self.link(&mut tx, contract, &series, &state, &sub, &term, now)
                                .await?
                        }
                    }
                }
            }
        };
        mark(&mut tx, contract.id, outcome, Some(&series), now).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    async fn buyers(&self, contract: &Contract) -> ServiceResult<Vec<Buyer>> {
        let mut conn = self.services.store().conn().await?;
        let old: Vec<String> =
            sqlx::query_scalar("SELECT old_id FROM legacy_ids WHERE entity = $1 AND new_id = $2")
                .bind(format!("{}:client", contract.source))
                .bind(contract.client_id)
                .fetch_all(conn.conn())
                .await?;
        let client = ClientId(contract.client_id);
        let identities = conn.clients().identities(client).await?;
        let receipt = conn.clients().receipt_email(client).await?;
        let mut out: Vec<Buyer> = old
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .map(|id| Buyer::Prefix(format!("client-{id}@")))
            .collect();
        for identity in &identities {
            match identity.kind {
                IdentityKind::Email => out.push(Buyer::Exact(identity.value.trim().to_string())),
                IdentityKind::Telegram => out.push(Buyer::Exact(format!(
                    "tg_{}@telegram.local",
                    identity.value.trim()
                ))),
                IdentityKind::Google | IdentityKind::Apple => {}
            }
        }
        if let Some(receipt) = receipt {
            out.push(Buyer::Exact(receipt.trim().to_string()));
        }
        let mut seen: Vec<String> = Vec::new();
        out.retain(|buyer| {
            let key = buyer.query().to_lowercase();
            if key.is_empty() || seen.contains(&key) {
                return false;
            }
            seen.push(key);
            true
        });
        Ok(out)
    }

    async fn by_email(
        &self,
        provider: &dyn PaymentProvider,
        config: &ProviderConfig,
        contract: &Contract,
    ) -> ServiceResult<Pick> {
        let mut found: Vec<(String, RenewalState)> = Vec::new();
        for buyer in self.buyers(contract).await? {
            for (series, state) in provider.search_renewals(config, buyer.query()).await? {
                if buyer.matches(state.buyer_email.as_deref())
                    && !found
                        .iter()
                        .any(|(known, _)| known.eq_ignore_ascii_case(&series))
                {
                    found.push((series, state));
                }
            }
        }
        self.choose(contract, found).await
    }

    async fn taken(&self, contract: &Contract, series: &str) -> ServiceResult<Taken> {
        let mut conn = self.services.store().conn().await?;
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT client_id FROM provider_subscriptions WHERE provider = $1 AND lower(external_id) = lower($2)",
        )
        .bind(LAVA)
        .bind(series)
        .fetch_optional(conn.conn())
        .await?;
        if let Some(owner) = owner {
            return Ok(if owner == contract.client_id {
                Taken::Mine
            } else {
                Taken::Other
            });
        }
        let claimed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM legacy_invoices WHERE id <> $1 AND lower(contract_id) = lower($2) \
             AND contract_state IS NOT NULL AND contract_state NOT IN ('missing', 'error'))",
        )
        .bind(contract.id)
        .bind(series)
        .fetch_one(conn.conn())
        .await?;
        Ok(if claimed { Taken::Other } else { Taken::Free })
    }

    async fn choose(
        &self,
        contract: &Contract,
        found: Vec<(String, RenewalState)>,
    ) -> ServiceResult<Pick> {
        let mut free: Vec<(String, RenewalState)> = Vec::new();
        let mut mine: Vec<String> = Vec::new();
        for (series, state) in found {
            if state
                .amount
                .is_some_and(|a| !a.currency.code().eq_ignore_ascii_case(&contract.currency))
            {
                continue;
            }
            match self.taken(contract, &series).await? {
                Taken::Free => free.push((series, state)),
                Taken::Mine => mine.push(series),
                Taken::Other => {}
            }
        }
        if free.is_empty() {
            return Ok(match mine.as_slice() {
                [only] => Pick::Mine(only.clone()),
                [] => Pick::None,
                _ => Pick::Ambiguous,
            });
        }
        let live: Vec<&(String, RenewalState)> = free
            .iter()
            .filter(|(_, s)| matches!(s.status, RenewalStatus::Active | RenewalStatus::Unknown))
            .collect();
        let pool: Vec<&(String, RenewalState)> = if live.is_empty() {
            free.iter().collect()
        } else {
            live
        };
        let same: Vec<&(String, RenewalState)> = pool
            .iter()
            .copied()
            .filter(|(_, s)| s.amount.is_some_and(|a| a.minor == contract.amount_minor))
            .collect();
        let pool = if same.is_empty() { pool } else { same };
        let chosen = match pool.as_slice() {
            [only] => Some(*only),
            _ => {
                let near: Vec<&(String, RenewalState)> = pool
                    .iter()
                    .copied()
                    .filter(|(_, s)| {
                        s.started_at.is_some_and(|at| {
                            (at - contract.created_at).abs() <= Duration::days(NEAR_DAYS)
                        })
                    })
                    .collect();
                match near.as_slice() {
                    [only] => Some(*only),
                    _ => None,
                }
            }
        };
        Ok(match chosen {
            Some((series, state)) => Pick::Found(series.clone(), Box::new(state.clone())),
            None => Pick::Ambiguous,
        })
    }

    async fn bound(
        &self,
        tx: &mut Tx,
        contract: &Contract,
        tariff: Option<TariffId>,
    ) -> ServiceResult<Option<Subscription>> {
        let (target, _, _) = contract.intent();
        if let Some(target) = target
            && let Some(sub) = usable(tx, contract.client_id, target).await?
        {
            return Ok(Some(sub));
        }
        if let Some(order) = contract.order_id
            && let Some(id) = tx.orders().get(OrderId(order)).await?.subscription_id
            && let Some(sub) = usable(tx, contract.client_id, id).await?
        {
            return Ok(Some(sub));
        }
        let live: Vec<Subscription> = tx
            .subscriptions()
            .by_client(ClientId(contract.client_id))
            .await?
            .into_iter()
            .filter(|sub| sub.ended_at.is_none() && sub.tariff_id.is_some())
            .collect();
        let same: Vec<&Subscription> = live
            .iter()
            .filter(|sub| tariff.is_some() && sub.tariff_id == tariff)
            .collect();
        Ok(match (same.as_slice(), live.as_slice()) {
            ([only], _) => Some((*only).clone()),
            (_, [only]) => Some(only.clone()),
            _ => None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn link(
        &self,
        tx: &mut Tx,
        contract: &Contract,
        series: &str,
        state: &RenewalState,
        sub: &Subscription,
        term: &Term,
        now: DateTime<Utc>,
    ) -> ServiceResult<State> {
        let Some(tariff_id) = sub.tariff_id else {
            return Ok(State::Unbound);
        };
        let tariff = match tx.tariffs().get(tariff_id).await {
            Ok(tariff) => tariff,
            Err(sn_store::StoreError::NotFound(_)) => return Ok(State::Unbound),
            Err(err) => return Err(err.into()),
        };
        let periodicity = state.periodicity.clone().unwrap_or_default();
        let cycle = cycle_of(self.services, LAVA, &periodicity, 30);
        let wanted = cycle_days(cycle);
        let period = tariff
            .periods
            .iter()
            .find(|p| Some(p.id) == term.period)
            .or_else(|| {
                tariff
                    .periods
                    .iter()
                    .min_by_key(|p| (i64::from(p.days) - i64::from(wanted)).abs())
            });
        let days = period.map_or(wanted, |p| p.days);
        let (amount_minor, currency) = match state.amount {
            Some(amount) if amount.minor > 0 => (amount.minor, amount.currency.code().to_string()),
            _ => (contract.amount_minor, contract.currency.clone()),
        };
        let next = match (state.status, state.paid_until) {
            (RenewalStatus::Active, Some(until)) if until > now - Duration::days(400) => until,
            _ => cycle.after(now),
        };
        let name = term
            .name
            .clone()
            .or_else(|| state.name.clone())
            .unwrap_or_else(|| tariff.name.clone());
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO provider_subscriptions (provider, external_id, client_id, subscription_id, \
             first_payment_id, tariff_id, period_id, days, offer_id, product_id, periodicity, \
             every_text, name, amount_minor, currency, email, status, next_charge_at, paid_until, \
             check_at, legacy_invoice_id, charges_since, started_at) \
             VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, '', NULL, $8, $9, $10, $11, $12, $13, 'active', \
             $14, $15, $16, $17, $18, $19) \
             ON CONFLICT (provider, external_id) DO NOTHING RETURNING id",
        )
        .bind(LAVA)
        .bind(series)
        .bind(contract.client_id)
        .bind(sub.id.0)
        .bind(tariff.id.0)
        .bind(period.map(|p| p.id.0))
        .bind(i32::try_from(days).unwrap_or(i32::MAX).max(1))
        .bind(&periodicity)
        .bind(state.every.clone().unwrap_or_default())
        .bind(name)
        .bind(amount_minor)
        .bind(currency)
        .bind(state.buyer_email.clone().unwrap_or_default())
        .bind(next)
        .bind(state.paid_until)
        .bind(next_check(next, now))
        .bind(contract.id)
        .bind(now)
        .bind(state.started_at)
        .fetch_optional(tx.conn())
        .await?;
        let Some(id) = id else {
            return Ok(State::Known);
        };
        mark_autorenew(tx, sub.id, true).await?;
        if let Some(link) = link_by_id(tx, id).await? {
            audit(
                tx,
                Actor::System,
                "subscription.renewal_adopted",
                &link,
                serde_json::json!({
                    "legacy_invoice": contract.id,
                    "amount": link.amount(),
                    "every": link.every_text,
                    "periodicity": link.periodicity,
                    "email_known": !link.email.is_empty(),
                    "plan": term.planned,
                }),
            )
            .await?;
        }
        Ok(State::Linked)
    }
}
