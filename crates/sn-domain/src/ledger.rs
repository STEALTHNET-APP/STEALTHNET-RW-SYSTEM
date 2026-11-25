use crate::DomainError;
use crate::ids::{AdminId, ClientId, LedgerEntryId, OrderId, PaymentId};
use crate::money::Money;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wallet {
    Main,
    Referral,
}

impl Wallet {
    pub const fn as_str(self) -> &'static str {
        match self {
            Wallet::Main => "main",
            Wallet::Referral => "referral",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "main" => Ok(Wallet::Main),
            "referral" => Ok(Wallet::Referral),
            other => Err(DomainError::unknown("wallet", other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKind {
    TopUp,
    Purchase,
    AutoRenew,
    Refund,
    ReferralCredit,
    ReferralReversal,
    WithdrawalHold,
    WithdrawalRelease,
    Contest,
    Roulette,
    AdminAdjust,
    MigrationOpening,
    RefundHold,
    RefundRelease,
    TopupBonus,
    TopupBonusReversal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Credit,
    Debit,
    Either,
}

impl LedgerKind {
    pub const ALL: [LedgerKind; 16] = [
        LedgerKind::TopUp,
        LedgerKind::Purchase,
        LedgerKind::AutoRenew,
        LedgerKind::Refund,
        LedgerKind::ReferralCredit,
        LedgerKind::ReferralReversal,
        LedgerKind::WithdrawalHold,
        LedgerKind::WithdrawalRelease,
        LedgerKind::Contest,
        LedgerKind::Roulette,
        LedgerKind::AdminAdjust,
        LedgerKind::MigrationOpening,
        LedgerKind::RefundHold,
        LedgerKind::RefundRelease,
        LedgerKind::TopupBonus,
        LedgerKind::TopupBonusReversal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            LedgerKind::TopUp => "topup",
            LedgerKind::Purchase => "purchase",
            LedgerKind::AutoRenew => "autorenew",
            LedgerKind::Refund => "refund",
            LedgerKind::ReferralCredit => "referral_credit",
            LedgerKind::ReferralReversal => "referral_reversal",
            LedgerKind::WithdrawalHold => "withdrawal_hold",
            LedgerKind::WithdrawalRelease => "withdrawal_release",
            LedgerKind::Contest => "contest",
            LedgerKind::Roulette => "roulette",
            LedgerKind::AdminAdjust => "admin_adjust",
            LedgerKind::MigrationOpening => "migration_opening",
            LedgerKind::RefundHold => "refund_hold",
            LedgerKind::RefundRelease => "refund_release",
            LedgerKind::TopupBonus => "topup_bonus",
            LedgerKind::TopupBonusReversal => "topup_bonus_reversal",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        LedgerKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("ledger_kind", value))
    }

    pub const fn direction(self) -> Direction {
        match self {
            LedgerKind::TopUp
            | LedgerKind::Refund
            | LedgerKind::ReferralCredit
            | LedgerKind::WithdrawalRelease
            | LedgerKind::MigrationOpening
            | LedgerKind::RefundRelease
            | LedgerKind::TopupBonus => Direction::Credit,
            LedgerKind::Purchase
            | LedgerKind::AutoRenew
            | LedgerKind::ReferralReversal
            | LedgerKind::WithdrawalHold
            | LedgerKind::RefundHold
            | LedgerKind::TopupBonusReversal => Direction::Debit,
            LedgerKind::Contest | LedgerKind::Roulette | LedgerKind::AdminAdjust => {
                Direction::Either
            }
        }
    }

    pub const fn wallet_allowed(self, wallet: Wallet) -> bool {
        match self {
            LedgerKind::ReferralCredit
            | LedgerKind::ReferralReversal
            | LedgerKind::WithdrawalHold
            | LedgerKind::WithdrawalRelease => matches!(wallet, Wallet::Referral),
            LedgerKind::TopUp
            | LedgerKind::Purchase
            | LedgerKind::AutoRenew
            | LedgerKind::Refund
            | LedgerKind::RefundHold
            | LedgerKind::RefundRelease
            | LedgerKind::TopupBonus
            | LedgerKind::TopupBonusReversal => {
                matches!(wallet, Wallet::Main)
            }
            LedgerKind::Contest
            | LedgerKind::Roulette
            | LedgerKind::AdminAdjust
            | LedgerKind::MigrationOpening => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: LedgerEntryId,
    pub client_id: ClientId,
    pub wallet: Wallet,
    pub amount: Money,
    pub kind: LedgerKind,
    pub order_id: Option<OrderId>,
    pub payment_id: Option<PaymentId>,
    pub admin_id: Option<AdminId>,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewLedgerEntry {
    pub client_id: ClientId,
    pub wallet: Wallet,
    pub amount: Money,
    pub kind: LedgerKind,
    pub order_id: Option<OrderId>,
    pub payment_id: Option<PaymentId>,
    pub admin_id: Option<AdminId>,
    pub comment: Option<String>,
    pub idempotency_key: Option<String>,
}

impl NewLedgerEntry {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.amount.is_zero() {
            return Err(DomainError::LedgerRule("нулевая сумма"));
        }
        match (self.kind.direction(), self.amount.is_negative()) {
            (Direction::Credit, true) => {
                return Err(DomainError::LedgerRule(
                    "начисление должно быть положительным",
                ));
            }
            (Direction::Debit, false) => {
                return Err(DomainError::LedgerRule(
                    "списание должно быть отрицательным",
                ));
            }
            _ => {}
        }
        if !self.kind.wallet_allowed(self.wallet) {
            return Err(DomainError::LedgerRule("операция не для этого кошелька"));
        }
        if self.kind == LedgerKind::AdminAdjust
            && (self.admin_id.is_none()
                || self.comment.as_deref().is_none_or(|c| c.trim().is_empty()))
        {
            return Err(DomainError::LedgerRule(
                "ручная правка баланса требует администратора и комментарий",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn entry(kind: LedgerKind, wallet: Wallet, minor: i64) -> NewLedgerEntry {
        NewLedgerEntry {
            client_id: ClientId(1),
            wallet,
            amount: Money::new(minor, Currency::Usd),
            kind,
            order_id: None,
            payment_id: None,
            admin_id: None,
            comment: None,
            idempotency_key: None,
        }
    }

    #[test]
    fn вывод_только_из_реферального_кошелька() {
        assert!(
            entry(LedgerKind::WithdrawalHold, Wallet::Main, -100)
                .validate()
                .is_err()
        );
        assert!(
            entry(LedgerKind::WithdrawalHold, Wallet::Referral, -100)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn знак_суммы_по_виду_операции() {
        assert!(
            entry(LedgerKind::TopUp, Wallet::Main, -100)
                .validate()
                .is_err()
        );
        assert!(
            entry(LedgerKind::Purchase, Wallet::Main, 100)
                .validate()
                .is_err()
        );
        assert!(
            entry(LedgerKind::Purchase, Wallet::Main, -100)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn удержание_под_возврат_списывает_основной_кошелёк_а_сторно_возвращает() {
        assert!(
            entry(LedgerKind::RefundHold, Wallet::Main, -100)
                .validate()
                .is_ok()
        );
        assert!(
            entry(LedgerKind::RefundHold, Wallet::Main, 100)
                .validate()
                .is_err()
        );
        assert!(
            entry(LedgerKind::RefundHold, Wallet::Referral, -100)
                .validate()
                .is_err()
        );
        assert!(
            entry(LedgerKind::RefundRelease, Wallet::Main, 100)
                .validate()
                .is_ok()
        );
        assert!(
            entry(LedgerKind::RefundRelease, Wallet::Main, -100)
                .validate()
                .is_err()
        );
        for kind in LedgerKind::ALL {
            assert_eq!(LedgerKind::parse(kind.as_str()).unwrap(), kind);
        }
        assert_eq!(LedgerKind::RefundHold.as_str(), "refund_hold");
        assert_eq!(LedgerKind::RefundRelease.as_str(), "refund_release");
    }

    #[test]
    fn бонус_за_пополнение_начисляется_и_снимается_только_на_основном_кошельке() {
        for (kind, minor) in [
            (LedgerKind::TopupBonus, 25),
            (LedgerKind::TopupBonusReversal, -25),
        ] {
            assert!(entry(kind, Wallet::Main, minor).validate().is_ok());
            assert!(entry(kind, Wallet::Main, -minor).validate().is_err());
            assert!(entry(kind, Wallet::Referral, minor).validate().is_err());
        }
        assert_eq!(LedgerKind::TopupBonus.as_str(), "topup_bonus");
        assert_eq!(
            LedgerKind::TopupBonusReversal.as_str(),
            "topup_bonus_reversal"
        );
    }

    #[test]
    fn ручная_правка_только_с_комментарием() {
        let mut e = entry(LedgerKind::AdminAdjust, Wallet::Main, 500);
        assert!(e.validate().is_err());
        e.admin_id = Some(AdminId(1));
        e.comment = Some("Компенсация за простой".into());
        assert!(e.validate().is_ok());
    }
}
