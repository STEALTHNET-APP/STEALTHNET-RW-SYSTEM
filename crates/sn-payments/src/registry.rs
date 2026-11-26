use crate::provider::{
    ClientLabel, ConnectionCheck, PayMethod, PaymentProvider, ProviderConfig, ProviderIssue,
    ProviderKind,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::access::ClientChannel;
use sn_domain::money::{Amount, Currency, Money, Stars};
use sn_domain::payment::{
    ButtonLabel, MethodChoice, PaymentChannels, TopupBonusView, TopupBonuses,
};
use sn_domain::pricing::{BalanceCheck, BonusTerms, balance_check};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderState {
    pub enabled: bool,
    pub config: ProviderConfig,
    pub channels: PaymentChannels,
    pub methods: Vec<MethodChoice>,
    pub label: ButtonLabel,
    pub topup_bonus: TopupBonuses,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegistrySnapshot {
    pub currency: Currency,
    pub states: HashMap<String, ProviderState>,
    pub order: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodsContext {
    pub currency: Currency,
    pub total: Money,
    pub stars_total: Option<Stars>,
    pub balance: Option<Money>,
    pub channel: Option<ClientChannel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Unavailable {
    InsufficientBalance { balance: Money, missing: Money },
    NoStarsPrice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckoutMethod {
    pub provider: &'static str,
    pub method: Option<String>,
    pub kind: ProviderKind,
    pub title: String,
    pub hint: String,
    pub amount: Option<Amount>,
    pub unavailable: Option<Unavailable>,
}

pub fn plain_methods(methods: Vec<CheckoutMethod>) -> Vec<CheckoutMethod> {
    methods
        .into_iter()
        .filter(|m| m.method.as_deref() != Some(sn_domain::payment::RECURRING_METHOD))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MethodView {
    pub code: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
    pub min_minor: Option<i64>,
    pub enabled: bool,
    pub custom_title: Option<String>,
}

fn chosen_methods(
    provider: &dyn PaymentProvider,
    choices: &[MethodChoice],
    currency: Currency,
) -> Vec<(PayMethod, Option<String>)> {
    let available = provider.methods(currency);
    choices
        .iter()
        .filter_map(|choice| {
            available
                .iter()
                .find(|m| m.code == choice.code)
                .map(|m| (*m, choice.title.clone()))
        })
        .collect()
}

fn method_views(
    provider: &dyn PaymentProvider,
    choices: &[MethodChoice],
    currency: Currency,
) -> Vec<MethodView> {
    let view = |m: &PayMethod, enabled: bool, custom_title: Option<String>| MethodView {
        code: m.code,
        title: m.title,
        hint: m.hint,
        min_minor: m.min_minor,
        enabled,
        custom_title,
    };
    let mut views: Vec<MethodView> = chosen_methods(provider, choices, currency)
        .iter()
        .map(|(m, title)| view(m, true, title.clone()))
        .collect();
    for m in provider.methods(currency) {
        if !views.iter().any(|v| v.code == m.code) {
            views.push(view(&m, false, None));
        }
    }
    views
}

pub fn charge(
    provider: &dyn PaymentProvider,
    config: &ProviderConfig,
    ctx: &MethodsContext,
) -> Option<Money> {
    match provider.fixed_amounts(config, ctx.currency) {
        None => Some(ctx.total),
        Some(amounts) => amounts
            .into_iter()
            .filter(|amount| {
                amount.currency == ctx.total.currency && amount.minor >= ctx.total.minor
            })
            .min_by_key(|amount| amount.minor),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRecord {
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub check: ConnectionCheck,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderView {
    pub id: &'static str,
    pub title: &'static str,
    pub kind: ProviderKind,
    pub currencies: &'static [Currency],
    pub enabled: bool,
    pub configured: bool,
    pub accepts_http_webhooks: bool,
    pub supports_recurring: bool,
    pub channels: PaymentChannels,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub issue: Option<ProviderIssue>,
    pub methods: Vec<MethodView>,
    pub label: ClientLabel,
    pub custom_label: ButtonLabel,
    pub topup_bonus: BTreeMap<Currency, TopupBonusView>,
    pub check: Option<CheckRecord>,
    pub own_api_url: Option<String>,
}

pub struct ProviderRegistry {
    providers: Vec<Arc<dyn PaymentProvider>>,
    snapshot: RwLock<Arc<RegistrySnapshot>>,
    checks: RwLock<HashMap<String, CheckRecord>>,
    telegram_api: String,
}

impl ProviderRegistry {
    pub fn new(providers: Vec<Arc<dyn PaymentProvider>>) -> Self {
        let states = providers
            .iter()
            .map(|p| {
                (
                    p.id().to_string(),
                    ProviderState {
                        enabled: p.enabled_by_default(),
                        config: ProviderConfig::default(),
                        channels: PaymentChannels::ALL,
                        methods: Vec::new(),
                        label: ButtonLabel::default(),
                        topup_bonus: TopupBonuses::default(),
                    },
                )
            })
            .collect();
        ProviderRegistry {
            providers,
            snapshot: RwLock::new(Arc::new(RegistrySnapshot {
                currency: Currency::Usd,
                states,
                order: Vec::new(),
            })),
            checks: RwLock::new(HashMap::new()),
            telegram_api: crate::providers::TELEGRAM_API.to_string(),
        }
    }

    pub fn builtin() -> Self {
        Self::new(crate::providers::builtin())
    }

    pub fn with_telegram_api(telegram_api: &str) -> Self {
        let mut registry = Self::new(crate::providers::with_telegram_api(telegram_api));
        registry.telegram_api = telegram_api.trim_end_matches('/').to_string();
        registry
    }

    pub fn telegram_api(&self) -> &str {
        &self.telegram_api
    }

    pub fn providers(&self) -> &[Arc<dyn PaymentProvider>] {
        &self.providers
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn PaymentProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }

    pub fn defaults(&self) -> Vec<(&'static str, bool)> {
        self.providers
            .iter()
            .map(|p| (p.id(), p.enabled_by_default()))
            .collect()
    }

    pub fn snapshot(&self) -> Arc<RegistrySnapshot> {
        match self.snapshot.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn apply(&self, snapshot: RegistrySnapshot) {
        let snapshot = Arc::new(snapshot);
        match self.snapshot.write() {
            Ok(mut guard) => *guard = snapshot,
            Err(poisoned) => *poisoned.into_inner() = snapshot,
        }
    }

    pub fn check(&self, id: &str) -> Option<CheckRecord> {
        match self.checks.read() {
            Ok(guard) => guard.get(id).cloned(),
            Err(poisoned) => poisoned.into_inner().get(id).cloned(),
        }
    }

    pub fn set_checks(&self, checks: HashMap<String, CheckRecord>) {
        match self.checks.write() {
            Ok(mut guard) => *guard = checks,
            Err(poisoned) => *poisoned.into_inner() = checks,
        }
    }

    pub fn record_check(&self, id: &str, record: Option<CheckRecord>) {
        let mut guard = match self.checks.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match record {
            Some(record) => {
                guard.insert(id.to_string(), record);
            }
            None => {
                guard.remove(id);
            }
        }
    }

    pub fn config(&self, id: &str) -> ProviderConfig {
        self.snapshot()
            .states
            .get(id)
            .map(|s| s.config.clone())
            .unwrap_or_default()
    }

    pub fn is_available(&self, id: &str, currency: Currency) -> bool {
        let Some(provider) = self.get(id) else {
            return false;
        };
        let snapshot = self.snapshot();
        let Some(state) = snapshot.states.get(id) else {
            return false;
        };
        state.enabled
            && provider.currencies().contains(&currency)
            && (provider.kind() == ProviderKind::Balance || provider.is_configured(&state.config))
    }

    pub fn balance_enabled(&self) -> bool {
        let snapshot = self.snapshot();
        self.providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Balance)
            .any(|p| snapshot.states.get(p.id()).is_some_and(|s| s.enabled))
    }

    pub fn topup_bonus(
        &self,
        id: &str,
        method: Option<&str>,
        currency: Currency,
    ) -> Option<BonusTerms> {
        self.get(id)
            .filter(|p| p.kind() == ProviderKind::External)?;
        self.snapshot()
            .states
            .get(id)?
            .topup_bonus
            .terms(currency, method)
    }

    pub fn allowed_in(&self, id: &str, channel: ClientChannel) -> bool {
        self.snapshot()
            .states
            .get(id)
            .is_some_and(|state| state.channels.allows(channel))
    }

    pub fn accepts_webhooks(&self, id: &str) -> bool {
        let Some(provider) = self.get(id) else {
            return false;
        };
        if provider.kind() != ProviderKind::External || !provider.accepts_http_webhooks() {
            return false;
        }
        self.snapshot()
            .states
            .get(id)
            .is_some_and(|state| provider.is_configured(&state.config))
    }

    pub fn views(&self) -> Vec<ProviderView> {
        self.views_in(self.snapshot().currency)
    }

    pub fn views_in(&self, currency: Currency) -> Vec<ProviderView> {
        let snapshot = self.snapshot();
        self.providers
            .iter()
            .map(|p| {
                let state = snapshot.states.get(p.id()).cloned().unwrap_or_default();
                ProviderView {
                    id: p.id(),
                    title: p.title(),
                    kind: p.kind(),
                    currencies: p.currencies(),
                    enabled: state.enabled,
                    configured: p.is_configured(&state.config),
                    accepts_http_webhooks: p.accepts_http_webhooks(),
                    supports_recurring: p.supports_recurring(),
                    channels: state.channels,
                    settings: state.config.public_view(&p.settings_schema()),
                    issue: p.issue(),
                    methods: method_views(p.as_ref(), &state.methods, currency),
                    label: p.client_label(currency, &state.config),
                    custom_label: state.label.clone(),
                    topup_bonus: p
                        .currencies()
                        .iter()
                        .map(|c| (*c, state.topup_bonus.get(*c).view(*c)))
                        .collect(),
                    check: self.check(p.id()),
                    own_api_url: p.own_api_url(&state.config),
                }
            })
            .collect()
    }

    pub fn checkout_methods(&self, ctx: &MethodsContext) -> Vec<CheckoutMethod> {
        let snapshot = self.snapshot();
        let mut ordered: Vec<&'static str> = Vec::new();
        for id in &snapshot.order {
            if let Some(provider) = self.get(id)
                && !ordered.contains(&provider.id())
            {
                ordered.push(provider.id());
            }
        }
        for provider in &self.providers {
            if !ordered.contains(&provider.id()) {
                ordered.push(provider.id());
            }
        }
        let mut methods = Vec::new();
        let mut balance_method = None;
        for id in ordered {
            let Some(provider) = self.get(id) else {
                continue;
            };
            let open = self.is_available(id, ctx.currency);
            let leftover = !open
                && provider.kind() == ProviderKind::Balance
                && provider.currencies().contains(&ctx.currency);
            if (!open && !leftover)
                || ctx
                    .channel
                    .is_some_and(|channel| !self.allowed_in(id, channel))
            {
                continue;
            }
            let empty = ProviderConfig::default();
            let state = snapshot.states.get(id);
            let label =
                provider.client_label(ctx.currency, state.map_or(&empty, |state| &state.config));
            let custom = state.map(|state| &state.label);
            let title = custom
                .and_then(|l| l.title.clone())
                .unwrap_or_else(|| crate::english::shown(label.title));
            let hint = custom
                .and_then(|l| l.hint.clone())
                .unwrap_or_else(|| crate::english::shown(label.hint));
            let (amount, unavailable) = match provider.kind() {
                ProviderKind::External => {
                    let choices = snapshot
                        .states
                        .get(id)
                        .map(|state| state.methods.as_slice())
                        .unwrap_or_default();
                    let chosen = chosen_methods(provider.as_ref(), choices, ctx.currency);
                    if chosen.is_empty() {
                        let config = state.map_or(&empty, |state| &state.config);
                        match charge(provider.as_ref(), config, ctx) {
                            Some(amount) => (Some(amount.into()), None),
                            None => continue,
                        }
                    } else {
                        for (method, title) in chosen {
                            if method.min_minor.is_some_and(|min| ctx.total.minor < min) {
                                continue;
                            }
                            methods.push(CheckoutMethod {
                                provider: provider.id(),
                                method: Some(method.code.to_string()),
                                kind: ProviderKind::External,
                                title: title.unwrap_or_else(|| crate::english::shown(method.title)),
                                hint: crate::english::shown(method.hint),
                                amount: Some(ctx.total.into()),
                                unavailable: None,
                            });
                        }
                        continue;
                    }
                }
                ProviderKind::Stars => match ctx.stars_total {
                    Some(stars) => (Some(stars.into()), None),
                    None => (None, Some(Unavailable::NoStarsPrice)),
                },
                ProviderKind::Balance => {
                    let Some(balance) = ctx.balance else {
                        continue;
                    };
                    let unavailable = match balance_check(balance, ctx.total) {
                        Ok(BalanceCheck::Enough) => None,
                        Ok(BalanceCheck::Short { .. }) if leftover => continue,
                        Ok(BalanceCheck::Short { balance, missing }) => {
                            Some(Unavailable::InsufficientBalance { balance, missing })
                        }
                        Err(_) => continue,
                    };
                    balance_method = Some(CheckoutMethod {
                        provider: provider.id(),
                        method: None,
                        kind: ProviderKind::Balance,
                        title,
                        hint,
                        amount: Some(ctx.total.into()),
                        unavailable,
                    });
                    continue;
                }
            };
            methods.push(CheckoutMethod {
                provider: provider.id(),
                method: None,
                kind: provider.kind(),
                title,
                hint,
                amount,
                unavailable,
            });
        }
        methods.extend(balance_method);
        methods
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{PaymentError, PaymentResult};
    use crate::provider::*;
    use async_trait::async_trait;
    use serde_json::json;
    use sn_domain::payment::providers;
    use sn_domain::settings::PaymentOrder;

    struct Fake {
        id: &'static str,
        kind: ProviderKind,
        currencies: &'static [Currency],
        enabled: bool,
    }

    #[async_trait]
    impl PaymentProvider for Fake {
        fn id(&self) -> &'static str {
            self.id
        }

        fn title(&self) -> &'static str {
            self.id
        }

        fn kind(&self) -> ProviderKind {
            self.kind
        }

        fn currencies(&self) -> &'static [Currency] {
            self.currencies
        }

        fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
            ClientLabel {
                title: self.id,
                hint: "",
            }
        }

        fn methods(&self, currency: Currency) -> Vec<PayMethod> {
            if self.id == providers::PLATEGA && currency == Currency::Rub {
                vec![
                    PayMethod::new("sbp", "СБП", "Через приложение банка"),
                    PayMethod::new("card", "Карта", "От 1000 рублей").min_minor(100_000),
                ]
            } else {
                Vec::new()
            }
        }

        fn enabled_by_default(&self) -> bool {
            self.enabled
        }

        async fn create_invoice(
            &self,
            _config: &ProviderConfig,
            _request: &InvoiceRequest,
        ) -> PaymentResult<Invoice> {
            Err(PaymentError::Unsupported("тест"))
        }

        async fn parse_webhook(
            &self,
            _config: &ProviderConfig,
            _request: &WebhookRequest,
        ) -> PaymentResult<WebhookOutcome> {
            Err(PaymentError::Unsupported("тест"))
        }
    }

    fn fake(
        id: &'static str,
        kind: ProviderKind,
        currencies: &'static [Currency],
        enabled: bool,
    ) -> Arc<dyn PaymentProvider> {
        Arc::new(Fake {
            id,
            kind,
            currencies,
            enabled,
        })
    }

    fn registry(currency: Currency) -> ProviderRegistry {
        let registry = ProviderRegistry::new(vec![
            fake(
                providers::BALANCE,
                ProviderKind::Balance,
                &Currency::ALL,
                true,
            ),
            fake(
                providers::CRYPTOPAY,
                ProviderKind::External,
                &[Currency::Usd],
                true,
            ),
            fake(
                providers::HELEKET,
                ProviderKind::External,
                &[Currency::Usd],
                false,
            ),
            fake(
                providers::LAVA,
                ProviderKind::External,
                &[Currency::Usd],
                true,
            ),
            fake(
                providers::PLATEGA,
                ProviderKind::External,
                &[Currency::Rub],
                true,
            ),
            fake(
                providers::YOOKASSA,
                ProviderKind::External,
                &[Currency::Rub],
                true,
            ),
            fake(
                providers::YOOMONEY,
                ProviderKind::External,
                &[Currency::Rub],
                true,
            ),
            fake(
                providers::OVERPAY,
                ProviderKind::External,
                &[Currency::Rub],
                false,
            ),
            fake(providers::STARS, ProviderKind::Stars, &Currency::ALL, true),
        ]);
        let states = registry
            .defaults()
            .into_iter()
            .map(|(id, enabled)| {
                (
                    id.to_string(),
                    ProviderState {
                        enabled,
                        config: ProviderConfig::default(),
                        channels: sn_domain::payment::PaymentChannels::ALL,
                        methods: Vec::new(),
                        label: ButtonLabel::default(),
                        topup_bonus: TopupBonuses::default(),
                    },
                )
            })
            .collect();
        registry.apply(RegistrySnapshot {
            currency,
            states,
            order: PaymentOrder::default().for_currency(currency).to_vec(),
        });
        registry
    }

    #[test]
    fn способы_платёжки_становятся_кнопками_а_чужие_коды_возвращают_одну_кнопку() {
        let registry = registry(Currency::Rub);
        let mut snapshot = (*registry.snapshot()).clone();
        let choose = |snapshot: &mut RegistrySnapshot, methods: Vec<MethodChoice>| {
            if let Some(state) = snapshot.states.get_mut(providers::PLATEGA) {
                state.methods = methods;
            }
        };
        choose(
            &mut snapshot,
            vec![
                MethodChoice {
                    code: "card".into(),
                    title: Some("Карта МИР".into()),
                },
                MethodChoice {
                    code: "sbp".into(),
                    title: None,
                },
            ],
        );
        registry.apply(snapshot.clone());
        let platega = |minor: i64| -> Vec<(Option<String>, String)> {
            registry
                .checkout_methods(&MethodsContext {
                    currency: Currency::Rub,
                    total: Money::new(minor, Currency::Rub),
                    stars_total: None,
                    balance: None,
                    channel: None,
                })
                .into_iter()
                .filter(|m| m.provider == providers::PLATEGA)
                .map(|m| (m.method, m.title))
                .collect()
        };
        assert_eq!(
            platega(150_000),
            vec![
                (Some("card".to_string()), "Карта МИР".to_string()),
                (Some("sbp".to_string()), "СБП".to_string())
            ]
        );
        assert_eq!(
            platega(50_000),
            vec![(Some("sbp".to_string()), "СБП".to_string())]
        );
        let view = registry
            .views()
            .into_iter()
            .find(|v| v.id == providers::PLATEGA)
            .unwrap();
        let listed: Vec<(&str, bool, Option<&str>)> = view
            .methods
            .iter()
            .map(|m| (m.code, m.enabled, m.custom_title.as_deref()))
            .collect();
        assert_eq!(
            listed,
            vec![("card", true, Some("Карта МИР")), ("sbp", true, None)]
        );
        choose(
            &mut snapshot,
            vec![MethodChoice {
                code: "paypal".into(),
                title: None,
            }],
        );
        registry.apply(snapshot);
        assert_eq!(platega(150_000), vec![(None, "platega".to_string())]);
    }

    #[test]
    fn умолчания_встроенных_систем_как_в_настройках() {
        let builtin = ProviderRegistry::builtin();
        let on: Vec<&str> = builtin
            .defaults()
            .into_iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            on,
            [
                providers::CRYPTOPAY,
                providers::LAVA,
                providers::YOOKASSA,
                providers::PLATEGA,
                providers::YOOMONEY,
                providers::STARS,
                providers::BALANCE
            ]
        );
    }

    #[test]
    fn способы_оплаты_в_долларах_как_в_макете() {
        let registry = registry(Currency::Usd);
        let methods = registry.checkout_methods(&MethodsContext {
            currency: Currency::Usd,
            total: Money::new(699, Currency::Usd),
            stars_total: Some(Stars(350)),
            balance: Some(Money::new(420, Currency::Usd)),
            channel: None,
        });
        let ids: Vec<&str> = methods.iter().map(|m| m.provider).collect();
        assert_eq!(
            ids,
            [
                providers::CRYPTOPAY,
                providers::LAVA,
                providers::STARS,
                providers::BALANCE
            ]
        );
        assert_eq!(methods[2].amount, Some(Stars(350).into()));
        assert_eq!(
            methods[3].unavailable,
            Some(Unavailable::InsufficientBalance {
                balance: Money::new(420, Currency::Usd),
                missing: Money::new(279, Currency::Usd)
            })
        );
    }

    #[test]
    fn способ_выключенный_в_канале_не_показывается_там() {
        let registry = registry(Currency::Usd);
        let mut snapshot = (*registry.snapshot()).clone();
        if let Some(state) = snapshot.states.get_mut(providers::CRYPTOPAY) {
            state.channels = sn_domain::payment::PaymentChannels {
                cabinet: true,
                bot: false,
                miniapp: false,
            };
        }
        if let Some(state) = snapshot.states.get_mut(providers::STARS) {
            state.channels.cabinet = false;
        }
        registry.apply(snapshot);
        let ids = |channel: Option<ClientChannel>| -> Vec<&'static str> {
            registry
                .checkout_methods(&MethodsContext {
                    currency: Currency::Usd,
                    total: Money::new(699, Currency::Usd),
                    stars_total: Some(Stars(350)),
                    balance: Some(Money::new(9_000, Currency::Usd)),
                    channel,
                })
                .into_iter()
                .map(|m| m.provider)
                .collect()
        };
        assert_eq!(
            ids(Some(ClientChannel::Bot)),
            [providers::LAVA, providers::STARS, providers::BALANCE]
        );
        assert_eq!(
            ids(Some(ClientChannel::Cabinet)),
            [providers::CRYPTOPAY, providers::LAVA, providers::BALANCE]
        );
        assert_eq!(
            ids(None),
            [
                providers::CRYPTOPAY,
                providers::LAVA,
                providers::STARS,
                providers::BALANCE
            ]
        );
        assert!(registry.allowed_in(providers::CRYPTOPAY, ClientChannel::Cabinet));
        assert!(!registry.allowed_in(providers::CRYPTOPAY, ClientChannel::Miniapp));
        assert!(!registry.allowed_in("нет-такого", ClientChannel::Bot));
        let view = registry
            .views()
            .into_iter()
            .find(|v| v.id == providers::CRYPTOPAY)
            .unwrap();
        assert!(!view.channels.bot);
    }

    #[test]
    fn выключенный_баланс_тратит_только_остаток_которого_хватает() {
        let registry = registry(Currency::Usd);
        assert!(registry.balance_enabled());
        let mut snapshot = (*registry.snapshot()).clone();
        if let Some(state) = snapshot.states.get_mut(providers::BALANCE) {
            state.enabled = false;
        }
        registry.apply(snapshot.clone());
        assert!(!registry.balance_enabled());
        assert!(!registry.is_available(providers::BALANCE, Currency::Usd));
        let methods = |total: i64, balance: Option<i64>, channel: Option<ClientChannel>| {
            registry.checkout_methods(&MethodsContext {
                currency: Currency::Usd,
                total: Money::new(total, Currency::Usd),
                stars_total: None,
                balance: balance.map(|minor| Money::new(minor, Currency::Usd)),
                channel,
            })
        };
        let short = methods(699, Some(420), None);
        assert!(short.iter().all(|m| m.provider != providers::BALANCE));
        let enough = methods(699, Some(700), None);
        let leftover = enough
            .iter()
            .find(|m| m.provider == providers::BALANCE)
            .unwrap();
        assert_eq!(leftover.unavailable, None);
        assert_eq!(leftover.amount, Some(Money::new(699, Currency::Usd).into()));
        assert!(
            methods(699, None, None)
                .iter()
                .all(|m| m.provider != providers::BALANCE)
        );
        assert!(
            methods(0, Some(0), None)
                .iter()
                .any(|m| m.provider == providers::BALANCE)
        );
        if let Some(state) = snapshot.states.get_mut(providers::BALANCE) {
            state.channels.bot = false;
        }
        registry.apply(snapshot);
        assert!(
            methods(699, Some(700), Some(ClientChannel::Bot))
                .iter()
                .all(|m| m.provider != providers::BALANCE)
        );
        assert!(
            methods(699, Some(700), Some(ClientChannel::Cabinet))
                .iter()
                .any(|m| m.provider == providers::BALANCE)
        );
    }

    #[test]
    fn способы_оплаты_в_рублях_и_без_цены_в_звёздах() {
        let registry = registry(Currency::Rub);
        let methods = registry.checkout_methods(&MethodsContext {
            currency: Currency::Rub,
            total: Money::new(49_900, Currency::Rub),
            stars_total: None,
            balance: None,
            channel: None,
        });
        let ids: Vec<&str> = methods.iter().map(|m| m.provider).collect();
        assert_eq!(
            ids,
            [
                providers::PLATEGA,
                providers::YOOKASSA,
                providers::YOOMONEY,
                providers::STARS
            ]
        );
        assert_eq!(methods[3].unavailable, Some(Unavailable::NoStarsPrice));
    }

    #[test]
    fn уведомления_принимаются_от_настроенной_системы_даже_выключенной() {
        let registry = ProviderRegistry::builtin();
        let configured = |id: &str| match id {
            providers::PLATEGA => crate::test_support::config(&[
                ("merchant_id", json!("29ef6fa6-0d2b-466c-9604-0363a30436cc")),
                ("secret", json!("s")),
            ]),
            providers::OVERPAY => crate::test_support::config(&[
                ("api_url", json!("https://api.overpay.io")),
                ("project_id", json!("1")),
                ("login", json!("l")),
                ("password", json!("p")),
            ]),
            providers::STARS => crate::test_support::config(&[("bot_token", json!("1:abc"))]),
            _ => ProviderConfig::default(),
        };
        let apply = |enabled: &[&str]| {
            registry.apply(RegistrySnapshot {
                currency: Currency::Rub,
                states: registry
                    .providers()
                    .iter()
                    .map(|p| {
                        (
                            p.id().to_string(),
                            ProviderState {
                                enabled: enabled.contains(&p.id()),
                                config: configured(p.id()),
                                channels: sn_domain::payment::PaymentChannels::ALL,
                                methods: Vec::new(),
                                label: ButtonLabel::default(),
                                topup_bonus: TopupBonuses::default(),
                            },
                        )
                    })
                    .collect(),
                order: Vec::new(),
            })
        };
        apply(&[
            providers::PLATEGA,
            providers::YOOKASSA,
            providers::OVERPAY,
            providers::STARS,
            providers::BALANCE,
        ]);
        assert!(registry.accepts_webhooks(providers::PLATEGA));
        assert!(!registry.accepts_webhooks(providers::YOOKASSA));
        assert!(!registry.accepts_webhooks(providers::OVERPAY));
        assert!(!registry.accepts_webhooks(providers::STARS));
        assert!(!registry.accepts_webhooks(providers::BALANCE));
        assert!(!registry.accepts_webhooks("nope"));
        apply(&[providers::YOOKASSA]);
        assert!(registry.accepts_webhooks(providers::PLATEGA));
        assert!(!registry.is_available(providers::PLATEGA, Currency::Rub));
        assert!(!registry.accepts_webhooks(providers::YOOKASSA));
        assert!(!registry.accepts_webhooks(providers::OVERPAY));
        assert!(!registry.accepts_webhooks(providers::STARS));
    }

    #[test]
    fn ненастроенная_система_не_предлагается() {
        let registry = ProviderRegistry::builtin();
        assert!(!registry.is_available(providers::CRYPTOPAY, Currency::Usd));
        let view = registry
            .views()
            .into_iter()
            .find(|v| v.id == providers::CRYPTOPAY)
            .unwrap();
        assert!(view.enabled);
        assert!(!view.configured);
        assert_eq!(view.settings["api_token"], json!({"filled": false}));
    }

    #[test]
    fn бонус_к_пополнению_только_у_внешних_платёжек() {
        let registry = ProviderRegistry::builtin();
        let mut snapshot = (*registry.snapshot()).clone();
        for state in snapshot.states.values_mut() {
            for currency in Currency::ALL {
                state.topup_bonus.set(
                    currency,
                    sn_domain::payment::TopupBonus {
                        enabled: true,
                        percent: 5,
                        ..Default::default()
                    },
                );
            }
        }
        registry.apply(snapshot);
        let percent = |id: &str| {
            registry
                .topup_bonus(id, None, Currency::Rub)
                .map(|terms| terms.percent)
        };
        for provider in registry.providers() {
            let expected = (provider.kind() == ProviderKind::External).then_some(5);
            assert_eq!(percent(provider.id()), expected);
        }
        assert_eq!(percent("stars"), None);
        assert_eq!(percent("balance"), None);
        assert_eq!(percent("cryptopay"), Some(5));
        assert_eq!(percent("unknown"), None);
    }
}
