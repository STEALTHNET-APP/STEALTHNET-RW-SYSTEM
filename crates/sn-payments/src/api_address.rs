use crate::error::{PaymentError, PaymentResult};
use crate::provider::ProviderConfig;
use sn_domain::access::Permission;
use sn_domain::settings::SettingField;

pub const API_URL: &str = "api_url";

pub const ORIGIN_RULE: &str =
    "нужен адрес сервера вида https://api.example.com, без пути, логина и пароля";

pub fn api_origin(raw: &str) -> Result<String, &'static str> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| ORIGIN_RULE)?;
    let bare = url.scheme() == "https"
        && url.host_str().is_some_and(|host| !host.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none();
    if bare {
        Ok(url.origin().ascii_serialization())
    } else {
        Err(ORIGIN_RULE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiAddress {
    pub standard: &'static str,
}

impl ApiAddress {
    pub const fn new(standard: &'static str) -> Self {
        ApiAddress { standard }
    }

    pub const fn field(&self, hint: &'static str) -> SettingField {
        SettingField::url(API_URL, "Адрес API", hint)
            .optional()
            .with_default(self.standard)
            .origin()
            .need(Permission::Money)
    }

    pub fn own(&self, config: &ProviderConfig) -> Option<String> {
        let origin = api_origin(config.get(API_URL)?).ok()?;
        (origin != self.standard).then_some(origin)
    }

    pub fn resolve(&self, config: &ProviderConfig) -> String {
        self.own(config)
            .unwrap_or_else(|| self.standard.to_string())
    }

    pub fn validate(&self, config: &ProviderConfig, brand: &str) -> PaymentResult<()> {
        match config.get(API_URL).map(api_origin) {
            Some(Err(rule)) => Err(PaymentError::NotConfigured(format!(
                "адрес API {brand}: {rule}"
            ))),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::config;
    use serde_json::json;

    const HELEKET: ApiAddress = ApiAddress::new("https://api.heleket.hk");

    #[test]
    fn адрес_api_только_https_и_имя_сервера() {
        for (raw, expected) in [
            ("https://api.heleket.hk", "https://api.heleket.hk"),
            ("  https://api.heleket.hk/  ", "https://api.heleket.hk"),
            ("https://API.Heleket.example", "https://api.heleket.example"),
            (
                "https://relay.example.com:8443",
                "https://relay.example.com:8443",
            ),
            (
                "https://relay.example.com:443/",
                "https://relay.example.com",
            ),
            ("https://127.0.0.1:9443", "https://127.0.0.1:9443"),
        ] {
            assert_eq!(api_origin(raw).as_deref(), Ok(expected), "{raw}");
        }
        for raw in [
            "",
            "api.heleket.hk",
            "http://api.heleket.hk",
            "ftp://api.heleket.hk",
            "https://api.heleket.hk/v1",
            "https://api.heleket.hk/v1/payment",
            "https://user@api.heleket.hk",
            "https://user:secret@api.heleket.hk",
            "https://:secret@api.heleket.hk",
            "https://api.heleket.hk?merchant=1",
            "https://api.heleket.hk/?",
            "https://api.heleket.hk#top",
            "https://",
        ] {
            assert_eq!(api_origin(raw), Err(ORIGIN_RULE), "{raw}");
        }
    }

    #[test]
    fn пустое_поле_и_стандартный_адрес_не_считаются_своими() {
        assert_eq!(HELEKET.own(&config(&[])), None);
        assert_eq!(HELEKET.resolve(&config(&[])), "https://api.heleket.hk");
        for same in [
            "",
            "  ",
            "https://api.heleket.hk",
            "https://api.heleket.hk/",
        ] {
            let settings = config(&[(API_URL, json!(same))]);
            assert_eq!(HELEKET.own(&settings), None, "{same}");
            assert_eq!(HELEKET.resolve(&settings), "https://api.heleket.hk");
            assert_eq!(HELEKET.validate(&settings, "Heleket"), Ok(()));
        }
        let custom = config(&[(API_URL, json!("https://relay.example.com/"))]);
        assert_eq!(
            HELEKET.own(&custom).as_deref(),
            Some("https://relay.example.com")
        );
        assert_eq!(HELEKET.resolve(&custom), "https://relay.example.com");
        for wrong in [
            "http://relay.example.com",
            "https://relay.example.com/api",
            "https://u:p@relay.example.com",
        ] {
            let settings = config(&[(API_URL, json!(wrong))]);
            assert_eq!(HELEKET.own(&settings), None, "{wrong}");
            assert_eq!(HELEKET.resolve(&settings), "https://api.heleket.hk");
            assert!(matches!(
                HELEKET.validate(&settings, "Heleket"),
                Err(PaymentError::NotConfigured(text)) if text.contains("Heleket") && text.contains("без пути")
            ));
        }
    }

    #[test]
    fn поле_адреса_требует_права_деньги() {
        let field = HELEKET.field("подсказка");
        assert_eq!(field.key, API_URL);
        assert_eq!(field.need, Some(Permission::Money));
        assert!(field.origin && !field.required);
        assert_eq!(field.default, Some("https://api.heleket.hk"));
        let shown = serde_json::to_value(field).unwrap();
        assert_eq!(shown["need"], "money");
        assert_eq!(shown["origin"], true);
        let plain = serde_json::to_value(SettingField::url("x", "X", "")).unwrap();
        assert!(plain.get("need").is_none() && plain.get("origin").is_none());
    }
}
