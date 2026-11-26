use super::merchant;
use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use chrono::DateTime;
use sn_domain::money::Currency;
use sn_domain::payment::{NoticeKind, providers};
use sn_domain::settings::SettingField;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub use merchant::signature;

const API: &str = "https://api.cryptomus.com";
const BRAND: &str = "Cryptomus";
const CATALOG_FRESH: Duration = Duration::from_secs(30 * 60);
const CATALOG_RETRY: Duration = Duration::from_secs(60);

const PAGE: PayMethod = PayMethod::new(
    "page",
    "Любая монета",
    "Монету и сеть выбирают на странице Cryptomus",
);

const TON: &[&str] = &["GRAM", "TON"];

struct Coin {
    method: PayMethod,
    currencies: &'static [&'static str],
    networks: &'static [&'static str],
}

impl Coin {
    fn fixed(&self) -> bool {
        self.currencies.len() == 1 && self.networks.len() == 1
    }

    fn known(&self) -> (String, String) {
        (self.currencies[0].to_string(), self.networks[0].to_string())
    }

    fn matches(&self, currency: &str, network: &str) -> bool {
        self.currencies
            .iter()
            .any(|c| c.eq_ignore_ascii_case(currency))
            && self
                .networks
                .iter()
                .any(|n| n.eq_ignore_ascii_case(network))
    }
}

const COINS: [Coin; 6] = [
    Coin {
        method: PayMethod::new("usdt_trc20", "USDT TRC-20", "USDT в сети Tron"),
        currencies: &["USDT"],
        networks: &["tron"],
    },
    Coin {
        method: PayMethod::new("usdt_ton", "USDT TON", "USDT в сети TON"),
        currencies: &["USDT"],
        networks: TON,
    },
    Coin {
        method: PayMethod::new("usdt_bep20", "USDT BEP-20", "USDT в сети BNB Smart Chain"),
        currencies: &["USDT"],
        networks: &["bsc"],
    },
    Coin {
        method: PayMethod::new("btc", "Bitcoin", "BTC в сети Bitcoin"),
        currencies: &["BTC"],
        networks: &["btc"],
    },
    Coin {
        method: PayMethod::new("ton", "TON", "Gram, бывший Toncoin, в сети TON"),
        currencies: TON,
        networks: TON,
    },
    Coin {
        method: PayMethod::new("ltc", "Litecoin", "LTC в сети Litecoin"),
        currencies: &["LTC"],
        networks: &["ltc"],
    },
];

type Pairs = Vec<(String, String)>;

struct Catalog {
    owner: String,
    at: Instant,
    services: Option<Pairs>,
}

pub struct Cryptomus {
    http: sn_egress::Http,
    api_base: Option<String>,
    catalog: Mutex<Option<Catalog>>,
}

impl Default for Cryptomus {
    fn default() -> Self {
        Self::new()
    }
}

impl Cryptomus {
    pub fn new() -> Self {
        Cryptomus {
            http: http_client(sn_domain::payment::providers::CRYPTOMUS),
            api_base: None,
            catalog: Mutex::new(None),
        }
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Cryptomus {
            http: http_client(sn_domain::payment::providers::CRYPTOMUS),
            api_base: Some(api_base.into()),
            catalog: Mutex::new(None),
        }
    }

    fn base(&self, config: &ProviderConfig) -> String {
        match &self.api_base {
            Some(base) => base.trim_end_matches('/').to_string(),
            None => config
                .get("api_url")
                .filter(|url| util::is_https(url))
                .unwrap_or(API)
                .trim_end_matches('/')
                .to_string(),
        }
    }

    async fn call(&self, config: &ProviderConfig, path: &str, body: &Json) -> PaymentResult<Json> {
        merchant::call(
            &self.http,
            format!("{}{path}", self.base(config)),
            config,
            body,
        )
        .await
    }

    fn cached(&self, owner: &str) -> Option<Option<Pairs>> {
        let guard = match self.catalog.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let catalog = guard.as_ref().filter(|c| c.owner == owner)?;
        let fresh = match catalog.services {
            Some(_) => CATALOG_FRESH,
            None => CATALOG_RETRY,
        };
        (catalog.at.elapsed() < fresh).then(|| catalog.services.clone())
    }

    async fn services(&self, config: &ProviderConfig) -> Option<Pairs> {
        let owner = format!(
            "{}|{}",
            self.base(config),
            config.get("merchant_id").unwrap_or_default()
        );
        if let Some(cached) = self.cached(&owner) {
            return cached;
        }
        let fetched = merchant::services(
            &self.http,
            format!("{}/v1/payment/services", self.base(config)),
            config,
            BRAND,
        )
        .await
        .map(|list| {
            list.iter()
                .filter_map(|service| {
                    let text = |key: &str| service.get(key).and_then(Json::as_str);
                    Some((text("currency")?.to_string(), text("network")?.to_string()))
                })
                .collect::<Pairs>()
        })
        .map_err(|check| {
            tracing::warn!(reason = %check.summary, "список монет Cryptomus не получен, коды монет берутся по умолчанию");
        })
        .ok();
        let mut guard = match self.catalog.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = Some(Catalog {
            owner,
            at: Instant::now(),
            services: fetched.clone(),
        });
        fetched
    }

    async fn pair(&self, config: &ProviderConfig, coin: &Coin) -> (String, String) {
        if coin.fixed() {
            return coin.known();
        }
        self.services(config)
            .await
            .and_then(|list| {
                list.into_iter()
                    .find(|(currency, network)| coin.matches(currency, network))
            })
            .unwrap_or_else(|| coin.known())
    }
}

fn coin(code: &str) -> PaymentResult<Option<&'static Coin>> {
    if code == PAGE.code {
        return Ok(None);
    }
    COINS
        .iter()
        .find(|coin| coin.method.code == code)
        .map(Some)
        .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into()))
}

const REFUND_NOTE: &str = "Cryptomus сообщил о возврате денег по этому платежу, но сумму возврата \
                           в уведомлении не прислал. Сверьте возврат в кабинете Cryptomus и отметьте \
                           его в панели как сделанный вне панели";

fn aml_note(info: &Json) -> String {
    let head = "Cryptomus задержал оплату для проверки AML: деньги заблокированы, пока клиент \
                не пройдёт проверку";
    match info
        .get("aml_verification_url")
        .and_then(Json::as_str)
        .filter(|link| util::is_https(link))
    {
        Some(link) => format!("{head}. Ссылка на проверку для клиента: {link}"),
        None => format!("{head}. Ссылку на проверку возьмите в кабинете Cryptomus"),
    }
}

fn read(info: &Json) -> PaymentResult<WebhookOutcome> {
    let outcome = merchant::outcome(info, REFUND_NOTE)?;
    Ok(if outcome.status == ProviderStatus::Held {
        outcome.with_notice(NoticeKind::AmlCheck, aml_note(info))
    } else {
        outcome
    })
}

#[async_trait]
impl PaymentProvider for Cryptomus {
    fn api_url(&self, config: &ProviderConfig) -> Option<String> {
        Some(self.base(config))
    }

    fn id(&self) -> &'static str {
        providers::CRYPTOMUS
    }

    fn title(&self) -> &'static str {
        BRAND
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Usd, Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Криптовалюта",
            hint: "USDT и другие монеты",
        }
    }

    fn methods(&self, _currency: Currency) -> Vec<PayMethod> {
        std::iter::once(PAGE)
            .chain(COINS.iter().map(|coin| coin.method))
            .collect()
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                "merchant_id",
                "ID мерчанта",
                "Кабинет Cryptomus → «Бизнес» → ваш мерчант → «Настройки», поле Merchant ID",
            ),
            SettingField::secret(
                "api_key",
                "Платёжный API-ключ",
                "Там же, в настройках мерчанта: ключ для приёма платежей (Payment API key) появляется после проверки мерчанта. Ключ выплат не подходит",
            ),
            SettingField::url(
                "api_url",
                "Адрес API",
                "Оставьте пустым, тогда используется https://api.cryptomus.com. Меняйте, только если Cryptomus объявит новый адрес",
            )
            .optional(),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if config
            .get("api_url")
            .is_some_and(|url| !util::is_https(url))
        {
            return Err(PaymentError::NotConfigured(
                "адрес API Cryptomus должен начинаться с https://".into(),
            ));
        }
        match config.get("merchant_id").map(Uuid::parse_str) {
            Some(Ok(_)) => Ok(()),
            _ => Err(PaymentError::NotConfigured(
                "ID мерчанта Cryptomus — это UUID из настроек мерчанта".into(),
            )),
        }
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        let currency = merchant::invoice_currency(request.amount.currency)?;
        let coin = match request.method.as_deref() {
            Some(code) => coin(code)?,
            None => None,
        };
        let mut body = Json::object([
            ("amount", Json::from(request.amount.units())),
            ("currency", Json::from(currency)),
            (
                "order_id",
                Json::from(request.payment_public_id.to_string()),
            ),
            ("url_callback", Json::from(request.webhook_url.clone())),
            ("is_payment_multiple", Json::from(true)),
            (
                "lifetime",
                Json::from(request.expires_in.num_seconds().clamp(300, 43_200)),
            ),
        ]);
        if let Some(url) = util::return_url(request) {
            body.push("url_return", Json::from(url.clone()));
            body.push("url_success", Json::from(url));
        }
        if let Some(coin) = coin {
            let (currency, network) = self.pair(config, coin).await;
            body.push(
                "currencies",
                Json::Array(vec![Json::object([
                    ("currency", Json::from(currency)),
                    ("network", Json::from(network)),
                ])]),
            );
        }
        let result = self.call(config, "/v1/payment", &body).await?;
        let uuid = result
            .get("uuid")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| util::bad("Cryptomus не вернул uuid счёта"))?;
        let pay_url = result
            .get("url")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Cryptomus не вернул ссылку на оплату"))?;
        if let Some(order) = result.get("order_id").and_then(Json::as_str)
            && order != request.payment_public_id.to_string()
        {
            return Err(util::bad("Cryptomus вернул счёт другого заказа"));
        }
        let expires_at = result
            .get("expired_at")
            .and_then(Json::scalar_text)
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|secs| DateTime::from_timestamp(secs, 0));
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(uuid.to_string()),
            expires_at,
            payload: serde_json::json!({ "uuid": uuid }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let key = config.require("api_key")?;
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        if !merchant::verify(key, &request.body) {
            return Err(PaymentError::BadSignature);
        }
        match body.get("type").and_then(Json::as_str) {
            Some("payment") | None => read(&body),
            Some(other) => Ok(merchant::skipped_type(&body, other)),
        }
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let uuid = util::safe_id(external_id)?;
        let result = self
            .call(
                config,
                "/v1/payment/info",
                &Json::object([("uuid", Json::from(uuid))]),
            )
            .await?;
        read(&merchant::requested(result, uuid, BRAND)?).map(Some)
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        _context: &CheckContext,
    ) -> ConnectionCheck {
        let services = match merchant::services(
            &self.http,
            format!("{}/v1/payment/services", self.base(config)),
            config,
            BRAND,
        )
        .await
        {
            Ok(services) => services,
            Err(check) => return check,
        };
        let found = |coin: &Coin| -> Vec<&Json> {
            services
                .iter()
                .filter(|service| merchant::serves(service, coin.currencies, coin.networks))
                .collect()
        };
        let closed: Vec<&str> = COINS
            .iter()
            .filter(|coin| {
                let list = found(coin);
                !list.is_empty() && list.iter().all(|service| merchant::service_closed(service))
            })
            .map(|coin| coin.method.title)
            .collect();
        let missing: Vec<&str> = COINS
            .iter()
            .filter(|coin| found(coin).is_empty())
            .map(|coin| coin.method.title)
            .collect();
        let mut check = ConnectionCheck::ok("ID мерчанта и платёжный ключ приняты Cryptomus");
        if !closed.is_empty() {
            check = check.detail(format!(
                "Сейчас Cryptomus не принимает: {}",
                closed.join(", ")
            ));
        }
        if !missing.is_empty() && !services.is_empty() {
            check = check.detail(format!(
                "В списке монет мерчанта нет: {}. Кнопки этих монет не сработают, пока монета не включена в настройках мерчанта Cryptomus",
                missing.join(", ")
            ));
        }
        check
    }
}

#[cfg(test)]
mod tests;
