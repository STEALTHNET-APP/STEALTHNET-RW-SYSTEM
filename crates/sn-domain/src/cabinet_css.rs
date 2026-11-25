use crate::css::{CssSite, Place, PlacedCss};
use crate::image::ImageKind;
use serde::{Deserialize, Serialize};

pub const FILES: &str = "/api/public/cabinet/";

pub const STORED_IMAGES_MAX: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CssPlace {
    Page,
    Header,
    Login,
    Subscription,
    Connect,
    Pay,
    Devices,
    Money,
    Friends,
    Gifts,
    Help,
    Security,
    Footer,
}

impl Place for CssPlace {
    const SITE: CssSite = CssSite {
        files: FILES,
        classes: &["cb", "cb-phone"],
    };

    const ALL: &'static [CssPlace] = &[
        CssPlace::Page,
        CssPlace::Header,
        CssPlace::Login,
        CssPlace::Subscription,
        CssPlace::Connect,
        CssPlace::Pay,
        CssPlace::Devices,
        CssPlace::Money,
        CssPlace::Friends,
        CssPlace::Gifts,
        CssPlace::Help,
        CssPlace::Security,
        CssPlace::Footer,
    ];

    fn as_str(self) -> &'static str {
        match self {
            CssPlace::Page => "page",
            CssPlace::Header => "header",
            CssPlace::Login => "login",
            CssPlace::Subscription => "subscription",
            CssPlace::Connect => "connect",
            CssPlace::Pay => "pay",
            CssPlace::Devices => "devices",
            CssPlace::Money => "money",
            CssPlace::Friends => "friends",
            CssPlace::Gifts => "gifts",
            CssPlace::Help => "help",
            CssPlace::Security => "security",
            CssPlace::Footer => "footer",
        }
    }

    fn scope(self) -> Option<String> {
        (self != CssPlace::Page).then(|| format!("[data-part=\"{}\"]", self.as_str()))
    }

    fn title(self) -> String {
        let name = match self {
            CssPlace::Page => return "Общий CSS".to_string(),
            CssPlace::Header => "Шапка и меню",
            CssPlace::Login => "Вход и регистрация",
            CssPlace::Subscription => "Подписка",
            CssPlace::Connect => "Подключение",
            CssPlace::Pay => "Тарифы и оплата",
            CssPlace::Devices => "Устройства",
            CssPlace::Money => "Пополнение и история оплат",
            CssPlace::Friends => "Друзья",
            CssPlace::Gifts => "Подарки",
            CssPlace::Help => "Помощь",
            CssPlace::Security => "Безопасность",
            CssPlace::Footer => "Ссылки внизу",
        };
        format!("{name}, CSS раздела")
    }
}

pub type CabinetCss = PlacedCss<CssPlace>;

pub fn image_path(tag: &str, kind: ImageKind) -> String {
    format!("{FILES}{tag}.{}", kind.extension())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::check;
    use serde_json::json;

    #[test]
    fn раздел_ограничен_своим_местом_а_общий_css_нет() {
        let part = check(CssPlace::Pay, ".cb-card { color: red }").unwrap();
        assert!(
            part.style.contains("[data-part=\"pay\"] .cb-card"),
            "{}",
            part.style
        );
        let bare = check(CssPlace::Money, "color: red;").unwrap();
        assert!(
            bare.style.starts_with("[data-part=\"money\"]"),
            "{}",
            bare.style
        );
        let outside = check(
            CssPlace::Header,
            "[data-theme='dark'] .cb .cb-tab { color: red }",
        )
        .unwrap();
        assert!(
            outside
                .style
                .contains("[data-theme='dark'] .cb [data-part=\"header\"] .cb-tab"),
            "{}",
            outside.style
        );
        let page = check(CssPlace::Page, ".cb-card { color: red }").unwrap();
        assert!(!page.style.contains("data-part"), "{}", page.style);
    }

    #[test]
    fn у_каждого_места_своё_имя_и_предел() {
        for place in CssPlace::ALL {
            assert_eq!(CssPlace::parse(place.as_str()), Some(*place));
            assert!(!place.title().is_empty());
            let wide = *place == CssPlace::Page;
            assert_eq!(place.limit() == crate::css::PAGE_CSS_MAX, wide);
        }
        assert_eq!(CssPlace::parse("stories"), None);
    }

    #[test]
    fn картинки_только_свои_по_адресу_кабинета() {
        let tag = "0123456789abcdef";
        let own = check(
            CssPlace::Page,
            &format!(
                ".cb {{ background: url(\"{}\") }}",
                image_path(tag, ImageKind::Png)
            ),
        )
        .unwrap();
        assert_eq!(
            own.images.keys().cloned().collect::<Vec<_>>(),
            vec![tag.to_string()]
        );
        let mini = check(
            CssPlace::Page,
            &format!(".cb {{ background: url(/api/public/miniapp/{tag}.png) }}"),
        );
        assert!(mini.is_err());
        assert!(
            check(
                CssPlace::Page,
                ".cb { background: url(https://example.com/a.png) }"
            )
            .is_err()
        );
    }

    #[test]
    fn места_совпадают_со_списком_админки() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/src/admin/cabinet/places.ts"
        );
        let source = std::fs::read_to_string(path).unwrap();
        let line = source
            .lines()
            .find(|line| line.starts_with("export const CABINET_PLACES"))
            .unwrap();
        let listed: Vec<&str> = line.split('\'').skip(1).step_by(2).collect();
        let ours: Vec<&str> = CssPlace::ALL.iter().map(|place| place.as_str()).collect();
        assert_eq!(listed, ours);
    }

    #[test]
    fn неизвестные_и_пустые_места_при_чтении_пропускаются() {
        let css: CabinetCss = serde_json::from_value(
            json!({ "page": ".cb {}", "stories": "x", "pay": "", "help": null }),
        )
        .unwrap();
        assert_eq!(
            css.places().map(|(place, _)| place).collect::<Vec<_>>(),
            vec![CssPlace::Page]
        );
    }
}
