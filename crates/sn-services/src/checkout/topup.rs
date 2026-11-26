use super::{Channel, Checkout, CheckoutResult, reachable, telegram_linked};
use crate::error::{ServiceError, ServiceResult};
use serde::Serialize;
use sn_domain::ids::ClientId;
use sn_domain::ledger::Wallet;
use sn_domain::money::{Currency, Money};
use sn_domain::pricing::{BonusTerms, topup_presets};
use sn_payments::{MethodsContext, ProviderKind};
use sn_store::Db;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MethodBonus {
    pub percent: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<Money>,
}

impl MethodBonus {
    fn of(terms: BonusTerms, currency: Currency) -> Self {
        let money = |minor: i64| Money::new(minor, currency);
        MethodBonus {
            percent: terms.percent,
            min: terms.min_minor.map(money),
            max: terms.max_minor.map(money),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopUpMethod {
    pub provider: &'static str,
    pub method: Option<String>,
    pub title: String,
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amounts: Option<Vec<Money>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bonus: Option<MethodBonus>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_email: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopUpOptions {
    pub currency: Currency,
    pub balance: Money,
    pub min: Money,
    pub max: Money,
    pub presets: Vec<Money>,
    pub methods: Vec<TopUpMethod>,
}

impl Checkout<'_> {
    pub async fn topup(
        &self,
        client_id: ClientId,
        amount: Money,
        provider: &str,
        method: Option<&str>,
        channel: Channel,
    ) -> ServiceResult<CheckoutResult> {
        self.topup_from(client_id, amount, provider, method, channel, None)
            .await
    }

    pub async fn topup_options(
        &self,
        client_id: ClientId,
        channel: Channel,
    ) -> ServiceResult<TopUpOptions> {
        self.topup_open()?;
        let settings = self.services.settings();
        let currency = settings.currency;
        let (balance, telegram) = {
            let mut conn = self.services.store().conn().await?;
            if conn.clients().get(client_id).await?.is_blocked() {
                return Err(ServiceError::Forbidden);
            }
            let balance = conn
                .ledger()
                .balance(client_id, Wallet::Main, currency)
                .await?;
            (balance, telegram_linked(&mut conn, client_id).await?)
        };
        let limits = settings.topup_limits();
        let registry = self.services.payments();
        let found = sn_payments::plain_methods(registry.checkout_methods(&MethodsContext {
            currency,
            total: limits.min,
            stars_total: None,
            balance: None,
            channel: channel.client(),
        }));
        let methods = reachable(registry, found, telegram)
            .into_iter()
            .filter(|method| method.kind == ProviderKind::External && method.unavailable.is_none())
            .filter_map(|method| {
                let amounts = registry.get(method.provider).and_then(|provider| {
                    provider.fixed_amounts(&registry.config(provider.id()), currency)
                });
                let amounts = match amounts {
                    None => None,
                    Some(list) => {
                        let fits: Vec<Money> = list
                            .into_iter()
                            .filter(|amount| {
                                amount.minor >= limits.min.minor && amount.minor <= limits.max.minor
                            })
                            .collect();
                        if fits.is_empty() {
                            return None;
                        }
                        Some(fits)
                    }
                };
                Some(TopUpMethod {
                    bonus: registry
                        .topup_bonus(method.provider, method.method.as_deref(), currency)
                        .map(|terms| MethodBonus::of(terms, currency)),
                    provider: method.provider,
                    needs_email: method.needs_email,
                    method: method.method,
                    title: method.title,
                    hint: method.hint,
                    amounts,
                })
            })
            .collect();
        Ok(TopUpOptions {
            currency,
            balance,
            min: limits.min,
            max: limits.max,
            presets: topup_presets(limits),
            methods,
        })
    }
}
