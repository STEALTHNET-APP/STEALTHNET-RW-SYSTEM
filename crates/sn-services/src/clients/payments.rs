use super::{ClientOps, order_brief};
use crate::autorenew::AUTORENEW_METHOD;
use crate::error::{ServiceError, ServiceResult};
use crate::inbox::REPEAT_METHOD;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::ids::{ClientId, OrderId, PaymentId, TariffId};
use sn_domain::lang;
use sn_domain::money::{Amount, Currency};
use sn_domain::order::{Order, OrderKind, OrderStatus};
use sn_domain::paging::PageRequest;
use sn_domain::payment::{Payment, PaymentStatus, providers};
use sn_payments::ProviderRegistry;
use sn_store::{Db, StoreError};
use std::collections::HashMap;
use uuid::Uuid;

pub const PAGE: u32 = 20;
pub const PAGE_MAX: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Purchase {
    pub kind: OrderKind,
    pub tariff: Option<String>,
    pub days: Option<u32>,
    pub months: Option<u32>,
    pub label: Option<String>,
    pub extra_devices: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientPayment {
    pub id: Uuid,
    pub status: PaymentStatus,
    pub amount: Amount,
    pub refunded: Option<Amount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bonus: Option<Amount>,
    pub method: String,
    pub autorenew: bool,
    pub repeat: Option<&'static str>,
    pub purchase: Option<Purchase>,
    pub created_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub pay_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientPayments {
    pub items: Vec<ClientPayment>,
    pub next: Option<Uuid>,
}

fn chosen_title(
    registry: &ProviderRegistry,
    currency: Currency,
    payment: &Payment,
) -> Option<String> {
    let provider = payment.provider.as_str();
    let code = payment.pay_method.as_deref()?;
    let custom = registry.snapshot().states.get(provider).and_then(|state| {
        state
            .methods
            .iter()
            .find(|m| m.code == code)
            .and_then(|m| m.title.clone())
    });
    custom.or_else(|| {
        registry
            .get(provider)?
            .method(currency, code)
            .map(|m| sn_payments::english::shown(m.title))
    })
}

pub(crate) fn method_title(
    registry: &ProviderRegistry,
    currency: Currency,
    payment: &Payment,
) -> String {
    if let Some(title) = chosen_title(registry, currency, payment) {
        return title;
    }
    let provider = payment.provider.as_str();
    let base = match registry.get(provider) {
        Some(found) => registry
            .snapshot()
            .states
            .get(provider)
            .and_then(|state| state.label.title.clone())
            .unwrap_or_else(|| {
                sn_payments::english::shown(
                    found
                        .client_label(currency, &registry.config(provider))
                        .title,
                )
            }),
        None if provider == providers::MANUAL && payment.amount.minor == 0 => {
            lang::pick("Бесплатно", "Free").into()
        }
        None if provider == providers::MANUAL => lang::pick("Вручную", "Manual").into(),
        None => lang::pick("Другой способ", "Other method").into(),
    };
    let Some(detail) = payment
        .method
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    else {
        return base;
    };
    match detail.to_ascii_lowercase().as_str() {
        AUTORENEW_METHOD | REPEAT_METHOD | "free" => base,
        "sbp" => lang::pick("СБП", "SBP").into(),
        "bank_card" | "card" => lang::pick("Банковская карта", "Bank card").into(),
        "yoo_money" => lang::pick("ЮMoney", "YooMoney").into(),
        "sberbank" | "sber_pay" => "SberPay".into(),
        _ if matches!(
            provider,
            providers::CRYPTOPAY | providers::HELEKET | providers::CRYPTOMUS
        ) && detail.len() <= 12
            && detail.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            format!("{base}, {}", detail.to_ascii_uppercase())
        }
        _ => base,
    }
}

fn purchase(order: &Order, tariffs: &HashMap<TariffId, String>) -> Purchase {
    let brief = order_brief(order, tariffs);
    Purchase {
        kind: brief.kind,
        tariff: brief.tariff_name,
        days: brief.days,
        months: brief.months,
        label: brief.label,
        extra_devices: brief.extra_devices,
    }
}

impl ClientOps<'_> {
    pub async fn payments(
        &self,
        client_id: ClientId,
        before: Option<Uuid>,
        limit: Option<u32>,
    ) -> ServiceResult<ClientPayments> {
        let mut conn = self.services.store().conn().await?;
        let after = match before {
            Some(cursor) => match conn.payments().by_public_id(cursor).await {
                Ok(payment) if payment.client_id == client_id => Some(payment.id.0),
                Ok(_) | Err(StoreError::NotFound(_)) => {
                    return Err(ServiceError::NotFound("платёж"));
                }
                Err(err) => return Err(err.into()),
            },
            None => None,
        };
        let request = PageRequest::new(Some(limit.unwrap_or(PAGE).clamp(1, PAGE_MAX)), after);
        let page = conn.payments().for_client(client_id, request).await?;
        let order_ids: Vec<OrderId> = page.items.iter().map(|p| p.order_id).collect();
        let orders: HashMap<OrderId, Order> = conn
            .orders()
            .by_ids(&order_ids)
            .await?
            .into_iter()
            .map(|order| (order.id, order))
            .collect();
        let tariffs: HashMap<TariffId, String> = conn
            .tariffs()
            .list(true)
            .await?
            .into_iter()
            .map(|tariff| (tariff.id, tariff.name))
            .collect();
        let payment_ids: Vec<PaymentId> = page.items.iter().map(|p| p.id).collect();
        let outcomes = conn.payments().outcomes(&payment_ids).await?;
        let settings = self.services.settings();
        let base = settings
            .public_url
            .as_deref()
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_string();
        let registry = self.services.payments();
        let now = self.services.now();
        let next = page
            .next
            .and_then(|_| page.items.last().map(|payment| payment.public_id));
        let items = page
            .items
            .into_iter()
            .map(|payment| {
                let outcome = outcomes.get(&payment.id).copied().unwrap_or_default();
                let open = payment.status == PaymentStatus::Pending
                    && payment.pay_url.is_some()
                    && payment.expires_at.is_none_or(|at| at > now)
                    && orders
                        .get(&payment.order_id)
                        .is_none_or(|order| order.status != OrderStatus::Review);
                ClientPayment {
                    id: payment.public_id,
                    status: payment.status,
                    amount: payment.amount,
                    refunded: (outcome.refunded_minor > 0)
                        .then(|| Amount::new(outcome.refunded_minor, payment.amount.currency)),
                    bonus: (outcome.bonus_minor > 0)
                        .then(|| Amount::new(outcome.bonus_minor, payment.amount.currency)),
                    method: method_title(registry, settings.currency, &payment),
                    autorenew: payment.method.as_deref() == Some(AUTORENEW_METHOD),
                    repeat: outcome.repeat.map(|action| action.as_str()),
                    purchase: orders
                        .get(&payment.order_id)
                        .map(|order| purchase(order, &tariffs)),
                    created_at: payment.created_at,
                    paid_at: payment.paid_at,
                    expires_at: payment.expires_at,
                    pay_url: open.then(|| format!("{base}/api/pay/{}", payment.public_id)),
                }
            })
            .collect();
        Ok(ClientPayments { items, next })
    }
}
