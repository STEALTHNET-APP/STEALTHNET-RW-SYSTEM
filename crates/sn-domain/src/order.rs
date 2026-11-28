use crate::DomainError;
use crate::ids::{ClientId, OrderId, PeriodId, SubscriptionId, TariffId};
use crate::module::ModuleId;
use crate::money::Money;
use crate::plan::FulfillmentPlan;
use crate::pricing::Quote;
use crate::subscription::SubscriptionKind;
use crate::tariff::TariffSnapshot;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderKind {
    New,
    Renewal,
    Change,
    TopUp,
    Trial,
    Module,
    AdminGrant,
    ModuleGrant,
}

impl OrderKind {
    pub const ALL: [OrderKind; 8] = [
        OrderKind::New,
        OrderKind::Renewal,
        OrderKind::Change,
        OrderKind::TopUp,
        OrderKind::Trial,
        OrderKind::Module,
        OrderKind::AdminGrant,
        OrderKind::ModuleGrant,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            OrderKind::New => "new",
            OrderKind::Renewal => "renewal",
            OrderKind::Change => "change",
            OrderKind::TopUp => "topup",
            OrderKind::Trial => "trial",
            OrderKind::Module => "module",
            OrderKind::AdminGrant => "admin_grant",
            OrderKind::ModuleGrant => "module_grant",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        OrderKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("order_kind", value))
    }

    pub const fn needs_remnawave(self) -> bool {
        !matches!(self, OrderKind::TopUp | OrderKind::Module)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Pending,
    Paid,
    Fulfilled,
    Review,
    Canceled,
    Refunded,
}

impl OrderStatus {
    pub const ALL: [OrderStatus; 6] = [
        OrderStatus::Pending,
        OrderStatus::Paid,
        OrderStatus::Fulfilled,
        OrderStatus::Review,
        OrderStatus::Canceled,
        OrderStatus::Refunded,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            OrderStatus::Pending => "pending",
            OrderStatus::Paid => "paid",
            OrderStatus::Fulfilled => "fulfilled",
            OrderStatus::Review => "review",
            OrderStatus::Canceled => "canceled",
            OrderStatus::Refunded => "refunded",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        OrderStatus::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .ok_or_else(|| DomainError::unknown("order_status", value))
    }

    pub const fn can_become(self, to: OrderStatus) -> bool {
        use OrderStatus::*;
        matches!(
            (self, to),
            (Pending, Paid)
                | (Pending, Canceled)
                | (Pending, Review)
                | (Canceled, Paid)
                | (Paid, Fulfilled)
                | (Paid, Review)
                | (Paid, Refunded)
                | (Review, Paid)
                | (Review, Fulfilled)
                | (Review, Refunded)
                | (Review, Canceled)
                | (Fulfilled, Refunded)
        )
    }

    pub fn transition(self, to: OrderStatus) -> Result<bool, DomainError> {
        if self == to {
            Ok(false)
        } else if self.can_become(to) {
            Ok(true)
        } else {
            Err(DomainError::InvalidTransition {
                from: self.as_str(),
                to: to.as_str(),
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderChannel {
    Bot,
    Miniapp,
    Cabinet,
    Admin,
    Autorenew,
}

impl OrderChannel {
    pub const ALL: [OrderChannel; 5] = [
        OrderChannel::Bot,
        OrderChannel::Miniapp,
        OrderChannel::Cabinet,
        OrderChannel::Admin,
        OrderChannel::Autorenew,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            OrderChannel::Bot => "bot",
            OrderChannel::Miniapp => "miniapp",
            OrderChannel::Cabinet => "cabinet",
            OrderChannel::Admin => "admin",
            OrderChannel::Autorenew => "autorenew",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        OrderChannel::ALL
            .into_iter()
            .find(|c| c.as_str() == value)
            .ok_or_else(|| DomainError::unknown("order_channel", value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PurchaseIntent {
    Subscription {
        tariff_id: TariffId,
        period_id: PeriodId,
        extra_devices: u32,
        target: Option<SubscriptionId>,
    },
    TopUp {
        amount: Money,
    },
    Trial,
    Module {
        module: ModuleId,
        kind: String,
        payload: serde_json::Value,
    },
    AdminGrant {
        tariff_id: TariffId,
        period_id: Option<PeriodId>,
        days: u32,
        note: Option<String>,
    },
    Imported {
        source: String,
        old_id: String,
        label: Option<String>,
    },
    ModuleGrant {
        module: ModuleId,
        reference: String,
        tariff_id: Option<TariffId>,
        period_id: Option<PeriodId>,
        days: u32,
    },
}

impl PurchaseIntent {
    pub fn granted_kind(&self) -> Option<SubscriptionKind> {
        match self {
            PurchaseIntent::ModuleGrant { module, .. } => Some(match module {
                ModuleId::Gifts => SubscriptionKind::Gift,
                ModuleId::Promolinks => SubscriptionKind::Promo,
                _ => SubscriptionKind::Manual,
            }),
            _ => None,
        }
    }

    pub const fn custom_grant(&self) -> bool {
        matches!(
            self,
            PurchaseIntent::ModuleGrant {
                tariff_id: None,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub public_id: Uuid,
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
    pub kind: OrderKind,
    pub status: OrderStatus,
    pub intent: PurchaseIntent,
    pub quote: Quote,
    pub snapshot: Option<TariffSnapshot>,
    pub plan: Option<FulfillmentPlan>,
    pub created_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
    pub fulfilled_at: Option<DateTime<Utc>>,
}

pub const OPTIONS_DEVICES: &str = "devices";

impl Order {
    pub fn bought_devices(&self) -> u32 {
        match &self.intent {
            PurchaseIntent::Subscription { extra_devices, .. } => *extra_devices,
            PurchaseIntent::Module {
                module: ModuleId::Options,
                kind,
                payload,
            } if kind == OPTIONS_DEVICES => payload
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .and_then(|count| u32::try_from(count).ok())
                .unwrap_or(0),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewOrder {
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
    pub kind: OrderKind,
    pub intent: PurchaseIntent,
    pub quote: Quote,
    pub snapshot: Option<TariffSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn поздняя_оплата_отменённого_заказа_принимается() {
        assert_eq!(
            OrderStatus::Canceled.transition(OrderStatus::Paid),
            Ok(true)
        );
        assert_eq!(OrderStatus::Paid.transition(OrderStatus::Paid), Ok(false));
        assert!(
            OrderStatus::Fulfilled
                .transition(OrderStatus::Pending)
                .is_err()
        );
        assert!(OrderStatus::Refunded.transition(OrderStatus::Paid).is_err());
    }

    #[test]
    fn задержанный_проверкой_заказ_ждёт_решения_и_может_оплатиться_или_отмениться() {
        assert_eq!(
            OrderStatus::Pending.transition(OrderStatus::Review),
            Ok(true)
        );
        assert_eq!(OrderStatus::Review.transition(OrderStatus::Paid), Ok(true));
        assert_eq!(
            OrderStatus::Review.transition(OrderStatus::Canceled),
            Ok(true)
        );
        assert!(
            OrderStatus::Fulfilled
                .transition(OrderStatus::Review)
                .is_err()
        );
        assert!(
            OrderStatus::Canceled
                .transition(OrderStatus::Review)
                .is_err()
        );
    }

    #[test]
    fn канал_покупки_пишется_и_читается_одним_словом() {
        for channel in OrderChannel::ALL {
            assert_eq!(OrderChannel::parse(channel.as_str()), Ok(channel));
            assert_eq!(
                serde_json::to_value(channel).unwrap(),
                serde_json::json!(channel.as_str())
            );
        }
        assert!(OrderChannel::parse("site").is_err());
    }
}
