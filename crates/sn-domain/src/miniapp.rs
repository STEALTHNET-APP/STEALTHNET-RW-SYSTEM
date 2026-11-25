use crate::css::{BLOCK_CSS_MAX, PAGE_CSS_MAX};
use crate::image::ImageKind;
pub use crate::landing::LandingImage as Picture;
use crate::landing::{clean_line, clean_paragraph, valid_tag};
use crate::miniapp_css::MiniCss;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Display;

pub const BANNERS_MAX: usize = 5;
pub const BANNER_TITLE_MAX: usize = 80;
pub const BANNER_TEXT_MAX: usize = 200;
pub const GREETING_TITLE_MAX: usize = 60;
pub const GREETING_TEXT_MAX: usize = 300;
pub const EMPTY_TITLE_MAX: usize = 40;
pub const EMPTY_LINE_MAX: usize = 80;
pub const BUTTON_MAX: usize = 24;
pub const LABEL_MAX: usize = 40;
pub const TAB_LABEL_MAX: usize = 12;
pub const FRIENDS_TITLE_MAX: usize = 40;
pub const FRIENDS_TEXT_MAX: usize = 300;
pub const URL_MAX: usize = 500;
pub const ACTIONS_MAX: usize = 4;
pub const TABS_MIN: usize = 3;
pub const TABS_MAX: usize = 5;
pub const LINKS_MAX: usize = 8;
pub const STORED_IMAGES_MAX: usize = 40;

const URL_HINT: &str = "нужна ссылка вида https://… или t.me/…";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub banners: usize,
    pub banner_title: usize,
    pub banner_text: usize,
    pub greeting_title: usize,
    pub greeting_text: usize,
    pub empty_title: usize,
    pub empty_line: usize,
    pub button: usize,
    pub label: usize,
    pub tab: usize,
    pub friends_title: usize,
    pub friends_text: usize,
    pub url: usize,
    pub actions: usize,
    pub tabs_min: usize,
    pub tabs_max: usize,
    pub links: usize,
    pub page_css: usize,
    pub block_css: usize,
}

pub const LIMITS: Limits = Limits {
    banners: BANNERS_MAX,
    banner_title: BANNER_TITLE_MAX,
    banner_text: BANNER_TEXT_MAX,
    greeting_title: GREETING_TITLE_MAX,
    greeting_text: GREETING_TEXT_MAX,
    empty_title: EMPTY_TITLE_MAX,
    empty_line: EMPTY_LINE_MAX,
    button: BUTTON_MAX,
    label: LABEL_MAX,
    tab: TAB_LABEL_MAX,
    friends_title: FRIENDS_TITLE_MAX,
    friends_text: FRIENDS_TEXT_MAX,
    url: URL_MAX,
    actions: ACTIONS_MAX,
    tabs_min: TABS_MIN,
    tabs_max: TABS_MAX,
    links: LINKS_MAX,
    page_css: PAGE_CSS_MAX,
    block_css: BLOCK_CSS_MAX,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MiniAppError(pub String);

type Checked<T> = Result<T, MiniAppError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Home,
    Plans,
    Connect,
    Profile,
    Friends,
    Roulette,
    Addons,
    Gifts,
    Promo,
    Payments,
    Guide,
    Support,
    Topup,
    Trial,
    Renew,
    Devices,
}

impl Section {
    pub const ALL: [Section; 16] = [
        Section::Home,
        Section::Plans,
        Section::Connect,
        Section::Profile,
        Section::Friends,
        Section::Roulette,
        Section::Addons,
        Section::Gifts,
        Section::Promo,
        Section::Payments,
        Section::Guide,
        Section::Support,
        Section::Topup,
        Section::Trial,
        Section::Renew,
        Section::Devices,
    ];

    pub const ROWS: [Section; 8] = [
        Section::Friends,
        Section::Roulette,
        Section::Addons,
        Section::Gifts,
        Section::Promo,
        Section::Payments,
        Section::Guide,
        Section::Support,
    ];

    pub const ACTIONS: [Section; 11] = [
        Section::Renew,
        Section::Devices,
        Section::Addons,
        Section::Topup,
        Section::Friends,
        Section::Gifts,
        Section::Promo,
        Section::Roulette,
        Section::Payments,
        Section::Guide,
        Section::Support,
    ];

    pub const TABS: [Section; 11] = [
        Section::Home,
        Section::Plans,
        Section::Connect,
        Section::Profile,
        Section::Friends,
        Section::Support,
        Section::Roulette,
        Section::Gifts,
        Section::Promo,
        Section::Payments,
        Section::Topup,
    ];

    pub const TARGETS: [Section; 12] = [
        Section::Plans,
        Section::Connect,
        Section::Trial,
        Section::Topup,
        Section::Addons,
        Section::Friends,
        Section::Gifts,
        Section::Promo,
        Section::Roulette,
        Section::Payments,
        Section::Support,
        Section::Profile,
    ];

    pub const ALWAYS_TABS: [Section; 3] = [Section::Home, Section::Plans, Section::Profile];

    pub const fn as_str(self) -> &'static str {
        match self {
            Section::Home => "home",
            Section::Plans => "plans",
            Section::Connect => "connect",
            Section::Profile => "profile",
            Section::Friends => "friends",
            Section::Roulette => "roulette",
            Section::Addons => "addons",
            Section::Gifts => "gifts",
            Section::Promo => "promo",
            Section::Payments => "payments",
            Section::Guide => "guide",
            Section::Support => "support",
            Section::Topup => "topup",
            Section::Trial => "trial",
            Section::Renew => "renew",
            Section::Devices => "devices",
        }
    }

    pub fn parse(raw: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub const fn title(self) -> &'static str {
        match self {
            Section::Home => "Главная",
            Section::Plans => "Тарифы",
            Section::Connect => "Подключение",
            Section::Profile => "Профиль",
            Section::Friends => "Друзья",
            Section::Roulette => "Рулетка",
            Section::Addons => "Докупить устройства или трафик",
            Section::Gifts => "Подарить подписку",
            Section::Promo => "Промокод",
            Section::Payments => "История оплат",
            Section::Guide => "Инструкции",
            Section::Support => "Поддержка",
            Section::Topup => "Пополнение баланса",
            Section::Trial => "Пробный период",
            Section::Renew => "Продлить",
            Section::Devices => "Устройства",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    New,
    Trial,
    Active,
    Ended,
}

impl Audience {
    pub const ALL: [Audience; 4] = [
        Audience::New,
        Audience::Trial,
        Audience::Active,
        Audience::Ended,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Glyph {
    Send,
    Globe,
    Chat,
    Mail,
    Book,
    File,
    Gift,
    Percent,
    Trophy,
    Zap,
    Flag,
    Phone,
}

impl Glyph {
    pub const ALL: [Glyph; 12] = [
        Glyph::Send,
        Glyph::Globe,
        Glyph::Chat,
        Glyph::Mail,
        Glyph::Book,
        Glyph::File,
        Glyph::Gift,
        Glyph::Percent,
        Glyph::Trophy,
        Glyph::Zap,
        Glyph::Flag,
        Glyph::Phone,
    ];
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phrase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ru: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub en: Option<String>,
}

impl Phrase {
    pub fn is_unset(&self) -> bool {
        self.ru.is_none() && self.en.is_none()
    }

    fn each(
        self,
        at: &At,
        max: usize,
        keep_empty: bool,
        clean: fn(&str) -> String,
    ) -> Checked<Phrase> {
        let one = |raw: Option<String>, at: &At| -> Checked<Option<String>> {
            let Some(raw) = raw else {
                return Ok(None);
            };
            let text = clean(&raw);
            fits(&text, at, max)?;
            Ok((keep_empty || !text.is_empty()).then_some(text))
        };
        Ok(Phrase {
            ru: one(self.ru, at)?,
            en: one(self.en, &at.english())?,
        })
    }

    fn line(self, at: &At, max: usize) -> Checked<Phrase> {
        self.each(at, max, false, clean_line)
    }

    fn paragraph(self, at: &At, max: usize) -> Checked<Phrase> {
        self.each(at, max, false, clean_paragraph)
    }

    fn optional_line(self, at: &At, max: usize) -> Checked<Phrase> {
        self.each(at, max, true, clean_line)
    }

    fn optional_paragraph(self, at: &At, max: usize) -> Checked<Phrase> {
        self.each(at, max, true, clean_paragraph)
    }

    fn required_line(self, at: &At, max: usize) -> Checked<Phrase> {
        let phrase = self.line(at, max)?;
        if phrase.ru.is_none() {
            return Err(at.fail("нужно заполнить"));
        }
        Ok(phrase)
    }
}

const fn yes() -> bool {
    true
}

const fn is_true(value: &bool) -> bool {
    *value
}

const fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub balance: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub avatar: bool,
}

impl Default for Header {
    fn default() -> Self {
        Header {
            balance: true,
            avatar: false,
        }
    }
}

impl Header {
    fn is_standard(&self) -> bool {
        *self == Header::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Banner {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Picture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_en: Option<Picture>,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub title: Phrase,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub text: Phrase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, rename = "for", skip_serializing_if = "Vec::is_empty")]
    pub audience: Vec<Audience>,
}

impl Default for Banner {
    fn default() -> Self {
        Banner {
            on: true,
            image: None,
            image_en: None,
            title: Phrase::default(),
            text: Phrase::default(),
            to: None,
            audience: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Banners {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Banner>,
}

impl Default for Banners {
    fn default() -> Self {
        Banners {
            on: true,
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Greeting {
    #[serde(default)]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub title: Phrase,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub text: Phrase,
    #[serde(default, rename = "for", skip_serializing_if = "Vec::is_empty")]
    pub audience: Vec<Audience>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    #[default]
    Ring,
    Compact,
}

impl Look {
    fn is_standard(&self) -> bool {
        *self == Look::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub title: Phrase,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub line: Phrase,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub button: Phrase,
}

impl Empty {
    fn is_unset(&self) -> bool {
        self.title.is_unset() && self.line.is_unset() && self.button.is_unset()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    #[serde(default, skip_serializing_if = "Look::is_standard")]
    pub look: Look,
    #[serde(default, skip_serializing_if = "Empty::is_unset")]
    pub empty: Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub std: Option<Section>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub label: Phrase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Glyph>,
    #[serde(default, rename = "for", skip_serializing_if = "Vec::is_empty")]
    pub audience: Vec<Audience>,
}

impl Default for Item {
    fn default() -> Self {
        Item {
            std: None,
            on: true,
            label: Phrase::default(),
            to: None,
            icon: None,
            audience: Vec::new(),
        }
    }
}

impl Item {
    pub fn standard(section: Section) -> Item {
        Item {
            std: Some(section),
            ..Item::default()
        }
    }
}

fn standard_actions() -> Vec<Item> {
    vec![
        Item::standard(Section::Renew),
        Item::standard(Section::Devices),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actions {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
    #[serde(default = "standard_actions")]
    pub items: Vec<Item>,
}

impl Default for Actions {
    fn default() -> Self {
        Actions {
            on: true,
            items: standard_actions(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rows {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
    #[serde(default)]
    pub items: Vec<Item>,
}

impl Default for Rows {
    fn default() -> Self {
        Rows {
            on: true,
            items: Section::ROWS.into_iter().map(Item::standard).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeKind {
    Banners,
    Greeting,
    Subscription,
    Actions,
    Sections,
}

impl HomeKind {
    pub const ALL: [HomeKind; 5] = [
        HomeKind::Banners,
        HomeKind::Greeting,
        HomeKind::Subscription,
        HomeKind::Actions,
        HomeKind::Sections,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            HomeKind::Banners => "banners",
            HomeKind::Greeting => "greeting",
            HomeKind::Subscription => "subscription",
            HomeKind::Actions => "actions",
            HomeKind::Sections => "sections",
        }
    }

    pub fn parse(raw: &str) -> Option<HomeKind> {
        HomeKind::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }

    pub const fn title(self) -> &'static str {
        match self {
            HomeKind::Banners => "Баннеры",
            HomeKind::Greeting => "Приветствие",
            HomeKind::Subscription => "Подписка",
            HomeKind::Actions => "Кнопки под подпиской",
            HomeKind::Sections => "Разделы",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HomeBlock {
    Banners(Banners),
    Greeting(Greeting),
    Subscription(Subscription),
    Actions(Actions),
    Sections(Rows),
}

impl HomeBlock {
    pub fn standard(kind: HomeKind) -> HomeBlock {
        match kind {
            HomeKind::Banners => HomeBlock::Banners(Banners::default()),
            HomeKind::Greeting => HomeBlock::Greeting(Greeting::default()),
            HomeKind::Subscription => HomeBlock::Subscription(Subscription::default()),
            HomeKind::Actions => HomeBlock::Actions(Actions::default()),
            HomeKind::Sections => HomeBlock::Sections(Rows::default()),
        }
    }

    pub const fn kind(&self) -> HomeKind {
        match self {
            HomeBlock::Banners(_) => HomeKind::Banners,
            HomeBlock::Greeting(_) => HomeKind::Greeting,
            HomeBlock::Subscription(_) => HomeKind::Subscription,
            HomeBlock::Actions(_) => HomeKind::Actions,
            HomeBlock::Sections(_) => HomeKind::Sections,
        }
    }

    pub const fn on(&self) -> bool {
        match self {
            HomeBlock::Banners(banners) => banners.on,
            HomeBlock::Greeting(greeting) => greeting.on,
            HomeBlock::Subscription(_) => true,
            HomeBlock::Actions(actions) => actions.on,
            HomeBlock::Sections(rows) => rows.on,
        }
    }

    fn clean(self) -> Checked<HomeBlock> {
        let at = At::new(format!("Главная, {}", lower(self.kind().title())));
        Ok(match self {
            HomeBlock::Banners(banners) => HomeBlock::Banners(banners.clean(&at)?),
            HomeBlock::Greeting(greeting) => HomeBlock::Greeting(greeting.clean(&at)?),
            HomeBlock::Subscription(sub) => HomeBlock::Subscription(sub.clean(&at)?),
            HomeBlock::Actions(actions) => HomeBlock::Actions(actions.clean(&at)?),
            HomeBlock::Sections(rows) => HomeBlock::Sections(rows.clean(&at)?),
        })
    }

    fn arranged(self) -> HomeBlock {
        match self {
            HomeBlock::Actions(actions) => HomeBlock::Actions(actions.arranged()),
            HomeBlock::Sections(rows) => HomeBlock::Sections(rows.arranged()),
            other => other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub std: Section,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub label: Phrase,
}

impl Tab {
    pub fn standard(section: Section) -> Tab {
        Tab {
            std: section,
            label: Phrase::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Person,
    Balance,
    Friends,
    Help,
    Links,
    Documents,
    Account,
}

impl ProfileKind {
    pub const ALL: [ProfileKind; 7] = [
        ProfileKind::Person,
        ProfileKind::Balance,
        ProfileKind::Friends,
        ProfileKind::Help,
        ProfileKind::Links,
        ProfileKind::Documents,
        ProfileKind::Account,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ProfileKind::Person => "person",
            ProfileKind::Balance => "balance",
            ProfileKind::Friends => "friends",
            ProfileKind::Help => "help",
            ProfileKind::Links => "links",
            ProfileKind::Documents => "documents",
            ProfileKind::Account => "account",
        }
    }

    pub fn parse(raw: &str) -> Option<ProfileKind> {
        ProfileKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == raw)
    }

    pub const fn title(self) -> &'static str {
        match self {
            ProfileKind::Person => "Фото и имя",
            ProfileKind::Balance => "Баланс",
            ProfileKind::Friends => "Друзья",
            ProfileKind::Help => "Оплаты и поддержка",
            ProfileKind::Links => "Свои ссылки",
            ProfileKind::Documents => "Документы",
            ProfileKind::Account => "Аккаунт",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    #[serde(default, skip_serializing_if = "is_false")]
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
}

impl Default for Shown {
    fn default() -> Self {
        Shown { on: true }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FriendsPlace {
    #[default]
    Card,
    Screen,
}

impl FriendsPlace {
    fn is_standard(&self) -> bool {
        *self == FriendsPlace::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Friends {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "FriendsPlace::is_standard")]
    pub place: FriendsPlace,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub title: Phrase,
    #[serde(default, skip_serializing_if = "Phrase::is_unset")]
    pub text: Phrase,
}

impl Default for Friends {
    fn default() -> Self {
        Friends {
            on: true,
            place: FriendsPlace::default(),
            title: Phrase::default(),
            text: Phrase::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Help {
    #[serde(default, skip_serializing_if = "is_false")]
    pub gifts: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixed {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Links {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
}

impl Default for Links {
    fn default() -> Self {
        Links {
            on: true,
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileBlock {
    Person(Person),
    Balance(Shown),
    Friends(Friends),
    Help(Help),
    Links(Links),
    Documents(Fixed),
    Account(Fixed),
}

impl ProfileBlock {
    pub fn standard(kind: ProfileKind) -> ProfileBlock {
        match kind {
            ProfileKind::Person => ProfileBlock::Person(Person::default()),
            ProfileKind::Balance => ProfileBlock::Balance(Shown::default()),
            ProfileKind::Friends => ProfileBlock::Friends(Friends::default()),
            ProfileKind::Help => ProfileBlock::Help(Help::default()),
            ProfileKind::Links => ProfileBlock::Links(Links::default()),
            ProfileKind::Documents => ProfileBlock::Documents(Fixed::default()),
            ProfileKind::Account => ProfileBlock::Account(Fixed::default()),
        }
    }

    pub const fn kind(&self) -> ProfileKind {
        match self {
            ProfileBlock::Person(_) => ProfileKind::Person,
            ProfileBlock::Balance(_) => ProfileKind::Balance,
            ProfileBlock::Friends(_) => ProfileKind::Friends,
            ProfileBlock::Help(_) => ProfileKind::Help,
            ProfileBlock::Links(_) => ProfileKind::Links,
            ProfileBlock::Documents(_) => ProfileKind::Documents,
            ProfileBlock::Account(_) => ProfileKind::Account,
        }
    }

    pub const fn on(&self) -> bool {
        match self {
            ProfileBlock::Person(person) => person.on,
            ProfileBlock::Balance(shown) => shown.on,
            ProfileBlock::Friends(friends) => friends.on,
            ProfileBlock::Links(links) => links.on,
            ProfileBlock::Help(_) | ProfileBlock::Documents(_) | ProfileBlock::Account(_) => true,
        }
    }

    fn clean(self) -> Checked<ProfileBlock> {
        let at = At::new(format!("Профиль, {}", lower(self.kind().title())));
        Ok(match self {
            ProfileBlock::Friends(friends) => ProfileBlock::Friends(friends.clean(&at)?),
            ProfileBlock::Links(links) => ProfileBlock::Links(links.clean(&at)?),
            other => other,
        })
    }

    fn arranged(self) -> ProfileBlock {
        match self {
            ProfileBlock::Links(links) => ProfileBlock::Links(Links {
                items: allowed(links.items, &[]),
                ..links
            }),
            other => other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MiniAppConfig {
    #[serde(default, skip_serializing_if = "Header::is_standard")]
    pub header: Header,
    #[serde(default)]
    pub home: Vec<HomeBlock>,
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub profile: Vec<ProfileBlock>,
    #[serde(default, skip_serializing_if = "MiniCss::is_empty")]
    pub css: MiniCss,
}

impl Default for MiniAppConfig {
    fn default() -> Self {
        MiniAppConfig::standard()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Tabs,
    Profile,
}

impl Screen {
    pub const fn as_str(self) -> &'static str {
        match self {
            Screen::Home => "home",
            Screen::Tabs => "tabs",
            Screen::Profile => "profile",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Difference {
    pub screens: Vec<&'static str>,
    pub blocks: Vec<&'static str>,
    pub order: bool,
}

impl MiniAppConfig {
    pub fn standard() -> MiniAppConfig {
        MiniAppConfig {
            header: Header::default(),
            home: HomeKind::ALL.into_iter().map(HomeBlock::standard).collect(),
            tabs: standard_tabs(),
            profile: ProfileKind::ALL
                .into_iter()
                .map(ProfileBlock::standard)
                .collect(),
            css: MiniCss::default(),
        }
    }

    pub fn is_standard(&self) -> bool {
        *self == MiniAppConfig::standard()
    }

    pub fn parse(raw: &Value) -> Checked<MiniAppConfig> {
        let Some(map) = raw.as_object() else {
            return Err(MiniAppError("Настройки заполнены неверно".into()));
        };
        let header = match map.get("header") {
            None | Some(Value::Null) => Header::default(),
            Some(value) => read(value, "Шапка")?,
        };
        let home = parse_blocks(
            map.get("home"),
            "Главная",
            HomeKind::parse,
            HomeKind::title,
            |block: HomeBlock| block.clean(),
        )?;
        let profile = parse_blocks(
            map.get("profile"),
            "Профиль",
            ProfileKind::parse,
            ProfileKind::title,
            |block: ProfileBlock| block.clean(),
        )?;
        let tabs = parse_tabs(map.get("tabs"))?;
        Ok(MiniAppConfig {
            header,
            home,
            tabs,
            profile,
            css: MiniCss::default(),
        }
        .arranged())
    }

    pub fn stored(raw: Value) -> Option<MiniAppConfig> {
        let Value::Object(mut map) = raw else {
            return None;
        };
        let header = map
            .remove("header")
            .and_then(|header| serde_json::from_value(header).ok())
            .unwrap_or_default();
        Some(
            MiniAppConfig {
                header,
                home: known_list(map.remove("home")),
                tabs: known_list(map.remove("tabs")),
                profile: known_list(map.remove("profile")),
                css: map
                    .remove("css")
                    .and_then(|css| serde_json::from_value(css).ok())
                    .unwrap_or_default(),
            }
            .arranged(),
        )
    }

    pub fn arranged(self) -> MiniAppConfig {
        let mut home: Vec<HomeBlock> = Vec::new();
        for block in self.home {
            if home.iter().all(|b| b.kind() != block.kind()) {
                home.push(block.arranged());
            }
        }
        fill(
            &mut home,
            &HomeKind::ALL,
            HomeBlock::kind,
            HomeBlock::standard,
        );
        let mut profile: Vec<ProfileBlock> = Vec::new();
        for block in self.profile {
            if profile.iter().all(|b| b.kind() != block.kind()) {
                profile.push(block.arranged());
            }
        }
        fill(
            &mut profile,
            &ProfileKind::ALL,
            ProfileBlock::kind,
            ProfileBlock::standard,
        );
        MiniAppConfig {
            header: self.header,
            home,
            tabs: arranged_tabs(self.tabs),
            profile,
            css: self.css,
        }
    }

    pub fn banners(&self) -> &[Banner] {
        self.home
            .iter()
            .find_map(|block| match block {
                HomeBlock::Banners(banners) => Some(banners.items.as_slice()),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn images(&self) -> Vec<&Picture> {
        self.banners()
            .iter()
            .flat_map(|banner| banner.image.iter().chain(banner.image_en.iter()))
            .collect()
    }

    pub fn images_mut(&mut self) -> Vec<(String, &mut Picture)> {
        let mut found = Vec::new();
        for block in &mut self.home {
            let HomeBlock::Banners(banners) = block else {
                continue;
            };
            for (index, banner) in banners.items.iter_mut().enumerate() {
                let place = format!("Главная, баннер {}", index + 1);
                if let Some(image) = banner.image.as_mut() {
                    found.push((format!("{place}, картинка"), image));
                }
                if let Some(image) = banner.image_en.as_mut() {
                    found.push((format!("{place}, картинка на английском"), image));
                }
            }
        }
        found
    }

    pub fn image_tags(&self) -> BTreeSet<String> {
        self.images()
            .into_iter()
            .map(|image| image.tag.clone())
            .collect()
    }

    pub fn with_urls(&self) -> MiniAppConfig {
        let mut copy = self.clone();
        copy.fill_urls();
        copy
    }

    fn fill_urls(&mut self) {
        for (_, image) in self.images_mut() {
            image.url = image.kind.map(|kind| image_path(&image.tag, kind));
        }
    }

    pub fn published(&self) -> MiniAppConfig {
        let own_on = |item: &Item| item.std.is_some() || item.on;
        let mut copy = MiniAppConfig {
            header: self.header.clone(),
            home: self.home.clone(),
            tabs: self.tabs.clone(),
            profile: self.profile.clone(),
            css: MiniCss::default(),
        };
        copy.fill_urls();
        for block in &mut copy.home {
            match block {
                HomeBlock::Banners(banners) => {
                    let shown = banners.on;
                    banners.items.retain(|banner| shown && banner.on);
                }
                HomeBlock::Greeting(greeting) if !greeting.on => {
                    *greeting = Greeting::default();
                }
                HomeBlock::Actions(actions) if !actions.on => actions.items.clear(),
                HomeBlock::Sections(rows) => {
                    let shown = rows.on;
                    rows.items
                        .retain(|item| own_on(item) && (shown || item.std.is_some()));
                }
                _ => {}
            }
        }
        for block in &mut copy.profile {
            if let ProfileBlock::Links(links) = block {
                let shown = links.on;
                links.items.retain(|item| shown && item.on);
            }
        }
        copy
    }

    pub fn friends(&self) -> Friends {
        self.profile
            .iter()
            .find_map(|block| match block {
                ProfileBlock::Friends(friends) => Some(friends.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn friends_in_profile(&self) -> bool {
        let friends = self.friends();
        friends.on && friends.place == FriendsPlace::Card
    }

    pub fn difference(&self, next: &MiniAppConfig) -> Difference {
        let mut out = Difference::default();
        let home_order = self
            .home
            .iter()
            .map(HomeBlock::kind)
            .ne(next.home.iter().map(HomeBlock::kind));
        let profile_order = self
            .profile
            .iter()
            .map(ProfileBlock::kind)
            .ne(next.profile.iter().map(ProfileBlock::kind));
        if self.header != next.header {
            out.blocks.push("header");
        }
        for kind in HomeKind::ALL {
            let find = |config: &MiniAppConfig| {
                config
                    .home
                    .iter()
                    .find(|block| block.kind() == kind)
                    .cloned()
            };
            if find(self) != find(next) {
                out.blocks.push(kind.as_str());
            }
        }
        if home_order || !out.blocks.is_empty() {
            out.screens.push(Screen::Home.as_str());
        }
        if self.tabs != next.tabs {
            out.screens.push(Screen::Tabs.as_str());
        }
        let before = out.blocks.len();
        for kind in ProfileKind::ALL {
            let find = |config: &MiniAppConfig| {
                config
                    .profile
                    .iter()
                    .find(|block| block.kind() == kind)
                    .cloned()
            };
            if find(self) != find(next) {
                out.blocks.push(kind.as_str());
            }
        }
        if profile_order || out.blocks.len() > before {
            out.screens.push(Screen::Profile.as_str());
        }
        out.order = home_order || profile_order;
        out
    }

    pub fn hidden(&self) -> Vec<&'static str> {
        let home = self
            .home
            .iter()
            .filter(|block| !block.on() && block.kind() != HomeKind::Greeting)
            .map(|block| block.kind().as_str());
        let profile = self
            .profile
            .iter()
            .filter(|block| !block.on() && block.kind() != ProfileKind::Person)
            .map(|block| block.kind().as_str());
        home.chain(profile).collect()
    }
}

pub fn image_path(tag: &str, kind: ImageKind) -> String {
    format!("/api/public/miniapp/{tag}.{}", kind.extension())
}

fn standard_tabs() -> Vec<Tab> {
    [
        Section::Home,
        Section::Plans,
        Section::Connect,
        Section::Profile,
    ]
    .into_iter()
    .map(Tab::standard)
    .collect()
}

fn fill<B, K: Copy + PartialEq>(
    blocks: &mut Vec<B>,
    all: &[K],
    kind: impl Fn(&B) -> K,
    standard: impl Fn(K) -> B,
) {
    for (place, wanted) in all.iter().copied().enumerate() {
        if blocks.iter().any(|block| kind(block) == wanted) {
            continue;
        }
        let at = all[..place]
            .iter()
            .rev()
            .find_map(|before| {
                blocks
                    .iter()
                    .position(|block| kind(block) == *before)
                    .map(|index| index + 1)
            })
            .unwrap_or(0);
        blocks.insert(at, standard(wanted));
    }
}

fn allowed(items: Vec<Item>, sections: &[Section]) -> Vec<Item> {
    let mut kept: Vec<Item> = Vec::with_capacity(items.len());
    for item in items {
        let keep = match item.std {
            Some(section) => {
                sections.contains(&section) && kept.iter().all(|i| i.std != Some(section))
            }
            None => true,
        };
        if keep {
            kept.push(item);
        }
    }
    kept
}

fn known<T: DeserializeOwned>(value: &Value) -> bool {
    T::deserialize(value).is_ok()
}

fn known_target(value: &Value) -> bool {
    value.as_str().is_some_and(|to| target(to).is_ok())
}

type Check = fn(&Value) -> bool;

const CHECKED_FIELDS: [(&str, Check); 4] = [
    ("look", known::<Look>),
    ("place", known::<FriendsPlace>),
    ("icon", known::<Option<Glyph>>),
    ("to", known_target),
];

fn known_list<T: DeserializeOwned>(raw: Option<Value>) -> Vec<T> {
    let Some(Value::Array(list)) = raw else {
        return Vec::new();
    };
    list.into_iter()
        .filter_map(|mut value| {
            drop_unknown(&mut value);
            serde_json::from_value(value).ok()
        })
        .collect()
}

fn drop_unknown(value: &mut Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    for (field, check) in CHECKED_FIELDS {
        if map.get(field).is_some_and(|value| !check(value)) {
            map.remove(field);
        }
    }
    if let Some(Value::Array(audience)) = map.get_mut("for") {
        let chosen = audience.len();
        audience.retain(known::<Audience>);
        if audience.is_empty() && chosen > 0 {
            map.insert("on".into(), Value::Bool(false));
        }
    }
    let banners = map.get("kind").and_then(Value::as_str) == Some(HomeKind::Banners.as_str());
    if let Some(Value::Array(items)) = map.get_mut("items") {
        items.iter_mut().for_each(drop_unknown);
        items.retain(|item| {
            if banners {
                known::<Banner>(item)
            } else {
                Item::deserialize(item).is_ok_and(|item| item.std.is_some() || item.to.is_some())
            }
        });
    }
}

fn read<T: DeserializeOwned>(value: &Value, place: &str) -> Checked<T> {
    serde_json::from_value(value.clone())
        .map_err(|err| MiniAppError(format!("{place}: поля заполнены неверно ({err})")))
}

fn shown(raw: &str) -> String {
    raw.chars().take(40).collect()
}

fn parse_blocks<K: Copy + Ord, B: DeserializeOwned>(
    raw: Option<&Value>,
    screen: &str,
    parse: fn(&str) -> Option<K>,
    title: fn(K) -> &'static str,
    clean: impl Fn(B) -> Checked<B>,
) -> Checked<Vec<B>> {
    let list = match raw {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err(MiniAppError(format!("{screen}: нужен список блоков"))),
    };
    let mut seen = BTreeSet::new();
    let mut blocks = Vec::with_capacity(list.len());
    for (index, value) in list.iter().enumerate() {
        let place = format!("{screen}, блок {}", index + 1);
        let Some(name) = value.get("kind").and_then(Value::as_str) else {
            return Err(MiniAppError(format!(
                "{place}: не указано, что это за блок"
            )));
        };
        let Some(kind) = parse(name) else {
            return Err(MiniAppError(format!(
                "{place}: неизвестный блок «{}»",
                shown(name)
            )));
        };
        if !seen.insert(kind) {
            return Err(MiniAppError(format!(
                "{screen}: блок «{}» указан дважды",
                title(kind)
            )));
        }
        let block: B = read(value, &format!("{screen}, {}", lower(title(kind))))?;
        blocks.push(clean(block)?);
    }
    Ok(blocks)
}

fn parse_tabs(raw: Option<&Value>) -> Checked<Vec<Tab>> {
    let at = At::new("Меню внизу");
    let list = match raw {
        None | Some(Value::Null) => return Ok(standard_tabs()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err(at.fail("нужен список вкладок")),
    };
    if list.is_empty() {
        return Ok(standard_tabs());
    }
    if list.len() > TABS_MAX {
        return Err(at.fail(format!("не больше {TABS_MAX} вкладок")));
    }
    let mut tabs: Vec<Tab> = Vec::with_capacity(list.len());
    for (index, value) in list.iter().enumerate() {
        let place = at.then(format!("вкладка {}", index + 1));
        let name = value.get("std").and_then(Value::as_str).unwrap_or_default();
        let Some(section) = Section::parse(name).filter(|s| Section::TABS.contains(s)) else {
            return Err(place.fail(format!("раздела «{}» для вкладки нет", shown(name))));
        };
        if tabs.iter().any(|tab| tab.std == section) {
            return Err(place.fail(format!("«{}» уже есть в меню", section.title())));
        }
        let tab: Tab = read(value, &place.0)?;
        tabs.push(Tab {
            std: section,
            label: tab.label.line(&place.then("надпись"), TAB_LABEL_MAX)?,
        });
    }
    for section in Section::ALWAYS_TABS {
        if tabs.iter().all(|tab| tab.std != section) {
            return Err(at.fail(format!(
                "вкладку «{}» убрать нельзя: {}",
                section.title(),
                always_reason(section)
            )));
        }
    }
    Ok(tabs)
}

const fn always_reason(section: Section) -> &'static str {
    match section {
        Section::Plans => "без неё клиент не оплатит подписку",
        Section::Profile => "без неё клиент не найдёт поддержку и свой аккаунт",
        _ => "без неё клиент не увидит подписку",
    }
}

fn arranged_tabs(raw: Vec<Tab>) -> Vec<Tab> {
    let mut tabs: Vec<Tab> = Vec::new();
    for tab in raw {
        if Section::TABS.contains(&tab.std) && tabs.iter().all(|t| t.std != tab.std) {
            tabs.push(tab);
        }
    }
    let complete = Section::ALWAYS_TABS
        .iter()
        .all(|section| tabs.iter().any(|tab| tab.std == *section));
    if complete && (TABS_MIN..=TABS_MAX).contains(&tabs.len()) {
        tabs
    } else {
        standard_tabs()
    }
}

struct At(String);

impl At {
    fn new(place: impl Into<String>) -> At {
        At(place.into())
    }

    fn then(&self, part: impl Display) -> At {
        At(format!("{}, {part}", self.0))
    }

    fn english(&self) -> At {
        At(format!("{} на английском", self.0))
    }

    fn fail(&self, reason: impl Display) -> MiniAppError {
        MiniAppError(format!("{}: {reason}", self.0))
    }
}

fn lower(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn fits(text: &str, at: &At, max: usize) -> Checked<()> {
    if text.chars().count() > max {
        Err(at.fail(format!("не длиннее {max} символов")))
    } else {
        Ok(())
    }
}

fn clean_audience(raw: Vec<Audience>) -> Vec<Audience> {
    let set: BTreeSet<Audience> = raw.into_iter().collect();
    if set.len() == Audience::ALL.len() {
        Vec::new()
    } else {
        set.into_iter().collect()
    }
}

fn clean_picture(picture: Option<Picture>, at: &At) -> Checked<Option<Picture>> {
    let Some(picture) = picture else {
        return Ok(None);
    };
    let tag = picture.tag.trim().to_ascii_lowercase();
    if !valid_tag(&tag) {
        return Err(at.fail("указана неверно, загрузите её заново"));
    }
    Ok(Some(Picture {
        tag,
        ..Picture::default()
    }))
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

pub fn target(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("укажите, куда ведёт".into());
    }
    if let Some(section) = Section::parse(raw) {
        return if Section::TARGETS.contains(&section) {
            Ok(section.as_str().to_string())
        } else {
            Err("на этот раздел сослаться нельзя".into())
        };
    }
    let url = if starts_with_ignore_case(raw, "t.me/") && raw.len() > "t.me/".len() {
        format!("https://{raw}")
    } else {
        raw.to_string()
    };
    https_url(&url)
}

fn https_url(url: &str) -> Result<String, String> {
    if url.chars().count() > URL_MAX {
        return Err(format!("не длиннее {URL_MAX} символов"));
    }
    if !starts_with_ignore_case(url, "https://") {
        return Err(URL_HINT.into());
    }
    let forbidden = |c: char| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                c,
                '"' | '\'' | '<' | '>' | '`' | '\\' | '{' | '}' | '|' | '^'
            )
    };
    if url.chars().any(forbidden) {
        return Err("в ссылке не должно быть пробелов, кавычек и скобок".into());
    }
    let rest = &url["https://".len()..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        && host.chars().any(char::is_alphanumeric)
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-']);
    let port_ok =
        port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    if !host_ok || !port_ok {
        return Err("в ссылке нет адреса сайта".into());
    }
    if host.eq_ignore_ascii_case("t.me") {
        return Ok(format!("https://t.me{tail}"));
    }
    Ok(format!("https://{rest}"))
}

fn clean_target(raw: Option<String>, at: &At) -> Checked<Option<String>> {
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(value) => target(value).map(Some).map_err(|reason| at.fail(reason)),
    }
}

impl Banners {
    fn clean(self, at: &At) -> Checked<Banners> {
        if self.items.len() > BANNERS_MAX {
            return Err(at.fail(format!("не больше {BANNERS_MAX}")));
        }
        let items = self
            .items
            .into_iter()
            .enumerate()
            .map(|(index, banner)| banner.clean(&At::new(format!("Главная, баннер {}", index + 1))))
            .collect::<Checked<Vec<_>>>()?;
        Ok(Banners { on: self.on, items })
    }
}

impl Banner {
    fn clean(self, at: &At) -> Checked<Banner> {
        let image = clean_picture(self.image, &at.then("картинка"))?;
        let image_en = clean_picture(self.image_en, &at.then("картинка на английском"))?;
        let title = self.title.line(&at.then("заголовок"), BANNER_TITLE_MAX)?;
        let text = self.text.paragraph(&at.then("текст"), BANNER_TEXT_MAX)?;
        if image.is_none() && title.ru.is_none() {
            return Err(at.fail("нужна картинка или заголовок"));
        }
        let to = clean_target(self.to, &at.then("ссылка"))?;
        Ok(Banner {
            on: self.on,
            image,
            image_en,
            title,
            text,
            to,
            audience: clean_audience(self.audience),
        })
    }
}

impl Greeting {
    fn clean(self, at: &At) -> Checked<Greeting> {
        let title = self.title.line(&at.then("заголовок"), GREETING_TITLE_MAX)?;
        let text = self.text.paragraph(&at.then("текст"), GREETING_TEXT_MAX)?;
        if self.on && title.ru.is_none() && text.ru.is_none() {
            return Err(at.fail("нужен заголовок или текст"));
        }
        Ok(Greeting {
            on: self.on,
            title,
            text,
            audience: clean_audience(self.audience),
        })
    }
}

impl Subscription {
    fn clean(self, at: &At) -> Checked<Subscription> {
        let at = at.then("без подписки");
        Ok(Subscription {
            look: self.look,
            empty: Empty {
                title: self
                    .empty
                    .title
                    .line(&at.then("заголовок"), EMPTY_TITLE_MAX)?,
                line: self
                    .empty
                    .line
                    .optional_line(&at.then("строка"), EMPTY_LINE_MAX)?,
                button: self.empty.button.line(&at.then("кнопка"), BUTTON_MAX)?,
            },
        })
    }
}

impl Item {
    fn clean_std(self, at: &At, allowed: &[Section], max: usize) -> Checked<Item> {
        let Some(section) = self.std else {
            return Err(at.fail("не указан раздел"));
        };
        if !allowed.contains(&section) {
            return Err(at.fail(format!("раздела «{}» здесь нет", section.title())));
        }
        if self.to.is_some() || self.icon.is_some() || !self.audience.is_empty() {
            return Err(at.fail("у раздела не бывает своей ссылки и значка"));
        }
        Ok(Item {
            std: Some(section),
            on: self.on,
            label: self.label.line(&at.then("название"), max)?,
            ..Item::default()
        })
    }

    fn clean_own(self, at: &At, max: usize) -> Checked<Item> {
        let label = self.label.required_line(&at.then("название"), max)?;
        let to = target(self.to.as_deref().unwrap_or_default())
            .map_err(|reason| at.then("ссылка").fail(reason))?;
        Ok(Item {
            std: None,
            on: self.on,
            label,
            to: Some(to),
            icon: self.icon,
            audience: clean_audience(self.audience),
        })
    }
}

impl Actions {
    fn clean(self, at: &At) -> Checked<Actions> {
        if self.items.len() > ACTIONS_MAX {
            return Err(at.fail(format!("не больше {ACTIONS_MAX} кнопок")));
        }
        let mut seen = BTreeSet::new();
        let mut items = Vec::with_capacity(self.items.len());
        for (index, item) in self.items.into_iter().enumerate() {
            let place = At::new(format!("Главная, кнопка {}", index + 1));
            let mut item = match item.std {
                Some(section) => {
                    if !seen.insert(section) {
                        return Err(place.fail(format!("«{}» уже есть", section.title())));
                    }
                    item.clean_std(&place, &Section::ACTIONS, BUTTON_MAX)?
                }
                None => item.clean_own(&place, BUTTON_MAX)?,
            };
            item.on = true;
            items.push(item);
        }
        Ok(Actions { on: self.on, items })
    }

    fn arranged(self) -> Actions {
        Actions {
            on: self.on,
            items: allowed(self.items, &Section::ACTIONS),
        }
    }
}

impl Rows {
    fn clean(self, at: &At) -> Checked<Rows> {
        let own = self.items.iter().filter(|item| item.std.is_none()).count();
        if own > LINKS_MAX {
            return Err(at.fail(format!("не больше {LINKS_MAX} своих ссылок")));
        }
        let mut seen = BTreeSet::new();
        let mut items = Vec::with_capacity(self.items.len());
        for (index, item) in self.items.into_iter().enumerate() {
            items.push(match item.std {
                Some(section) => {
                    let place = at.then(format!("«{}»", section.title()));
                    if !seen.insert(section) {
                        return Err(place.fail("указан дважды"));
                    }
                    item.clean_std(&place, &Section::ROWS, LABEL_MAX)?
                }
                None => item.clean_own(&at.then(format!("пункт {}", index + 1)), LABEL_MAX)?,
            });
        }
        Ok(Rows { on: self.on, items }.arranged())
    }

    fn arranged(self) -> Rows {
        let mut items = allowed(self.items, &Section::ROWS);
        for (place, wanted) in Section::ROWS.into_iter().enumerate() {
            if items.iter().any(|item| item.std == Some(wanted)) {
                continue;
            }
            let at = Section::ROWS[..place]
                .iter()
                .rev()
                .find_map(|before| {
                    items
                        .iter()
                        .position(|item| item.std == Some(*before))
                        .map(|index| index + 1)
                })
                .unwrap_or(0);
            items.insert(at, Item::standard(wanted));
        }
        Rows { on: self.on, items }
    }
}

impl Friends {
    fn clean(self, at: &At) -> Checked<Friends> {
        Ok(Friends {
            on: self.on,
            place: self.place,
            title: self.title.line(&at.then("заголовок"), FRIENDS_TITLE_MAX)?,
            text: self
                .text
                .optional_paragraph(&at.then("текст"), FRIENDS_TEXT_MAX)?,
        })
    }
}

impl Links {
    fn clean(self, at: &At) -> Checked<Links> {
        if self.items.len() > LINKS_MAX {
            return Err(at.fail(format!("не больше {LINKS_MAX}")));
        }
        let items = self
            .items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let place = at.then(format!("ссылка {}", index + 1));
                if item.std.is_some() {
                    return Err(place.fail("здесь только свои ссылки"));
                }
                item.clean_own(&place, LABEL_MAX)
            })
            .collect::<Checked<Vec<_>>>()?;
        Ok(Links { on: self.on, items })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Checked<MiniAppConfig> {
        MiniAppConfig::parse(&value)
    }

    fn message(result: Checked<MiniAppConfig>) -> String {
        result.unwrap_err().0
    }

    fn home(config: &MiniAppConfig) -> Vec<&'static str> {
        config.home.iter().map(|b| b.kind().as_str()).collect()
    }

    fn profile(config: &MiniAppConfig) -> Vec<&'static str> {
        config.profile.iter().map(|b| b.kind().as_str()).collect()
    }

    fn tabs(config: &MiniAppConfig) -> Vec<&'static str> {
        config.tabs.iter().map(|t| t.std.as_str()).collect()
    }

    fn rows(config: &MiniAppConfig) -> Vec<Item> {
        config
            .home
            .iter()
            .find_map(|block| match block {
                HomeBlock::Sections(items) => Some(items.items.clone()),
                _ => None,
            })
            .unwrap()
    }

    fn actions(config: &MiniAppConfig) -> Vec<Item> {
        config
            .home
            .iter()
            .find_map(|block| match block {
                HomeBlock::Actions(items) => Some(items.items.clone()),
                _ => None,
            })
            .unwrap()
    }

    fn row_names(config: &MiniAppConfig) -> Vec<String> {
        rows(config)
            .into_iter()
            .map(|item| match item.std {
                Some(section) => section.as_str().to_string(),
                None => format!("own:{}", item.label.ru.unwrap_or_default()),
            })
            .collect()
    }

    #[test]
    fn стандарт_повторяет_нынешний_вид_мини_приложения() {
        let standard = MiniAppConfig::standard();
        assert_eq!(
            home(&standard),
            ["banners", "greeting", "subscription", "actions", "sections"]
        );
        assert_eq!(tabs(&standard), ["home", "plans", "connect", "profile"]);
        assert_eq!(
            profile(&standard),
            [
                "person",
                "balance",
                "friends",
                "help",
                "links",
                "documents",
                "account"
            ]
        );
        assert!(standard.header.balance);
        assert!(standard.banners().is_empty());
        assert_eq!(
            row_names(&standard),
            [
                "friends", "roulette", "addons", "gifts", "promo", "payments", "guide", "support"
            ]
        );
        assert_eq!(
            actions(&standard),
            [
                Item::standard(Section::Renew),
                Item::standard(Section::Devices)
            ]
        );
        let off: Vec<HomeKind> = standard
            .home
            .iter()
            .filter(|b| !b.on())
            .map(HomeBlock::kind)
            .collect();
        assert_eq!(off, [HomeKind::Greeting]);
        assert!(standard.hidden().is_empty());
        assert!(standard.friends_in_profile());
        assert_eq!(MiniAppConfig::default(), standard);
        assert_eq!(parse(json!({})).unwrap(), standard);
        assert_eq!(
            parse(json!({ "home": [], "tabs": [], "profile": [] })).unwrap(),
            standard
        );
        assert_eq!(
            parse(json!({ "header": null, "home": null, "tabs": null, "profile": null })).unwrap(),
            standard
        );
        let stored = serde_json::to_value(&standard).unwrap();
        let back: MiniAppConfig = serde_json::from_value(stored).unwrap();
        assert_eq!(back.arranged(), standard);
        let empty: MiniAppConfig = serde_json::from_value(json!({})).unwrap();
        assert!(empty.arranged().is_standard());
    }

    #[test]
    fn блоки_главной_переставляются_а_недостающие_встают_на_свои_места() {
        let config = parse(json!({ "home": [
            { "kind": "sections" },
            { "kind": "subscription", "on": false },
            { "kind": "actions", "on": false }
        ] }))
        .unwrap();
        assert_eq!(
            home(&config),
            ["banners", "greeting", "sections", "subscription", "actions"]
        );
        let on: Vec<bool> = config.home.iter().map(HomeBlock::on).collect();
        assert_eq!(on, [true, false, true, true, false]);
        assert_eq!(config.hidden(), ["actions"]);
        assert_eq!(
            actions(&config),
            actions(&MiniAppConfig::standard()),
            "без списка кнопок остаются стандартные"
        );

        assert_eq!(
            message(parse(json!({ "home": [{ "kind": "video" }] }))),
            "Главная, блок 1: неизвестный блок «video»"
        );
        assert_eq!(
            message(parse(json!({ "home": [{ "on": true }] }))),
            "Главная, блок 1: не указано, что это за блок"
        );
        assert_eq!(
            message(parse(
                json!({ "home": [{ "kind": "greeting" }, { "kind": "greeting" }] })
            )),
            "Главная: блок «Приветствие» указан дважды"
        );
        assert_eq!(
            message(parse(json!({ "home": {} }))),
            "Главная: нужен список блоков"
        );
        assert!(
            message(parse(
                json!({ "home": [{ "kind": "subscription", "look": "huge" }] })
            ))
            .starts_with("Главная, подписка: поля заполнены неверно")
        );
    }

    #[test]
    fn подписку_и_обязательные_вкладки_спрятать_нельзя() {
        let config = parse(json!({ "home": [{ "kind": "subscription", "on": false }] })).unwrap();
        assert!(
            config
                .home
                .iter()
                .find(|b| b.kind() == HomeKind::Subscription)
                .unwrap()
                .on()
        );
        for (tabs, text) in [
            (
                json!([{ "std": "home" }, { "std": "profile" }, { "std": "support" }]),
                "Меню внизу: вкладку «Тарифы» убрать нельзя: без неё клиент не оплатит подписку",
            ),
            (
                json!([{ "std": "home" }, { "std": "plans" }, { "std": "support" }]),
                "Меню внизу: вкладку «Профиль» убрать нельзя: без неё клиент не найдёт поддержку и свой аккаунт",
            ),
            (
                json!([{ "std": "plans" }, { "std": "profile" }, { "std": "support" }]),
                "Меню внизу: вкладку «Главная» убрать нельзя: без неё клиент не увидит подписку",
            ),
        ] {
            assert_eq!(message(parse(json!({ "tabs": tabs }))), text);
        }
        let hidden = parse(json!({ "profile": [
            { "kind": "help", "on": false },
            { "kind": "documents", "on": false },
            { "kind": "account", "on": false }
        ] }))
        .unwrap();
        assert!(
            hidden
                .profile
                .iter()
                .filter(|block| block.kind() != ProfileKind::Person)
                .all(ProfileBlock::on)
        );
    }

    #[test]
    fn вкладок_от_трёх_до_пяти_без_повторов_и_с_короткой_надписью() {
        let config = parse(json!({ "tabs": [
            { "std": "profile" },
            { "std": "plans", "label": { "ru": "  Купить ", "en": "Buy" } },
            { "std": "home", "label": { "ru": "   " } },
            { "std": "friends" },
            { "std": "support" }
        ] }))
        .unwrap();
        assert_eq!(
            tabs(&config),
            ["profile", "plans", "home", "friends", "support"]
        );
        assert_eq!(config.tabs[1].label.ru.as_deref(), Some("Купить"));
        assert_eq!(config.tabs[1].label.en.as_deref(), Some("Buy"));
        assert!(config.tabs[2].label.is_unset());

        assert_eq!(
            message(parse(json!({ "tabs": [
                { "std": "home" }, { "std": "plans" }, { "std": "profile" },
                { "std": "friends" }, { "std": "support" }, { "std": "gifts" }
            ] }))),
            "Меню внизу: не больше 5 вкладок"
        );
        assert_eq!(
            message(parse(
                json!({ "tabs": [{ "std": "home" }, { "std": "home" }] })
            )),
            "Меню внизу, вкладка 2: «Главная» уже есть в меню"
        );
        assert_eq!(
            message(parse(json!({ "tabs": [{ "std": "trial" }] }))),
            "Меню внизу, вкладка 1: раздела «trial» для вкладки нет"
        );
        assert_eq!(
            message(parse(json!({ "tabs": [{ "to": "https://example.com" }] }))),
            "Меню внизу, вкладка 1: раздела «» для вкладки нет"
        );
        assert_eq!(
            message(parse(json!({ "tabs": [
                { "std": "home", "label": { "ru": "Ж".repeat(13) } }
            ] }))),
            "Меню внизу, вкладка 1, надпись: не длиннее 12 символов"
        );
        assert_eq!(
            message(parse(json!({ "tabs": [
                { "std": "home", "label": { "en": "a".repeat(13) } }
            ] }))),
            "Меню внизу, вкладка 1, надпись на английском: не длиннее 12 символов"
        );
        let broken: MiniAppConfig =
            serde_json::from_value(json!({ "tabs": [{ "std": "gifts" }] })).unwrap();
        assert_eq!(
            tabs(&broken.arranged()),
            ["home", "plans", "connect", "profile"]
        );
    }

    #[test]
    fn баннер_с_картинкой_текстом_ссылкой_и_аудиторией() {
        let config = parse(json!({ "home": [{ "kind": "banners", "items": [
            {
                "image": { "tag": "0123456789ABCDEF", "width": 1, "kind": "webp" },
                "image_en": { "tag": "fedcba9876543210" },
                "title": { "ru": " Новый  год ", "en": "New Year" },
                "text": { "ru": "Скидка\r\n\r\n\r\nдо 31 декабря" },
                "to": " plans ",
                "for": ["ended", "new", "new"]
            },
            { "title": { "ru": "Канал" }, "to": "T.me/quiet_vpn", "for": ["new", "trial", "active", "ended"], "on": false },
            { "title": { "ru": "Без ссылки" }, "to": "" }
        ] }] }))
        .unwrap();
        let banners = config.banners();
        assert_eq!(banners.len(), 3);
        assert_eq!(banners[0].image.as_ref().unwrap().tag, "0123456789abcdef");
        assert_eq!(banners[0].image.as_ref().unwrap().width, 0);
        assert_eq!(
            banners[0].image_en.as_ref().unwrap().tag,
            "fedcba9876543210"
        );
        assert_eq!(banners[0].title.ru.as_deref(), Some("Новый год"));
        assert_eq!(
            banners[0].text.ru.as_deref(),
            Some("Скидка\n\nдо 31 декабря")
        );
        assert_eq!(banners[0].to.as_deref(), Some("plans"));
        assert_eq!(banners[0].audience, [Audience::New, Audience::Ended]);
        assert_eq!(banners[1].to.as_deref(), Some("https://t.me/quiet_vpn"));
        assert!(banners[1].audience.is_empty());
        assert!(!banners[1].on);
        assert_eq!(banners[2].to, None);
        assert_eq!(
            config.image_tags().into_iter().collect::<Vec<_>>(),
            ["0123456789abcdef", "fedcba9876543210"]
        );

        for (banner, text) in [
            (
                json!({ "text": { "ru": "Без заголовка" } }),
                "Главная, баннер 1: нужна картинка или заголовок",
            ),
            (
                json!({ "title": { "en": "Only English" } }),
                "Главная, баннер 1: нужна картинка или заголовок",
            ),
            (
                json!({ "title": { "ru": "Ж".repeat(81) } }),
                "Главная, баннер 1, заголовок: не длиннее 80 символов",
            ),
            (
                json!({ "title": { "ru": "Скидка" }, "text": { "en": "a".repeat(201) } }),
                "Главная, баннер 1, текст на английском: не длиннее 200 символов",
            ),
            (
                json!({ "image": { "tag": "../../etc/passwd" } }),
                "Главная, баннер 1, картинка: указана неверно, загрузите её заново",
            ),
            (
                json!({ "title": { "ru": "Скидка" }, "to": "javascript:alert(1)" }),
                "Главная, баннер 1, ссылка: нужна ссылка вида https://… или t.me/…",
            ),
            (
                json!({ "title": { "ru": "Скидка" }, "to": "http://example.com" }),
                "Главная, баннер 1, ссылка: нужна ссылка вида https://… или t.me/…",
            ),
        ] {
            assert_eq!(
                message(parse(
                    json!({ "home": [{ "kind": "banners", "items": [banner] }] })
                )),
                text
            );
        }
        let many: Vec<Value> = (0..6)
            .map(|_| json!({ "title": { "ru": "Скидка" } }))
            .collect();
        assert_eq!(
            message(parse(
                json!({ "home": [{ "kind": "banners", "items": many }] })
            )),
            "Главная, баннеры: не больше 5"
        );
    }

    #[test]
    fn ссылки_только_https_и_разделы() {
        for (raw, clean) in [
            ("plans", "plans"),
            ("support", "support"),
            ("https://example.com/a?b=1#c", "https://example.com/a?b=1#c"),
            ("HTTPS://Example.com", "https://Example.com"),
            ("https://пример.рф/скидка", "https://пример.рф/скидка"),
            ("https://example.com:8443/x", "https://example.com:8443/x"),
            ("t.me/channel", "https://t.me/channel"),
            ("https://T.ME/channel", "https://t.me/channel"),
        ] {
            assert_eq!(target(raw).as_deref(), Ok(clean), "{raw}");
        }
        for raw in [
            "",
            "   ",
            "http://example.com",
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "tg://resolve?domain=x",
            "vbscript:x",
            "//example.com",
            "/app/plans",
            "../admin",
            "https://",
            "https://.example.com",
            "https://example.com-",
            "https://user@example.com",
            "https://user:pass@example.com",
            "https://exa mple.com",
            "https://example.com/\"><script>",
            "https://example.com/'x",
            "https://example.com\\@evil.example",
            "https://example.com:port",
            "https://example.com:999999",
            "t.me/",
            "home",
            "renew",
            "devices",
            "guide",
            "неизвестно",
        ] {
            assert!(target(raw).is_err(), "{raw}");
        }
        let long = format!("https://example.com/{}", "a".repeat(URL_MAX));
        assert_eq!(
            target(&long).unwrap_err(),
            format!("не длиннее {URL_MAX} символов")
        );
    }

    #[test]
    fn разделы_главной_прячутся_переименовываются_и_дополняются_своими_ссылками() {
        let config = parse(json!({ "home": [{ "kind": "sections", "items": [
            { "std": "support", "label": { "ru": "Написать нам", "en": "Write to us" } },
            { "label": { "ru": "Новости" }, "to": "t.me/news", "icon": "send", "for": ["active"] },
            { "std": "gifts", "on": false },
            { "std": "friends" }
        ] }] }))
        .unwrap();
        assert_eq!(
            row_names(&config),
            [
                "support",
                "own:Новости",
                "gifts",
                "promo",
                "payments",
                "guide",
                "friends",
                "roulette",
                "addons"
            ]
        );
        let items = rows(&config);
        assert!(!items[2].on);
        assert_eq!(items[0].label.en.as_deref(), Some("Write to us"));
        assert_eq!(items[1].to.as_deref(), Some("https://t.me/news"));
        assert_eq!(items[1].icon, Some(Glyph::Send));
        assert_eq!(items[1].audience, [Audience::Active]);

        for (item, text) in [
            (
                json!({ "std": "renew" }),
                "Главная, разделы, «Продлить»: раздела «Продлить» здесь нет",
            ),
            (
                json!({ "std": "friends", "to": "https://example.com" }),
                "Главная, разделы, «Друзья»: у раздела не бывает своей ссылки и значка",
            ),
            (
                json!({ "to": "https://example.com" }),
                "Главная, разделы, пункт 1, название: нужно заполнить",
            ),
            (
                json!({ "label": { "en": "Only English" }, "to": "https://example.com" }),
                "Главная, разделы, пункт 1, название: нужно заполнить",
            ),
            (
                json!({ "label": { "ru": "Ссылка" } }),
                "Главная, разделы, пункт 1, ссылка: укажите, куда ведёт",
            ),
            (
                json!({ "label": { "ru": "Ж".repeat(41) }, "to": "plans" }),
                "Главная, разделы, пункт 1, название: не длиннее 40 символов",
            ),
        ] {
            assert_eq!(
                message(parse(
                    json!({ "home": [{ "kind": "sections", "items": [item] }] })
                )),
                text
            );
        }
        assert_eq!(
            message(parse(json!({ "home": [{ "kind": "sections", "items": [
                { "std": "promo" }, { "std": "promo" }
            ] }] }))),
            "Главная, разделы, «Промокод»: указан дважды"
        );
        let many: Vec<Value> = (0..9)
            .map(|_| json!({ "label": { "ru": "Ссылка" }, "to": "plans" }))
            .collect();
        assert_eq!(
            message(parse(
                json!({ "home": [{ "kind": "sections", "items": many }] })
            )),
            "Главная, разделы: не больше 8 своих ссылок"
        );
        assert!(
            message(parse(json!({ "home": [{ "kind": "sections", "items": [
                { "label": { "ru": "Ссылка" }, "to": "plans", "icon": "<svg>" }
            ] }] })))
            .starts_with("Главная, разделы: поля заполнены неверно")
        );
    }

    #[test]
    fn кнопок_под_подпиской_до_четырёх_и_пустой_список_допустим() {
        let config = parse(json!({ "home": [{ "kind": "actions", "items": [
            { "std": "support" },
            { "label": { "ru": "Канал" }, "to": "t.me/news" },
            { "std": "renew", "label": { "ru": "Оплатить" }, "on": false }
        ] }] }))
        .unwrap();
        let items = actions(&config);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Item::standard(Section::Support));
        assert_eq!(items[2].label.ru.as_deref(), Some("Оплатить"));
        assert!(items.iter().all(|item| item.on));
        let none = parse(json!({ "home": [{ "kind": "actions", "items": [] }] })).unwrap();
        assert!(actions(&none).is_empty());
        let stored = serde_json::to_value(&none).unwrap();
        let back: MiniAppConfig = serde_json::from_value(stored).unwrap();
        assert!(actions(&back.arranged()).is_empty());

        assert_eq!(
            message(parse(json!({ "home": [{ "kind": "actions", "items": [
                { "std": "renew" }, { "std": "devices" }, { "std": "gifts" },
                { "std": "promo" }, { "std": "support" }
            ] }] }))),
            "Главная, кнопки под подпиской: не больше 4 кнопок"
        );
        assert_eq!(
            message(parse(json!({ "home": [{ "kind": "actions", "items": [
                { "std": "renew" }, { "std": "renew" }
            ] }] }))),
            "Главная, кнопка 2: «Продлить» уже есть"
        );
        assert_eq!(
            message(parse(json!({ "home": [{ "kind": "actions", "items": [
                { "std": "home" }
            ] }] }))),
            "Главная, кнопка 1: раздела «Главная» здесь нет"
        );
        assert_eq!(
            message(parse(json!({ "home": [{ "kind": "actions", "items": [
                { "std": "renew", "label": { "ru": "Ж".repeat(25) } }
            ] }] }))),
            "Главная, кнопка 1, название: не длиннее 24 символов"
        );
    }

    #[test]
    fn приветствие_и_тексты_без_подписки() {
        let config = parse(json!({ "home": [
            { "kind": "greeting", "on": true, "title": { "ru": "Привет, {name}!" }, "for": ["new"] },
            { "kind": "subscription", "look": "compact", "empty": {
                "title": { "ru": "Начните с тарифа" },
                "line": { "ru": "", "en": " " },
                "button": { "ru": "Купить" }
            } }
        ] }))
        .unwrap();
        let HomeBlock::Greeting(greeting) = &config.home[1] else {
            unreachable!()
        };
        assert!(greeting.on);
        assert_eq!(greeting.audience, [Audience::New]);
        let HomeBlock::Subscription(sub) = &config.home[2] else {
            unreachable!()
        };
        assert_eq!(sub.look, Look::Compact);
        assert_eq!(sub.empty.line.ru.as_deref(), Some(""));
        assert_eq!(sub.empty.line.en.as_deref(), Some(""));
        assert_eq!(sub.empty.button.ru.as_deref(), Some("Купить"));

        assert_eq!(
            message(parse(
                json!({ "home": [{ "kind": "greeting", "on": true }] })
            )),
            "Главная, приветствие: нужен заголовок или текст"
        );
        assert!(
            parse(json!({ "home": [{ "kind": "greeting", "on": false }] }))
                .unwrap()
                .is_standard()
        );
        assert_eq!(
            message(parse(
                json!({ "home": [{ "kind": "greeting", "text": { "ru": "Ж".repeat(301) } }] })
            )),
            "Главная, приветствие, текст: не длиннее 300 символов"
        );
        assert_eq!(
            message(parse(
                json!({ "home": [{ "kind": "subscription", "empty": { "button": { "en": "a".repeat(25) } } }] })
            )),
            "Главная, подписка, без подписки, кнопка на английском: не длиннее 24 символов"
        );
    }

    #[test]
    fn профиль_переставляется_а_друзья_уходят_на_свой_экран() {
        let config = parse(json!({ "profile": [
            { "kind": "account" },
            { "kind": "friends", "place": "screen", "title": { "ru": "Приглашайте" }, "text": { "ru": "" } },
            { "kind": "balance", "on": false },
            { "kind": "links", "items": [
                { "label": { "ru": "Канал", "en": "Channel" }, "to": "t.me/news", "icon": "send" }
            ] }
        ] }))
        .unwrap();
        assert_eq!(
            profile(&config),
            [
                "person",
                "account",
                "friends",
                "help",
                "balance",
                "links",
                "documents"
            ]
        );
        assert_eq!(config.hidden(), ["balance"]);
        assert!(!config.friends_in_profile());
        assert_eq!(config.friends().place, FriendsPlace::Screen);
        assert_eq!(config.friends().text.ru.as_deref(), Some(""));
        let off = parse(json!({ "profile": [{ "kind": "friends", "on": false }] })).unwrap();
        assert!(!off.friends_in_profile());

        assert_eq!(
            message(parse(json!({ "profile": [{ "kind": "links", "items": [
                { "std": "promo" }
            ] }] }))),
            "Профиль, свои ссылки, ссылка 1: здесь только свои ссылки"
        );
        assert_eq!(
            message(parse(json!({ "profile": [{ "kind": "cabinet" }] }))),
            "Профиль, блок 1: неизвестный блок «cabinet»"
        );
        assert_eq!(
            message(parse(
                json!({ "profile": [{ "kind": "friends", "title": { "ru": "Ж".repeat(41) } }] })
            )),
            "Профиль, друзья, заголовок: не длиннее 40 символов"
        );
    }

    #[test]
    fn адреса_картинок_добавляются_только_для_показа() {
        let mut config = parse(json!({ "home": [{ "kind": "banners", "items": [
            { "image": { "tag": "0123456789abcdef" } }
        ] }] }))
        .unwrap();
        for (_, image) in config.images_mut() {
            image.kind = Some(ImageKind::Webp);
            image.width = 1432;
            image.height = 536;
        }
        assert_eq!(
            config.images_mut()[0].0,
            "Главная, баннер 1, картинка".to_string()
        );
        let shown = config.with_urls();
        assert_eq!(
            shown.images()[0].url.as_deref(),
            Some("/api/public/miniapp/0123456789abcdef.webp")
        );
        assert_eq!(config.images()[0].url, None);
    }

    #[test]
    fn скрытые_тексты_и_ссылки_клиентам_не_отдаются() {
        let config = parse(json!({
            "home": [
                { "kind": "banners", "items": [
                    { "title": { "ru": "Видимый" } },
                    { "title": { "ru": "Чёрная пятница" }, "on": false }
                ] },
                { "kind": "greeting", "on": false, "title": { "ru": "Черновик приветствия" } },
                { "kind": "actions", "on": false, "items": [
                    { "label": { "ru": "Черновик кнопки" }, "to": "plans" }
                ] },
                { "kind": "sections", "items": [
                    { "std": "gifts", "on": false },
                    { "label": { "ru": "Черновик ссылки" }, "to": "plans", "on": false },
                    { "label": { "ru": "Канал" }, "to": "t.me/news" }
                ] }
            ],
            "profile": [{ "kind": "links", "on": false, "items": [
                { "label": { "ru": "Черновик профиля" }, "to": "plans" }
            ] }]
        }))
        .unwrap();
        let text = serde_json::to_string(&config.published()).unwrap();
        assert!(!text.contains("Черновик"), "{text}");
        assert!(!text.contains("пятница"), "{text}");
        assert!(text.contains("Видимый"), "{text}");
        assert!(text.contains("https://t.me/news"), "{text}");
        let shown = config.published();
        let names = row_names(&shown);
        assert_eq!(names.len(), 9);
        assert!(names.contains(&"own:Канал".to_string()));
        assert!(
            !rows(&shown)
                .iter()
                .find(|item| item.std == Some(Section::Gifts))
                .unwrap()
                .on
        );
        let off = parse(json!({ "home": [
            { "kind": "banners", "on": false, "items": [{ "title": { "ru": "Скрытый блок" } }] },
            { "kind": "sections", "on": false, "items": [
                { "label": { "ru": "Скрытая ссылка" }, "to": "plans" }
            ] }
        ] }))
        .unwrap();
        let text = serde_json::to_string(&off.published()).unwrap();
        assert!(!text.contains("Скрыт"), "{text}");
        assert_eq!(
            MiniAppConfig::standard().published(),
            MiniAppConfig::standard()
        );
    }

    fn stored(value: Value) -> MiniAppConfig {
        MiniAppConfig::stored(value).unwrap()
    }

    fn banners(config: &MiniAppConfig) -> Vec<Banner> {
        config.banners().to_vec()
    }

    #[test]
    fn аватар_в_шапке_выключен_пока_его_не_включили() {
        let standard = MiniAppConfig::standard();
        assert!(!standard.header.avatar);
        assert!(
            !stored(json!({ "header": { "balance": false } }))
                .header
                .avatar
        );
        let on = parse(json!({ "header": { "avatar": true } })).unwrap();
        assert!(on.header.avatar && on.header.balance);
        assert_eq!(
            serde_json::to_value(&on).unwrap()["header"],
            json!({ "avatar": true })
        );
        assert_eq!(stored(serde_json::to_value(&on).unwrap()), on);
        let off = parse(json!({ "header": { "avatar": false, "balance": true } })).unwrap();
        assert_eq!(off, standard);
        assert!(parse(json!({ "header": { "avatar": "да" } })).is_err());
    }

    #[test]
    fn фото_с_именем_и_подарок_в_профиле_выключены_пока_их_не_включили() {
        let blocks = |config: &MiniAppConfig| {
            config
                .profile
                .iter()
                .map(|block| serde_json::to_value(block).unwrap())
                .collect::<Vec<_>>()
        };
        let standard = MiniAppConfig::standard();
        assert_eq!(
            blocks(&standard)[..4],
            [
                json!({ "kind": "person" }),
                json!({ "kind": "balance" }),
                json!({ "kind": "friends" }),
                json!({ "kind": "help" }),
            ]
        );
        assert!(!standard.profile[0].on());
        assert!(standard.hidden().is_empty());
        let old = stored(json!({ "profile": [{ "kind": "account" }, { "kind": "help" }] }));
        assert_eq!(blocks(&old)[0], json!({ "kind": "person" }));
        assert!(!old.profile[0].on());
        assert_eq!(
            profile(&old)
                .into_iter()
                .filter(|kind| ["account", "help"].contains(kind))
                .collect::<Vec<_>>(),
            ["account", "help"]
        );
        let on = parse(json!({ "profile": [
            { "kind": "help", "gifts": true },
            { "kind": "person", "on": true }
        ] }))
        .unwrap();
        assert!(blocks(&on).contains(&json!({ "kind": "help", "gifts": true })));
        assert!(blocks(&on).contains(&json!({ "kind": "person", "on": true })));
        assert_eq!(stored(serde_json::to_value(&on).unwrap()), on);
        assert_eq!(on.published().profile, on.profile);
        let off = parse(json!({ "profile": [
            { "kind": "person", "on": false },
            { "kind": "help", "gifts": false }
        ] }))
        .unwrap();
        assert!(blocks(&off).contains(&json!({ "kind": "person" })));
        assert!(blocks(&off).contains(&json!({ "kind": "help" })));
        assert!(parse(json!({ "profile": [{ "kind": "person", "on": "да" }] })).is_err());
        assert!(parse(json!({ "profile": [{ "kind": "help", "gifts": 1 }] })).is_err());
    }

    #[test]
    fn сохранённый_вид_читается_без_частей_которых_в_этой_версии_нет() {
        let config = stored(json!({
            "header": { "balance": false, "wallet": "big" },
            "home": [
                { "kind": "stories", "items": [{ "title": { "ru": "История" } }] },
                { "kind": "sections", "items": [
                    { "std": "club", "label": { "ru": "Клуб" } },
                    { "std": "support", "label": { "ru": "Помощь" } },
                    { "label": { "ru": "Канал" }, "to": "https://t.me/news", "icon": "rocket" },
                    { "label": { "ru": "В клуб" }, "to": "club" },
                    { "label": { "ru": "Своим" }, "to": "plans", "for": ["vip"] },
                    { "label": { "ru": "Новым" }, "to": "plans", "for": ["vip", "new"] }
                ] },
                { "kind": "banners", "items": [
                    { "title": { "ru": "Скидка" }, "to": "club" },
                    { "title": 5 },
                    { "title": { "ru": "Своим" }, "to": "plans", "for": ["vip"] }
                ] },
                { "kind": "greeting", "on": true, "title": { "ru": "Привет" }, "for": ["vip"] },
                { "kind": "subscription", "look": "wide", "empty": { "button": { "ru": "Купить" } } },
                { "kind": "actions", "items": [{ "std": "club" }, { "std": "plans" }, { "std": "gifts" }] }
            ],
            "tabs": [
                { "std": "home" },
                { "std": "club" },
                { "std": "plans", "label": { "ru": "Купить" } },
                { "std": "support" },
                { "std": "profile" }
            ],
            "profile": [
                { "kind": "account" },
                { "kind": "awards" },
                { "kind": "friends", "place": "popup", "title": { "ru": "Зовите друзей" } },
                { "kind": "links", "items": [
                    { "std": "club" },
                    { "std": "plans" },
                    { "label": { "ru": "Сайт" }, "to": "https://example.com" }
                ] }
            ]
        }));
        assert!(!config.header.balance);
        assert_eq!(
            home(&config),
            ["sections", "banners", "greeting", "subscription", "actions"]
        );
        assert_eq!(
            row_names(&config),
            [
                "friends",
                "roulette",
                "addons",
                "gifts",
                "promo",
                "payments",
                "guide",
                "support",
                "own:Канал",
                "own:Своим",
                "own:Новым"
            ]
        );
        let rows = rows(&config);
        let row = |name: &str| {
            rows.iter()
                .find(|item| item.label.ru.as_deref() == Some(name))
                .unwrap()
        };
        assert_eq!(row("Помощь").std, Some(Section::Support));
        assert_eq!(row("Канал").icon, None);
        assert!(row("Канал").on);
        assert!(!row("Своим").on);
        assert!(row("Новым").on);
        assert_eq!(row("Новым").audience, [Audience::New]);
        let banners = banners(&config);
        assert_eq!(banners.len(), 2);
        assert_eq!(banners[0].title.ru.as_deref(), Some("Скидка"));
        assert_eq!(banners[0].to, None);
        assert!(banners[0].on);
        assert!(!banners[1].on);
        assert!(config.home.iter().any(|block| matches!(
            block,
            HomeBlock::Greeting(greeting) if !greeting.on && greeting.title.ru.as_deref() == Some("Привет")
        )));
        assert!(config.home.iter().any(|block| matches!(
            block,
            HomeBlock::Subscription(sub)
                if sub.look == Look::Ring && sub.empty.button.ru.as_deref() == Some("Купить")
        )));
        assert_eq!(actions(&config), [Item::standard(Section::Gifts)]);
        assert_eq!(tabs(&config), ["home", "plans", "support", "profile"]);
        assert_eq!(config.tabs[1].label.ru.as_deref(), Some("Купить"));
        assert_eq!(
            profile(&config),
            [
                "person",
                "balance",
                "account",
                "friends",
                "help",
                "links",
                "documents"
            ]
        );
        assert!(config.profile.iter().any(|block| matches!(
            block,
            ProfileBlock::Friends(friends)
                if friends.place == FriendsPlace::Card
                    && friends.title.ru.as_deref() == Some("Зовите друзей")
        )));
        assert!(config.profile.iter().any(|block| matches!(
            block,
            ProfileBlock::Links(links)
                if links.items.len() == 1 && links.items[0].label.ru.as_deref() == Some("Сайт")
        )));
        assert_eq!(parse(serde_json::to_value(&config).unwrap()), Ok(config));
    }

    #[test]
    fn обычный_вид_читается_из_базы_целиком() {
        let config = parse(json!({
            "header": { "balance": false },
            "home": [
                { "kind": "greeting", "on": true, "title": { "ru": "Привет, {name}!", "en": "Hi, {name}!" },
                  "text": { "ru": "" }, "for": ["new", "ended"] },
                { "kind": "banners", "items": [
                    { "image": { "tag": "0123456789abcdef", "kind": "png", "width": 1200, "height": 400 },
                      "title": { "ru": "Скидка" }, "to": "plans", "for": ["active"] },
                    { "title": { "ru": "Канал" }, "to": "t.me/news", "on": false }
                ] },
                { "kind": "subscription", "look": "compact", "empty": { "title": { "ru": "Пока пусто" } } },
                { "kind": "actions", "items": [
                    { "std": "support", "label": { "ru": "Помощь" } },
                    { "label": { "ru": "Статус" }, "to": "https://example.com/status", "icon": "flag" }
                ] },
                { "kind": "sections", "items": [
                    { "std": "gifts", "on": false },
                    { "label": { "ru": "Сайт" }, "to": "https://example.com", "icon": "globe", "for": ["trial"] }
                ] }
            ],
            "tabs": [
                { "std": "profile" }, { "std": "home", "label": { "ru": "Дом" } },
                { "std": "friends" }, { "std": "plans" }
            ],
            "profile": [
                { "kind": "links", "items": [{ "label": { "ru": "Сайт" }, "to": "https://example.com" }] },
                { "kind": "friends", "place": "screen", "text": { "ru": "" } },
                { "kind": "balance", "on": false }
            ]
        }))
        .unwrap();
        assert_eq!(stored(serde_json::to_value(&config).unwrap()), config);
        let standard = MiniAppConfig::standard();
        assert_eq!(stored(serde_json::to_value(&standard).unwrap()), standard);
        assert_eq!(stored(json!({})), standard);
        assert_eq!(
            stored(json!({ "header": 7, "home": 5, "tabs": "нет", "profile": [null, "мусор"] })),
            standard
        );
        for broken in [json!(null), json!([]), json!("мусор"), json!(5)] {
            assert_eq!(MiniAppConfig::stored(broken), None);
        }
    }

    #[test]
    fn опасные_адреса_из_базы_не_читаются() {
        for to in [
            "javascript:alert(1)",
            "http://example.com",
            "https://user:pass@example.com",
            "https://example.com/\"><script>",
            "renew",
        ] {
            let config = stored(json!({
                "home": [
                    { "kind": "banners", "items": [{ "title": { "ru": "Баннер" }, "to": to }] },
                    { "kind": "actions", "items": [{ "label": { "ru": "Кнопка" }, "to": to }] },
                    { "kind": "sections", "items": [{ "label": { "ru": "Пункт" }, "to": to }] }
                ],
                "profile": [{ "kind": "links", "items": [{ "label": { "ru": "Ссылка" }, "to": to }] }]
            }));
            assert_eq!(banners(&config).len(), 1, "{to}");
            assert_eq!(banners(&config)[0].to, None, "{to}");
            assert!(actions(&config).is_empty(), "{to}");
            assert_eq!(row_names(&config).len(), Section::ROWS.len(), "{to}");
            let text = serde_json::to_string(&config).unwrap();
            assert!(!text.contains("Ссылка"), "{to}");
        }
    }

    #[test]
    fn css_хранится_рядом_с_видом_и_старую_настройку_не_меняет() {
        use crate::miniapp_css::CssPlace;
        let plain = parse(json!({ "header": { "balance": false } })).unwrap();
        assert!(
            serde_json::to_value(&plain).unwrap().get("css").is_none(),
            "пустой CSS не пишется"
        );
        let old = stored(json!({ "header": { "balance": false } }));
        assert_eq!(old, plain);
        assert!(old.css.is_empty());

        let mut config = MiniAppConfig::standard();
        config.css = MiniCss::parse(
            &[("page".to_string(), Some(".mn { gap: 0 }".to_string()))]
                .into_iter()
                .collect(),
        )
        .unwrap();
        assert!(!config.is_standard());
        assert_eq!(
            config.difference(&MiniAppConfig::standard()),
            Difference::default()
        );
        let raw = serde_json::to_value(&config).unwrap();
        assert_eq!(raw["css"], json!({ "page": ".mn { gap: 0 }" }));
        assert_eq!(stored(raw), config);
        assert!(config.published().css.is_empty());
        assert!(
            serde_json::to_value(config.published())
                .unwrap()
                .get("css")
                .is_none()
        );

        let loose = stored(json!({
            "css": { "page": ".mn {}", "stories": "x", "tabs": null, "profile": "" }
        }));
        assert_eq!(
            loose
                .css
                .places()
                .map(|(place, _)| place)
                .collect::<Vec<_>>(),
            [CssPlace::Page]
        );
        for broken in [json!("x"), json!(["x"]), json!({ "page": 5 }), json!(null)] {
            let config = stored(json!({ "css": broken, "header": { "balance": false } }));
            assert_eq!(config, plain);
        }
    }

    #[test]
    fn списки_сервера_и_мини_приложения_совпадают() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/src/mini/config.ts");
        let source = std::fs::read_to_string(path).unwrap();
        let body = |start: &str, end: &str| {
            let at = source
                .find(start)
                .unwrap_or_else(|| panic!("нет «{start}»"))
                + start.len();
            let rest = &source[at..];
            rest[..rest.find(end).unwrap()].to_string()
        };
        let list = |name: &str| -> Vec<String> {
            let declared = format!("export const {name}:");
            let body = body(&declared, "];");
            let values = body.split_once("= [").unwrap().1;
            values
                .split(',')
                .map(|value| value.trim().trim_matches('\'').to_string())
                .collect()
        };
        let sections = |list: &[Section]| -> Vec<String> {
            list.iter().map(|s| s.as_str().to_string()).collect()
        };
        let named = |value: Value| value.as_str().unwrap().to_string();
        assert_eq!(list("ROW_KEYS"), sections(&Section::ROWS));
        assert_eq!(list("ACTION_KEYS"), sections(&Section::ACTIONS));
        assert_eq!(list("TAB_KEYS"), sections(&Section::TABS));
        assert_eq!(list("TARGET_KEYS"), sections(&Section::TARGETS));
        assert_eq!(list("ALWAYS_TABS"), sections(&Section::ALWAYS_TABS));
        assert_eq!(
            list("STANDARD_TABS"),
            sections(&standard_tabs().iter().map(|t| t.std).collect::<Vec<_>>())
        );
        assert_eq!(
            list("STANDARD_ACTIONS"),
            sections(
                &standard_actions()
                    .iter()
                    .filter_map(|i| i.std)
                    .collect::<Vec<_>>()
            )
        );
        assert_eq!(list("HOME_KINDS"), HomeKind::ALL.map(HomeKind::as_str));
        assert_eq!(
            list("PROFILE_KINDS"),
            ProfileKind::ALL.map(ProfileKind::as_str)
        );
        assert_eq!(list("AUDIENCES"), Audience::ALL.map(|a| named(json!(a))));
        assert_eq!(list("GLYPHS"), Glyph::ALL.map(|g| named(json!(g))));
        let icons = body(
            "export const SECTION_ICONS: Record<SectionKey, IconName> = {",
            "};",
        );
        let keys: Vec<&str> = icons
            .lines()
            .filter_map(|line| line.trim().split_once(':').map(|(key, _)| key))
            .collect();
        assert_eq!(keys, Section::ALL.map(Section::as_str));
        assert!(source.contains(&format!("export const TABS_MIN = {TABS_MIN};")));
        assert!(source.contains(&format!("export const TABS_MAX = {TABS_MAX};")));
    }

    #[test]
    fn отличия_называют_экраны_и_блоки() {
        let standard = MiniAppConfig::standard();
        assert_eq!(standard.difference(&standard), Difference::default());
        let next = parse(json!({
            "header": { "balance": false },
            "home": [
                { "kind": "sections" },
                { "kind": "greeting", "on": true, "title": { "ru": "Привет" } }
            ],
            "tabs": [{ "std": "home" }, { "std": "plans" }, { "std": "profile" }],
            "profile": [{ "kind": "balance", "on": false }]
        }))
        .unwrap();
        let diff = standard.difference(&next);
        assert_eq!(diff.screens, ["home", "tabs", "profile"]);
        assert_eq!(diff.blocks, ["header", "greeting", "balance"]);
        assert!(diff.order);
        let only = parse(json!({ "profile": [{ "kind": "balance", "on": false }] })).unwrap();
        let diff = standard.difference(&only);
        assert_eq!(diff.screens, ["profile"]);
        assert_eq!(diff.blocks, ["balance"]);
        assert!(!diff.order);
    }

    #[test]
    fn разметка_в_текстах_остаётся_текстом() {
        let config = parse(json!({ "home": [{ "kind": "greeting", "on": true,
            "title": { "ru": "<img src=x onerror=alert(1)>" },
            "text": { "ru": "строка\u{0000}\u{0007}\nвторая" }
        }] }))
        .unwrap();
        let HomeBlock::Greeting(greeting) = &config.home[1] else {
            unreachable!()
        };
        assert_eq!(
            greeting.title.ru.as_deref(),
            Some("<img src=x onerror=alert(1)>")
        );
        assert_eq!(greeting.text.ru.as_deref(), Some("строка\nвторая"));
    }
}
