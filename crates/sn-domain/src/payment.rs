use crate::DomainError;
use crate::access::ClientChannel;
use crate::ids::{AdminId, ClientId, OrderId, PaymentId, RefundId};
use crate::money::{Amount, Currency, Money};
use crate::pricing::BonusTerms;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

pub const REPLACED_REASON: &str = "счёт заменён новым счётом с тем же промокодом";

pub mod providers {
    pub const BALANCE: &str = "balance";
    pub const STARS: &str = "stars";
    pub const MANUAL: &str = "manual";
    pub const CRYPTOPAY: &str = "cryptopay";
    pub const HELEKET: &str = "heleket";
    pub const LAVA: &str = "lava";
    pub const YOOKASSA: &str = "yookassa";
    pub const PLATEGA: &str = "platega";
    pub const YOOMONEY: &str = "yoomoney";
    pub const CLOUDPAYMENTS: &str = "cloudpayments";
    pub const OVERPAY: &str = "overpay";
    pub const GRAM: &str = "gram";
    pub const ROLLYPAY: &str = "rollypay";
    pub const CRYPTOMUS: &str = "cryptomus";
    pub const MULENPAY: &str = "mulenpay";
    pub const PAL24: &str = "pal24";
    pub const KASSA_AI: &str = "kassa_ai";
    pub const AURAPAY: &str = "aurapay";
    pub const TRIBUTE: &str = "tribute";
    pub const WATA: &str = "wata";
    pub const FREEKASSA: &str = "freekassa";
    pub const ROBOKASSA: &str = "robokassa";
    pub const CISPAY: &str = "cispay";
    pub const PARITYPAY: &str = "paritypay";
    pub const ANTILOPAY: &str = "antilopay";
}

const fn shown() -> bool {
    true
}

pub const METHOD_TITLE_MAX: usize = 40;
pub const LABEL_HINT_MAX: usize = 80;
pub const AUTORENEW_METHOD: &str = "autorenew";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonLabel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodChoice {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopupBonus {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub percent: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Money>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Money>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
}

impl TopupBonus {
    pub fn foreign_limits(&self, currency: Currency) -> bool {
        [self.min, self.max]
            .into_iter()
            .flatten()
            .any(|limit| limit.currency != currency)
    }

    pub fn terms(&self, currency: Currency, method: Option<&str>) -> Option<BonusTerms> {
        let covered = self.methods.is_empty()
            || method.is_some_and(|code| self.methods.iter().any(|m| m == code));
        (self.enabled && self.percent > 0 && covered && !self.foreign_limits(currency)).then(|| {
            BonusTerms {
                percent: self.percent,
                min_minor: self.min.map(|m| m.minor),
                max_minor: self.max.map(|m| m.minor),
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaymentChannels {
    #[serde(default = "shown")]
    pub cabinet: bool,
    #[serde(default = "shown")]
    pub bot: bool,
    #[serde(default = "shown")]
    pub miniapp: bool,
}

impl Default for PaymentChannels {
    fn default() -> Self {
        PaymentChannels::ALL
    }
}

impl PaymentChannels {
    pub const ALL: PaymentChannels = PaymentChannels {
        cabinet: true,
        bot: true,
        miniapp: true,
    };

    pub const fn allows(self, channel: ClientChannel) -> bool {
        match channel {
            ClientChannel::Cabinet => self.cabinet,
            ClientChannel::Bot => self.bot,
            ClientChannel::Miniapp => self.miniapp,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(pub String);

impl ProviderId {
    pub fn new(id: impl Into<String>) -> Self {
        ProviderId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_internal(&self) -> bool {
        matches!(self.0.as_str(), providers::BALANCE | providers::MANUAL)
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(value: &str) -> Self {
        ProviderId(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Pending,
    Paid,
    Failed,
    Canceled,
    Refunded,
    PartiallyRefunded,
}

impl PaymentStatus {
    pub const ALL: [PaymentStatus; 6] = [
        PaymentStatus::Pending,
        PaymentStatus::Paid,
        PaymentStatus::Failed,
        PaymentStatus::Canceled,
        PaymentStatus::Refunded,
        PaymentStatus::PartiallyRefunded,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            PaymentStatus::Pending => "pending",
            PaymentStatus::Paid => "paid",
            PaymentStatus::Failed => "failed",
            PaymentStatus::Canceled => "canceled",
            PaymentStatus::Refunded => "refunded",
            PaymentStatus::PartiallyRefunded => "partially_refunded",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        PaymentStatus::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .ok_or_else(|| DomainError::unknown("payment_status", value))
    }

    pub const fn is_paid_like(self) -> bool {
        matches!(
            self,
            PaymentStatus::Paid | PaymentStatus::Refunded | PaymentStatus::PartiallyRefunded
        )
    }

    pub const fn can_become(self, to: PaymentStatus) -> bool {
        use PaymentStatus::*;
        matches!(
            (self, to),
            (Pending, Paid)
                | (Pending, Failed)
                | (Pending, Canceled)
                | (Failed, Paid)
                | (Canceled, Paid)
                | (Paid, Refunded)
                | (Paid, PartiallyRefunded)
                | (PartiallyRefunded, PartiallyRefunded)
                | (PartiallyRefunded, Refunded)
        )
    }

    pub fn transition(self, to: PaymentStatus) -> Result<bool, DomainError> {
        if self == to && to != PaymentStatus::PartiallyRefunded {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payment {
    pub id: PaymentId,
    pub public_id: Uuid,
    pub order_id: OrderId,
    pub client_id: ClientId,
    pub provider: ProviderId,
    pub method: Option<String>,
    pub pay_method: Option<String>,
    pub amount: Amount,
    pub status: PaymentStatus,
    pub provider_txid: Option<String>,
    pub pay_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub paid_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPayment {
    pub order_id: OrderId,
    pub client_id: ClientId,
    pub provider: ProviderId,
    pub method: Option<String>,
    pub amount: Amount,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AmountCheck {
    Ok,
    Overpaid { expected: Amount, received: Amount },
    Underpaid { expected: Amount, received: Amount },
    CurrencyMismatch { expected: Amount, received: Amount },
    Missing,
}

impl AmountCheck {
    pub const fn accepts(self) -> bool {
        matches!(self, AmountCheck::Ok | AmountCheck::Overpaid { .. })
    }
}

pub fn check_paid_amount(expected: Amount, received: Option<Amount>) -> AmountCheck {
    let Some(received) = received else {
        return AmountCheck::Missing;
    };
    if received.currency != expected.currency {
        AmountCheck::CurrencyMismatch { expected, received }
    } else if received.minor < expected.minor {
        AmountCheck::Underpaid { expected, received }
    } else if received.minor > expected.minor {
        AmountCheck::Overpaid { expected, received }
    } else {
        AmountCheck::Ok
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Dispute,
    Chargeback,
    UnmatchedRefund,
    RefundReported,
    AmlCheck,
    ManualCredit,
}

impl NoticeKind {
    pub const ALL: [NoticeKind; 6] = [
        NoticeKind::Dispute,
        NoticeKind::Chargeback,
        NoticeKind::UnmatchedRefund,
        NoticeKind::RefundReported,
        NoticeKind::AmlCheck,
        NoticeKind::ManualCredit,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            NoticeKind::Dispute => "dispute",
            NoticeKind::Chargeback => "chargeback",
            NoticeKind::UnmatchedRefund => "unmatched_refund",
            NoticeKind::RefundReported => "refund_reported",
            NoticeKind::AmlCheck => "aml_check",
            NoticeKind::ManualCredit => "manual_credit",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        NoticeKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("notice_kind", value))
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            NoticeKind::Dispute => "Спор по платежу",
            NoticeKind::Chargeback => "Чарджбэк",
            NoticeKind::UnmatchedRefund => "Возврат не привязан к оплате",
            NoticeKind::RefundReported => "Платёжная система сообщила о возврате",
            NoticeKind::AmlCheck => "Платёж задержан проверкой AML",
            NoticeKind::ManualCredit => "Деньги ждут ручного зачисления",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundMethod {
    Balance,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundSubscriptionAction {
    Keep,
    Cut,
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refund {
    pub id: RefundId,
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub method: RefundMethod,
    pub subscription_action: RefundSubscriptionAction,
    pub reason: Option<String>,
    pub admin_id: Option<AdminId>,
    pub provider_refund_id: Option<String>,
    #[serde(default)]
    pub bonus_minor: i64,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::PayCurrency;

    #[test]
    fn бонус_действует_только_включённым_для_выбранных_способов_и_в_своей_валюте() {
        let rub = Currency::Rub;
        let off: TopupBonus = serde_json::from_str("{}").unwrap();
        assert_eq!(off, TopupBonus::default());
        assert_eq!(off.terms(rub, None), None);
        let all = TopupBonus {
            enabled: true,
            percent: 5,
            ..TopupBonus::default()
        };
        assert_eq!(all.terms(rub, None).unwrap().percent, 5);
        assert!(all.terms(rub, Some("sbp")).is_some());
        assert_eq!(
            TopupBonus {
                enabled: false,
                ..all.clone()
            }
            .terms(rub, None),
            None
        );
        assert_eq!(
            TopupBonus {
                percent: 0,
                ..all.clone()
            }
            .terms(rub, None),
            None
        );
        let some = TopupBonus {
            methods: vec!["sbp".into()],
            min: Some(Money::new(50_000, rub)),
            max: Some(Money::new(10_000, rub)),
            ..all
        };
        assert_eq!(
            some.terms(rub, Some("sbp")),
            Some(BonusTerms {
                percent: 5,
                min_minor: Some(50_000),
                max_minor: Some(10_000),
            })
        );
        assert_eq!(some.terms(rub, Some("card")), None);
        assert_eq!(some.terms(rub, None), None);
        assert_eq!(some.terms(Currency::Usd, Some("sbp")), None);
        assert!(some.foreign_limits(Currency::Usd));
    }

    #[test]
    fn недоплата_никогда_не_считается_оплатой() {
        let expected = Amount::new(699, PayCurrency::Usd);
        assert!(!check_paid_amount(expected, Some(Amount::new(698, PayCurrency::Usd))).accepts());
        assert!(!check_paid_amount(expected, Some(Amount::new(699, PayCurrency::Rub))).accepts());
        assert!(!check_paid_amount(expected, None).accepts());
        assert!(check_paid_amount(expected, Some(expected)).accepts());
        assert!(check_paid_amount(expected, Some(Amount::new(700, PayCurrency::Usd))).accepts());
    }

    #[test]
    fn оплата_после_отмены_и_ошибки_засчитывается() {
        assert_eq!(
            PaymentStatus::Canceled.transition(PaymentStatus::Paid),
            Ok(true)
        );
        assert_eq!(
            PaymentStatus::Failed.transition(PaymentStatus::Paid),
            Ok(true)
        );
        assert_eq!(
            PaymentStatus::Paid.transition(PaymentStatus::Paid),
            Ok(false)
        );
        assert!(
            PaymentStatus::Refunded
                .transition(PaymentStatus::Paid)
                .is_err()
        );
    }
}

#[cfg(test)]
mod channel_tests {
    use super::*;

    #[test]
    fn каналы_оплаты_по_умолчанию_все_и_читаются_из_старых_настроек() {
        assert_eq!(PaymentChannels::default(), PaymentChannels::ALL);
        let old: PaymentChannels = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(old, PaymentChannels::ALL);
        let site_only: PaymentChannels =
            serde_json::from_value(serde_json::json!({"bot": false, "miniapp": false})).unwrap();
        assert!(site_only.allows(ClientChannel::Cabinet));
        assert!(!site_only.allows(ClientChannel::Bot));
        assert!(!site_only.allows(ClientChannel::Miniapp));
    }
}
