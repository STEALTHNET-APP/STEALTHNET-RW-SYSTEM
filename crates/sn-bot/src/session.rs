use chrono::Duration;
use serde::{Deserialize, Serialize};
use sn_domain::ids::{PeriodId, SubscriptionId, TariffId};
use sn_services::Services;
use sn_store::{Db, StoreResult};

pub const TTL_MINUTES: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Awaiting {
    PromoCode,
    PromoForCart,
    TopUpAmount,
    SupportMessage,
    WithdrawalAmount,
    WithdrawalDetails,
    GiftCode,
    EmailAddress,
    EmailProof,
    EmailCode,
    SitePassword,
    ReceiptEmail,
}

impl Awaiting {
    pub fn secret(self) -> bool {
        matches!(self, Awaiting::EmailProof | Awaiting::SitePassword)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cart {
    pub subscription_id: Option<SubscriptionId>,
    pub tariff_id: Option<TariffId>,
    pub period_id: Option<PeriodId>,
    pub extra_devices: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePick {
    pub subscription_id: SubscriptionId,
    pub hwids: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalDraft {
    pub method: Option<String>,
    pub amount: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub awaiting: Option<Awaiting>,
    #[serde(default)]
    pub cart: Option<Cart>,
    #[serde(default)]
    pub promo: Option<String>,
    #[serde(default)]
    pub days_code: Option<String>,
    #[serde(default)]
    pub pending_start: Option<String>,
    #[serde(default)]
    pub login_token: Option<String>,
    #[serde(default)]
    pub devices: Option<DevicePick>,
    #[serde(default)]
    pub topup_minor: Option<i64>,
    #[serde(default)]
    pub withdrawal: Option<WithdrawalDraft>,
    #[serde(default)]
    pub support_ticket: Option<i64>,
    #[serde(default)]
    pub gift_code: Option<String>,
    #[serde(default)]
    pub promo_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_resume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_back: Option<String>,
}

impl Session {
    pub async fn load(services: &Services, chat_id: i64) -> StoreResult<Session> {
        let mut conn = services.store().conn().await?;
        let state = conn.bot_state().dialog(chat_id).await?;
        Ok(state
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default())
    }

    pub async fn save(&self, services: &Services, chat_id: i64) -> StoreResult<()> {
        let mut conn = services.store().conn().await?;
        if self == &Session::default() {
            return conn.bot_state().clear_dialog(chat_id).await;
        }
        let value = serde_json::to_value(self)?;
        conn.bot_state()
            .set_dialog(
                chat_id,
                &value,
                services.now() + Duration::minutes(TTL_MINUTES),
            )
            .await
    }

    pub fn wait_for(&mut self, awaiting: Awaiting) {
        self.awaiting = Some(awaiting);
    }
}
