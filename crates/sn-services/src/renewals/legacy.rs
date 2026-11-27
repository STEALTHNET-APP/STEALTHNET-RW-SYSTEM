use super::*;
use sn_domain::ids::{PeriodId, TariffId};
use sn_domain::payment::providers::LAVA;
use sn_payments::{ProviderConfig, RenewalState};
use uuid::Uuid;

const ADOPT_BATCH: i64 = 200;
const RECHECK_HOURS: i64 = 24;
const RETRY_HOURS: i64 = 1;
const AFTER_CHARGE_MINUTES: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Linked,
    Unbound,
    Conflict,
    Ended,
    Missing,
    Error,
}

impl State {
    const fn as_str(self) -> &'static str {
        match self {
            State::Linked => "linked",
            State::Unbound => "unbound",
            State::Conflict => "conflict",
            State::Ended => "ended",
            State::Missing => "missing",
            State::Error => "error",
        }
    }

    fn next_check(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            State::Unbound | State::Conflict => Some(now + Duration::hours(RECHECK_HOURS)),
            State::Error => Some(now + Duration::hours(RETRY_HOURS)),
            State::Linked | State::Ended | State::Missing => None,
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
    pub(super) async fn adopt_legacy(&self, now: DateTime<Utc>) -> ServiceResult<usize> {
        let Some(provider) = self.provider(LAVA) else {
            return Ok(0);
        };
        let config = self.services.payments().config(LAVA);
        if config.get("api_key").is_none() {
            return Ok(0);
        }
        let due: Vec<Contract> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_as(
                "SELECT id, client_id, intent, external_id, order_ref, order_id, contract_id, \
                 amount_minor, currency FROM legacy_invoices \
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
        let mut adopted = 0usize;
        for contract in due {
            match self.adopt(provider.as_ref(), &config, &contract, now).await {
                Ok(State::Linked) => adopted += 1,
                Ok(_) => {}
                Err(err) => {
                    let mut conn = self.services.store().conn().await?;
                    mark(&mut conn, contract.id, State::Error, None, now).await?;
                    tracing::warn!(legacy_invoice = contract.id, error = %err, "договор Lava.top старой панели не проверен, повторим");
                    break;
                }
            }
        }
        Ok(adopted)
    }

    async fn adopt(
        &self,
        provider: &dyn PaymentProvider,
        config: &ProviderConfig,
        contract: &Contract,
        now: DateTime<Utc>,
    ) -> ServiceResult<State> {
        let mut found: Option<(String, RenewalState)> = None;
        for candidate in contract.candidates() {
            if let Some(state) = provider.find_renewal(config, &candidate).await? {
                found = Some((candidate, state));
                break;
            }
        }
        let Some((series, state)) = found else {
            let mut conn = self.services.store().conn().await?;
            mark(&mut conn, contract.id, State::Missing, None, now).await?;
            return Ok(State::Missing);
        };
        let mut tx = self.services.store().begin().await?;
        let outcome = match state.status {
            RenewalStatus::Cancelled | RenewalStatus::Failed => State::Ended,
            RenewalStatus::Active | RenewalStatus::Unknown => {
                if lock_series(&mut tx, LAVA, &series).await?.is_some() {
                    State::Linked
                } else {
                    match self.bound(&mut tx, contract).await? {
                        None => State::Unbound,
                        Some(sub) if live_link(&mut tx, sub.id).await?.is_some() => State::Conflict,
                        Some(sub) => {
                            self.link(&mut tx, contract, &series, &state, &sub, now)
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

    async fn bound(&self, tx: &mut Tx, contract: &Contract) -> ServiceResult<Option<Subscription>> {
        let (target, tariff, _) = contract.intent();
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

    async fn link(
        &self,
        tx: &mut Tx,
        contract: &Contract,
        series: &str,
        state: &RenewalState,
        sub: &Subscription,
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
        let (_, _, period) = contract.intent();
        let period = tariff
            .periods
            .iter()
            .find(|p| Some(p.id) == period)
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
        let name = state.name.clone().unwrap_or_else(|| tariff.name.clone());
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
            return Ok(State::Linked);
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
                }),
            )
            .await?;
        }
        Ok(State::Linked)
    }
}
