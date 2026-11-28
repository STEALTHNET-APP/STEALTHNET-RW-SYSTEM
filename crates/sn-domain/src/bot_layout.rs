use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ROW_MAX: usize = 3;

pub type Rows = Vec<Vec<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    Menu,
    Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    All,
    Subscribed,
    Unsubscribed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Slot {
    pub key: &'static str,
    pub audience: Audience,
    pub when: &'static str,
}

const fn slot(key: &'static str, audience: Audience, when: &'static str) -> Slot {
    Slot {
        key,
        audience,
        when,
    }
}

pub static MENU: [Slot; 17] = [
    slot(
        "menu.btn.app",
        Audience::All,
        "Если включено мини-приложение и у сайта адрес с https",
    ),
    slot(
        "menu.btn.subscription",
        Audience::Subscribed,
        "Клиентам с подпиской",
    ),
    slot(
        "menu.btn.trial",
        Audience::Unsubscribed,
        "Клиентам без подписки, пока они не брали пробный период",
    ),
    slot(
        "menu.btn.buy",
        Audience::Unsubscribed,
        "Клиентам без подписки",
    ),
    slot(
        "menu.btn.extend",
        Audience::Subscribed,
        "Клиентам с подпиской, действующей или закончившейся",
    ),
    slot(
        "menu.btn.balance",
        Audience::All,
        "Если включено пополнение баланса или у клиента на балансе есть деньги",
    ),
    slot(
        "menu.btn.promo",
        Audience::All,
        "Если включён модуль «Промокоды»",
    ),
    slot(
        "menu.btn.options",
        Audience::All,
        "Если включён модуль «Доп. опции»",
    ),
    slot(
        "menu.btn.gifts",
        Audience::All,
        "Если включён модуль «Подарочные подписки»",
    ),
    slot(
        "menu.btn.referral",
        Audience::All,
        "Если включён модуль «Рефералы»",
    ),
    slot("menu.btn.contests", Audience::All, "Пока идёт конкурс"),
    slot(
        "menu.btn.roulette",
        Audience::All,
        "Если включены модуль «Рулетка» и мини-приложение",
    ),
    slot(
        "menu.btn.modules",
        Audience::All,
        "Если у включённого внешнего модуля есть своя кнопка в боте",
    ),
    slot(
        "menu.btn.documents",
        Audience::All,
        "Если добавлены документы",
    ),
    slot("menu.btn.support", Audience::All, "Всегда"),
    slot(
        "menu.btn.email",
        Audience::All,
        "Если настроена «Почта» и открыт вход в кабинет на сайте",
    ),
    slot(
        "menu.btn.language",
        Audience::All,
        "Если включён английский язык",
    ),
];

pub static SUBSCRIPTION: [Slot; 8] = [
    slot(
        "sub.btn.connect",
        Audience::All,
        "Если у подписки есть ссылка",
    ),
    slot(
        "sub.btn.copy_link",
        Audience::All,
        "Если у подписки есть ссылка",
    ),
    slot(
        "sub.btn.devices",
        Audience::All,
        "Если подписка связана с Remnawave",
    ),
    slot(
        "sub.btn.reissue",
        Audience::All,
        "Если подписка связана с Remnawave и включено «Выпускать новую ссылку подписки»",
    ),
    slot("sub.btn.extend", Audience::All, "Всегда"),
    slot(
        "sub.btn.renewal_off",
        Audience::All,
        "Если подписку продлевает платёжная система, например Lava.top",
    ),
    slot("sub.btn.all", Audience::All, "Всегда"),
    slot("common.btn.menu", Audience::All, "Всегда"),
];

impl Screen {
    pub const ALL: [Screen; 2] = [Screen::Menu, Screen::Subscription];

    pub fn as_str(self) -> &'static str {
        match self {
            Screen::Menu => "menu",
            Screen::Subscription => "subscription",
        }
    }

    pub fn parse(raw: &str) -> Option<Screen> {
        Screen::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub fn title(self) -> &'static str {
        match self {
            Screen::Menu => "Главное меню",
            Screen::Subscription => "Подписка",
        }
    }

    pub fn slots(self) -> &'static [Slot] {
        match self {
            Screen::Menu => &MENU,
            Screen::Subscription => &SUBSCRIPTION,
        }
    }

    pub fn slot(self, key: &str) -> Option<&'static Slot> {
        self.slots().iter().find(|s| s.key == key)
    }

    pub fn standard(self, columns: usize) -> Rows {
        match self {
            Screen::Menu => MENU
                .chunks(columns.clamp(1, ROW_MAX))
                .map(|chunk| chunk.iter().map(|s| s.key.to_string()).collect())
                .collect(),
            Screen::Subscription => vec![
                vec!["sub.btn.connect".into()],
                vec!["sub.btn.copy_link".into()],
                vec!["sub.btn.devices".into()],
                vec!["sub.btn.reissue".into()],
                vec!["sub.btn.extend".into()],
                vec!["sub.btn.renewal_off".into()],
                vec!["sub.btn.all".into(), "common.btn.menu".into()],
            ],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuFacts {
    pub miniapp: bool,
    pub subscribed: bool,
    pub trial: bool,
    pub balance: bool,
    pub promo: bool,
    pub options: bool,
    pub gifts: bool,
    pub referral: bool,
    pub contests: bool,
    pub roulette: bool,
    pub modules: bool,
    pub documents: bool,
    pub email: bool,
    pub language: bool,
}

impl MenuFacts {
    pub fn shows(&self, key: &str) -> bool {
        match key {
            "menu.btn.app" => self.miniapp,
            "menu.btn.subscription" | "menu.btn.extend" => self.subscribed,
            "menu.btn.trial" => !self.subscribed && self.trial,
            "menu.btn.buy" => !self.subscribed,
            "menu.btn.balance" => self.balance,
            "menu.btn.promo" => self.promo,
            "menu.btn.options" => self.options,
            "menu.btn.gifts" => self.gifts,
            "menu.btn.referral" => self.referral,
            "menu.btn.contests" => self.contests,
            "menu.btn.roulette" => self.roulette && self.miniapp,
            "menu.btn.modules" => self.modules,
            "menu.btn.documents" => self.documents,
            "menu.btn.support" => true,
            "menu.btn.email" => self.email,
            "menu.btn.language" => self.language,
            _ => false,
        }
    }

    pub fn keys(&self) -> Vec<&'static str> {
        MENU.iter()
            .map(|s| s.key)
            .filter(|key| self.shows(key))
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubscriptionFacts {
    pub link: bool,
    pub remna: bool,
    pub reissue: bool,
    pub renewal: bool,
}

impl SubscriptionFacts {
    pub fn shows(&self, key: &str) -> bool {
        match key {
            "sub.btn.connect" | "sub.btn.copy_link" => self.link,
            "sub.btn.devices" => self.remna,
            "sub.btn.reissue" => self.remna && self.reissue,
            "sub.btn.renewal_off" => self.renewal,
            "sub.btn.extend" | "sub.btn.all" | "common.btn.menu" => true,
            _ => false,
        }
    }

    pub fn keys(&self) -> Vec<&'static str> {
        SUBSCRIPTION
            .iter()
            .map(|s| s.key)
            .filter(|key| self.shows(key))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuMode {
    #[default]
    Classic,
    Web,
}

impl MenuMode {
    pub fn is_classic(&self) -> bool {
        *self == MenuMode::Classic
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AppSection {
    pub path: &'static str,
    pub title: &'static str,
}

const fn section(path: &'static str, title: &'static str) -> AppSection {
    AppSection { path, title }
}

pub const APP_HOME: AppSection = section("", "Главная");
pub const APP_PROFILE: AppSection = section("/profile", "Профиль");
pub const APP_FRIENDS: AppSection = section("/friends", "Друзья");

pub fn app_section(key: &str) -> Option<AppSection> {
    Some(match key {
        "menu.btn.app" | "menu.btn.subscription" => APP_HOME,
        "menu.btn.trial" => section("/trial", "Пробный период"),
        "menu.btn.buy" | "menu.btn.extend" => section("/plans", "Тарифы"),
        "menu.btn.balance" => section("/topup", "Пополнение"),
        "menu.btn.promo" => section("/promo", "Промокод"),
        "menu.btn.options" => section("/addons", "Докупить"),
        "menu.btn.gifts" => section("/gifts", "Подарки"),
        "menu.btn.referral" => APP_PROFILE,
        "menu.btn.roulette" => section("/roulette", "Рулетка"),
        "menu.btn.support" => section("/support", "Поддержка"),
        "menu.btn.email" => section("/profile/email", "Почта"),
        "menu.btn.language" => section("/profile/language", "Язык"),
        _ => return None,
    })
}

pub fn arrange<'a, T>(rows: &[Vec<String>], present: Vec<(&'a str, T)>) -> Vec<Vec<T>> {
    let mut left: Vec<Option<(&'a str, T)>> = present.into_iter().map(Some).collect();
    let mut out = Vec::new();
    for row in rows {
        let mut line = Vec::new();
        for key in row {
            for item in left.iter_mut() {
                if item.as_ref().is_some_and(|(k, _)| *k == key.as_str())
                    && let Some((_, value)) = item.take()
                {
                    line.push(value);
                }
            }
        }
        let mut line = line.into_iter().peekable();
        while line.peek().is_some() {
            out.push(line.by_ref().take(ROW_MAX).collect());
        }
    }
    for (_, value) in left.into_iter().flatten() {
        out.push(vec![value]);
    }
    out
}

pub fn pack<T>(present: Vec<(&str, T)>, columns: usize) -> Vec<Vec<T>> {
    let mut items = present.into_iter().map(|(_, value)| value).peekable();
    let mut out = Vec::new();
    while items.peek().is_some() {
        out.push(items.by_ref().take(columns.clamp(1, ROW_MAX)).collect());
    }
    out
}

pub fn normalize(screen: Screen, rows: &[Vec<String>]) -> Rows {
    let mut seen = BTreeSet::new();
    let mut out: Rows = Vec::new();
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .filter_map(|key| screen.slot(key))
            .filter(|s| seen.insert(s.key))
            .map(|s| s.key.to_string())
            .collect();
        for chunk in line.chunks(ROW_MAX) {
            out.push(chunk.to_vec());
        }
    }
    for s in screen.slots() {
        if !seen.contains(s.key) {
            out.push(vec![s.key.to_string()]);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutProblem {
    Unknown(String),
    Twice(String),
    Crowded(usize),
}

impl LayoutProblem {
    pub fn message(&self, screen: Screen) -> String {
        match self {
            LayoutProblem::Unknown(key) => {
                format!("На экране «{}» нет кнопки {key}", screen.title())
            }
            LayoutProblem::Twice(key) => {
                format!("Кнопка {key} стоит на экране «{}» дважды", screen.title())
            }
            LayoutProblem::Crowded(row) => format!(
                "Экран «{}», ряд {row}: в ряду не больше {ROW_MAX} кнопок",
                screen.title()
            ),
        }
    }
}

pub fn clean(screen: Screen, rows: &[Vec<String>]) -> Result<Rows, LayoutProblem> {
    let mut seen = BTreeSet::new();
    let mut out: Rows = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if row.len() > ROW_MAX {
            return Err(LayoutProblem::Crowded(index + 1));
        }
        let mut line = Vec::new();
        for raw in row {
            let key = raw.trim();
            let found = screen
                .slot(key)
                .ok_or_else(|| LayoutProblem::Unknown(key.to_string()))?;
            if !seen.insert(found.key) {
                return Err(LayoutProblem::Twice(key.to_string()));
            }
            line.push(found.key.to_string());
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    for s in screen.slots() {
        if !seen.contains(s.key) {
            out.push(vec![s.key.to_string()]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(raw: &[&[&str]]) -> Rows {
        raw.iter()
            .map(|row| row.iter().map(|k| k.to_string()).collect())
            .collect()
    }

    #[test]
    fn раскладка_ставит_кнопки_по_рядам_и_сжимает_пустые_места() {
        let layout = rows(&[&["a", "b"], &["c"], &["d", "e", "f"]]);
        let present = vec![("f", 6), ("a", 1), ("c", 3), ("e", 5)];
        assert_eq!(
            arrange(&layout, present),
            vec![vec![1], vec![3], vec![5, 6]]
        );
    }

    #[test]
    fn кнопки_модулей_встают_на_место_своей_ячейки() {
        let layout = rows(&[&["a", "mods"], &["b"]]);
        let present = vec![("a", 1), ("mods", 2), ("mods", 3), ("b", 4)];
        assert_eq!(arrange(&layout, present), vec![vec![1, 2, 3], vec![4]]);
        let layout = rows(&[&["a", "b", "mods"]]);
        let present = vec![("a", 1), ("b", 2), ("mods", 3), ("mods", 4)];
        assert_eq!(arrange(&layout, present), vec![vec![1, 2, 3], vec![4]]);
    }

    #[test]
    fn новая_кнопка_не_теряется_а_встаёт_в_конец() {
        let layout = rows(&[&["b", "a"]]);
        let present = vec![("a", 1), ("b", 2), ("new", 3), ("newer", 4)];
        assert_eq!(
            arrange(&layout, present),
            vec![vec![2, 1], vec![3], vec![4]]
        );
    }

    #[test]
    fn удалённая_из_кода_кнопка_пропускается() {
        let layout = rows(&[&["gone", "a"], &["gone2"], &["b"]]);
        let present = vec![("a", 1), ("b", 2)];
        assert_eq!(arrange(&layout, present), vec![vec![1], vec![2]]);
    }

    #[test]
    fn ряд_длиннее_трёх_кнопок_делится_при_показе() {
        let layout = rows(&[&["a", "b", "c", "d", "e"]]);
        let present = vec![("a", 1), ("b", 2), ("c", 3), ("d", 4), ("e", 5)];
        assert_eq!(arrange(&layout, present), vec![vec![1, 2, 3], vec![4, 5]]);
    }

    #[test]
    fn без_своей_раскладки_кнопки_идут_подряд_по_колонкам() {
        let present = vec![("a", 1), ("b", 2), ("c", 3)];
        assert_eq!(pack(present.clone(), 2), vec![vec![1, 2], vec![3]]);
        assert_eq!(pack(present.clone(), 1), vec![vec![1], vec![2], vec![3]]);
        assert_eq!(pack(present, 0), vec![vec![1], vec![2], vec![3]]);
        assert!(pack(Vec::<(&str, u8)>::new(), 2).is_empty());
    }

    #[test]
    fn кнопка_в_раскладке_дважды_показывается_один_раз() {
        let layout = rows(&[&["a", "a"], &["a", "b"]]);
        let present = vec![("a", 1), ("b", 2)];
        assert_eq!(arrange(&layout, present), vec![vec![1], vec![2]]);
    }

    #[test]
    fn стандартная_раскладка_меню_по_одной_и_по_две() {
        let one = Screen::Menu.standard(1);
        assert_eq!(one.len(), MENU.len());
        assert!(one.iter().all(|row| row.len() == 1));
        assert_eq!(one[0], vec!["menu.btn.app".to_string()]);
        let two = Screen::Menu.standard(2);
        assert_eq!(two.len(), MENU.len().div_ceil(2));
        assert_eq!(
            two[1],
            vec!["menu.btn.trial".to_string(), "menu.btn.buy".to_string()]
        );
        assert_eq!(Screen::Menu.standard(9).first().map(Vec::len), Some(3));
        assert_eq!(
            Screen::Subscription.standard(2).last(),
            Some(&vec![
                "sub.btn.all".to_string(),
                "common.btn.menu".to_string()
            ])
        );
    }

    #[test]
    fn проверка_раскладки_ловит_лишнее_и_дописывает_недостающее() {
        let crowded = rows(&[&[
            "menu.btn.app",
            "menu.btn.buy",
            "menu.btn.trial",
            "menu.btn.support",
        ]]);
        assert_eq!(
            clean(Screen::Menu, &crowded),
            Err(LayoutProblem::Crowded(1))
        );
        assert!(
            LayoutProblem::Crowded(1)
                .message(Screen::Menu)
                .contains("не больше 3")
        );
        assert_eq!(
            clean(Screen::Menu, &rows(&[&["menu.btn.app", "sub.btn.all"]])),
            Err(LayoutProblem::Unknown("sub.btn.all".into()))
        );
        assert_eq!(
            clean(Screen::Menu, &rows(&[&["menu.btn.app"], &["menu.btn.app"]])),
            Err(LayoutProblem::Twice("menu.btn.app".into()))
        );
        let cleaned = clean(
            Screen::Subscription,
            &rows(&[&[], &["common.btn.menu", " sub.btn.extend "]]),
        )
        .unwrap();
        assert_eq!(cleaned[0], vec!["common.btn.menu", "sub.btn.extend"]);
        assert_eq!(cleaned.len(), 1 + SUBSCRIPTION.len() - 2);
        assert_eq!(cleaned.concat().len(), SUBSCRIPTION.len());
    }

    #[test]
    fn старая_раскладка_приводится_к_текущим_кнопкам() {
        let saved = rows(&[
            &["menu.btn.support", "gone", "menu.btn.support"],
            &[
                "menu.btn.app",
                "menu.btn.buy",
                "menu.btn.trial",
                "menu.btn.email",
            ],
        ]);
        let view = normalize(Screen::Menu, &saved);
        assert_eq!(view[0], vec!["menu.btn.support"]);
        assert_eq!(
            view[1],
            vec!["menu.btn.app", "menu.btn.buy", "menu.btn.trial"]
        );
        assert_eq!(view[2], vec!["menu.btn.email"]);
        assert_eq!(view.concat().len(), MENU.len());
        assert!(view.iter().all(|row| row.len() <= ROW_MAX));
    }

    #[test]
    fn видимость_кнопок_меню_зависит_от_подписки_и_модулей() {
        let fresh = MenuFacts {
            trial: true,
            promo: true,
            ..MenuFacts::default()
        };
        assert_eq!(
            fresh.keys(),
            vec![
                "menu.btn.trial",
                "menu.btn.buy",
                "menu.btn.promo",
                "menu.btn.support"
            ]
        );
        let subscribed = MenuFacts {
            subscribed: true,
            trial: true,
            roulette: true,
            ..MenuFacts::default()
        };
        assert_eq!(
            subscribed.keys(),
            vec![
                "menu.btn.subscription",
                "menu.btn.extend",
                "menu.btn.support"
            ]
        );
        let sub = SubscriptionFacts {
            link: true,
            remna: false,
            reissue: true,
            renewal: false,
        };
        assert_eq!(
            sub.keys(),
            vec![
                "sub.btn.connect",
                "sub.btn.copy_link",
                "sub.btn.extend",
                "sub.btn.all",
                "common.btn.menu"
            ]
        );
    }

    #[test]
    fn режим_меню_по_умолчанию_классический_и_не_пишется_в_настройку() {
        use crate::bot_content::ContentConfig;
        let old: ContentConfig = serde_json::from_value(serde_json::json!({
            "buttons": { "menu.btn.balance": { "hidden": true } }
        }))
        .unwrap();
        assert_eq!(old.menu_mode, MenuMode::Classic);
        assert!(
            serde_json::to_value(&old)
                .unwrap()
                .get("menu_mode")
                .is_none()
        );
        let web: ContentConfig =
            serde_json::from_value(serde_json::json!({ "menu_mode": "web" })).unwrap();
        assert_eq!(web.menu_mode, MenuMode::Web);
        assert_eq!(serde_json::to_value(&web).unwrap()["menu_mode"], "web");
        assert!(
            serde_json::from_value::<ContentConfig>(serde_json::json!({ "menu_mode": "tabs" }))
                .is_err()
        );
    }

    #[test]
    fn у_раздела_мини_приложения_свой_адрес() {
        let paths: Vec<(&str, Option<&str>)> = MENU
            .iter()
            .map(|s| (s.key, app_section(s.key).map(|a| a.path)))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("menu.btn.app", Some("")),
                ("menu.btn.subscription", Some("")),
                ("menu.btn.trial", Some("/trial")),
                ("menu.btn.buy", Some("/plans")),
                ("menu.btn.extend", Some("/plans")),
                ("menu.btn.balance", Some("/topup")),
                ("menu.btn.promo", Some("/promo")),
                ("menu.btn.options", Some("/addons")),
                ("menu.btn.gifts", Some("/gifts")),
                ("menu.btn.referral", Some("/profile")),
                ("menu.btn.contests", None),
                ("menu.btn.roulette", Some("/roulette")),
                ("menu.btn.modules", None),
                ("menu.btn.documents", None),
                ("menu.btn.support", Some("/support")),
                ("menu.btn.email", Some("/profile/email")),
                ("menu.btn.language", Some("/profile/language")),
            ]
        );
        assert!(SUBSCRIPTION.iter().all(|s| app_section(s.key).is_none()));
        assert!(
            MENU.iter()
                .filter_map(|s| app_section(s.key))
                .all(|a| !a.title.is_empty() && (a.path.is_empty() || a.path.starts_with('/')))
        );
    }

    #[test]
    fn у_каждой_кнопки_раскладки_есть_текст_в_каталоге() {
        for screen in Screen::ALL {
            assert_eq!(Screen::parse(screen.as_str()), Some(screen));
            for s in screen.slots() {
                let entry = crate::bot_content::entry(s.key)
                    .unwrap_or_else(|| panic!("нет кнопки {} в каталоге", s.key));
                assert_eq!(entry.kind, crate::bot_content::EntryKind::Button);
                assert!(!s.when.is_empty());
            }
        }
    }
}
