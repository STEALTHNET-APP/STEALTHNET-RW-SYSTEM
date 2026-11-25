use serde::{Deserialize, Serialize};

pub const MAX_LEN: usize = 64;
pub const SEPARATOR: &str = "__";

fn valid_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn segments(raw: &str) -> impl Iterator<Item = &str> {
    raw.trim()
        .split(SEPARATOR)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub fn segment<'a>(raw: &'a str, prefix: &str) -> Option<&'a str> {
    segments(raw)
        .filter_map(|s| s.strip_prefix(prefix))
        .find(|value| valid_value(value))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utm {
    pub source: Option<String>,
    pub medium: Option<String>,
    pub campaign: Option<String>,
    pub content: Option<String>,
    pub term: Option<String>,
}

impl Utm {
    pub fn parse(raw: &str) -> Option<Utm> {
        let pick = |prefix: &str| segment(raw, prefix).map(str::to_string);
        let utm = Utm {
            source: pick("s_"),
            medium: pick("m_"),
            campaign: pick("k_"),
            content: pick("n_"),
            term: pick("t_"),
        };
        (!utm.is_empty()).then_some(utm)
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_none()
            && self.medium.is_none()
            && self.campaign.is_none()
            && self.content.is_none()
            && self.term.is_none()
    }

    pub fn from_parts(
        source: Option<&str>,
        medium: Option<&str>,
        campaign: Option<&str>,
        content: Option<&str>,
        term: Option<&str>,
    ) -> Option<Utm> {
        let pick = |value: Option<&str>| {
            value
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .filter(|v| tag_value(v))
                .map(str::to_string)
        };
        let utm = Utm {
            source: pick(source),
            medium: pick(medium),
            campaign: pick(campaign),
            content: pick(content),
            term: pick(term),
        };
        (!utm.is_empty()).then_some(utm)
    }

    pub fn to_param(&self) -> String {
        [
            ("s_", &self.source),
            ("m_", &self.medium),
            ("k_", &self.campaign),
            ("n_", &self.content),
            ("t_", &self.term),
        ]
        .iter()
        .filter_map(|(prefix, value)| value.as_deref().map(|v| format!("{prefix}{v}")))
        .collect::<Vec<_>>()
        .join(SEPARATOR)
    }
}

pub fn tag_value(value: &str) -> bool {
    valid_value(value)
        && !value.contains(SEPARATOR)
        && !value.starts_with('_')
        && !value.ends_with('_')
}

pub fn with_referral(param: Option<&str>, referral: Option<&str>) -> Option<String> {
    let parts: Vec<String> = referral
        .map(|code| format!("ref_{code}"))
        .into_iter()
        .chain(param.map(str::to_string))
        .filter(|p| !p.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(SEPARATOR))
}

pub const LINK_PREFIX: &str = "tglink_";

pub const SECRET_PREFIXES: [&str; 4] = [
    crate::catalog::gifts::START_PREFIX,
    crate::catalog::promolinks::START_PREFIX,
    "auth_",
    LINK_PREFIX,
];

pub fn masked(raw: &str) -> String {
    segments(raw)
        .map(|segment| {
            SECRET_PREFIXES
                .iter()
                .find(|prefix| segment.starts_with(**prefix))
                .map_or(segment, |prefix| prefix)
        })
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

pub fn referral_code(raw: &str) -> Option<String> {
    segment(raw, "ref_")
        .map(str::to_ascii_uppercase)
        .filter(|code| code.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn метки_разбираются_через_двойное_подчёркивание() {
        let utm =
            Utm::parse("ref_ABC123__s_tiktok__m_cpc__k_winter-sale__n_video1__t_vpn").unwrap();
        assert_eq!(utm.source.as_deref(), Some("tiktok"));
        assert_eq!(utm.medium.as_deref(), Some("cpc"));
        assert_eq!(utm.campaign.as_deref(), Some("winter-sale"));
        assert_eq!(utm.content.as_deref(), Some("video1"));
        assert_eq!(utm.term.as_deref(), Some("vpn"));
        assert_eq!(
            referral_code("ref_abc123__s_tiktok").as_deref(),
            Some("ABC123")
        );
    }

    #[test]
    fn чужие_параметры_не_принимаются_за_метки() {
        assert_eq!(Utm::parse("auth_0123456789abcdef"), None);
        assert_eq!(Utm::parse("ref_ABC"), None);
        assert_eq!(Utm::parse(""), None);
        assert_eq!(Utm::parse("s_"), None);
        assert_eq!(Utm::parse("s_bad value"), None);
        assert_eq!(referral_code("auth_x"), None);
        assert_eq!(referral_code("ref_"), None);
        assert_eq!(referral_code("ref_A-B"), None);
    }

    #[test]
    fn метки_собираются_в_параметр_и_разбираются_обратно() {
        let utm = Utm::from_parts(
            Some("tiktok"),
            Some("video"),
            Some("osen-shorts"),
            None,
            Some(" "),
        )
        .unwrap();
        assert_eq!(utm.to_param(), "s_tiktok__m_video__k_osen-shorts");
        assert_eq!(Utm::parse(&utm.to_param()), Some(utm));
        assert_eq!(
            Utm::from_parts(Some("telegram_ads"), None, None, None, None)
                .unwrap()
                .to_param(),
            "s_telegram_ads"
        );
        assert_eq!(
            Utm::from_parts(Some("bad__value"), Some("ok"), None, None, None)
                .unwrap()
                .to_param(),
            "m_ok"
        );
        assert_eq!(Utm::from_parts(Some("осень"), None, None, None, None), None);
        assert_eq!(Utm::from_parts(None, None, None, None, None), None);
        assert_eq!(
            with_referral(Some("s_tiktok"), Some("ABC123")).as_deref(),
            Some("ref_ABC123__s_tiktok")
        );
        assert_eq!(with_referral(None, None), None);
    }

    #[test]
    fn коды_подарков_промо_ссылок_и_входа_скрываются() {
        assert_eq!(masked("gift_WRWM-48NG-B662"), "gift_");
        assert_eq!(masked("pl_blogger-nov"), "pl_");
        assert_eq!(masked("auth_0123456789abcdef"), "auth_");
        assert_eq!(masked("tglink_0123456789abcdef"), "tglink_");
        assert_eq!(
            masked("ref_ABC123__s_tiktok__gift_WRWM-48NG-B662"),
            "ref_ABC123__s_tiktok__gift_"
        );
        assert_eq!(masked("youtube"), "youtube");
        assert_eq!(masked(""), "");
    }

    #[test]
    fn берётся_первое_значение_метки() {
        let utm = Utm::parse("s_first__s_second").unwrap();
        assert_eq!(utm.source.as_deref(), Some("first"));
        assert_eq!(utm.medium, None);
    }
}
