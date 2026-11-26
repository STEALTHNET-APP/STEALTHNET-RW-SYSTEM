use crate::Actor;
use crate::bot_content::CONTENT_KEY;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sn_domain::bot_content::{self as catalog, ButtonStyle, Content, ContentConfig};
use sn_domain::bot_layout::{
    self as layout, AppSection, Audience, MenuFacts, MenuMode, ROW_MAX, Rows, Screen,
    SubscriptionFacts,
};
use sn_domain::client::Locale;
use sn_domain::ids::AdminId;
use sn_domain::module::ModuleId;
use sn_modules::catalog::{contests, roulette};
use sn_store::Db;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Install {
    pub miniapp: Option<String>,
    pub web: bool,
    pub trial: bool,
    pub balance: bool,
    pub promo: bool,
    pub options: bool,
    pub gifts: bool,
    pub referral: bool,
    pub contests: bool,
    pub contest_open: bool,
    pub roulette: bool,
    pub roulette_ready: bool,
    pub documents: bool,
    pub mail_on: bool,
    pub mail: bool,
    pub site_login: bool,
    pub site: bool,
    pub english: bool,
    pub reissue: bool,
    pub support: Option<String>,
    pub support_app: bool,
    pub friends_screen: bool,
}

pub fn site_url(services: &Services) -> Option<String> {
    let settings = services.settings();
    let base = settings.public_url.as_deref()?.trim().trim_end_matches('/');
    (base.starts_with("https://") || base.starts_with("http://")).then(|| base.to_string())
}

pub fn link_button(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("tg://")
}

pub fn miniapp_off(services: &Services) -> Option<&'static str> {
    let settings = services.settings();
    if !settings.client_access.miniapp {
        Some("Мини-приложение выключено")
    } else if settings.miniapp_url().is_none() {
        Some("У адреса сервиса нет https, а Telegram открывает мини-приложение только по https")
    } else {
        None
    }
}

pub fn email_offered(services: &Services) -> bool {
    services.settings().client_access.site_login
        && services.mail().links_email()
        && site_url(services).is_some()
}

impl Install {
    pub async fn load(services: &Services) -> Install {
        let settings = services.settings();
        let modules = services.modules();
        let ctx = services.module_context();
        let miniapp = settings.miniapp_url();
        let contests_on = modules.is_enabled(ModuleId::Contests);
        let roulette_on = modules.is_enabled(ModuleId::Roulette);
        let contest_open = contests_on && contests::has_running(&ctx).await.unwrap_or(false);
        let roulette_ready =
            roulette_on && miniapp.is_some() && roulette::ready(&ctx).await.unwrap_or(false);
        Install {
            trial: settings.trial.is_offered(),
            balance: services.payments().balance_enabled(),
            promo: modules.is_enabled(ModuleId::Promo),
            options: modules.is_enabled(ModuleId::Options),
            gifts: modules.is_enabled(ModuleId::Gifts),
            referral: modules.is_enabled(ModuleId::Referral),
            contests: contests_on,
            contest_open,
            roulette: roulette_on,
            roulette_ready,
            documents: !services.bot_content().documents.is_empty(),
            mail_on: modules.is_enabled(ModuleId::Mail),
            mail: services.mail().links_email(),
            site_login: settings.client_access.site_login,
            site: site_url(services).is_some(),
            english: settings.languages.several(),
            reissue: settings.client_access.self_reissue,
            support: settings
                .support_link()
                .filter(|u| link_button(u) && !settings.support_tickets())
                .map(str::to_string),
            support_app: settings.support_tickets(),
            friends_screen: !settings.miniapp.friends_in_profile(),
            web: services.bot_content().content.menu_mode() == MenuMode::Web,
            miniapp,
        }
    }

    pub fn app_section(&self, key: &str) -> Option<AppSection> {
        match key {
            "menu.btn.balance" if !self.balance => Some(layout::APP_PROFILE),
            "menu.btn.referral" if self.friends_screen => Some(layout::APP_FRIENDS),
            "menu.btn.support" if self.support.is_some() || !self.support_app => None,
            _ => layout::app_section(key),
        }
    }

    fn web_note(&self, key: &str) -> Option<String> {
        if self.app_section(key).is_some() {
            return None;
        }
        match key {
            "menu.btn.contests" => {
                Some("Конкурсы проходят в боте: в мини-приложении их нет".to_string())
            }
            "menu.btn.documents" => {
                Some("Документы открываются в боте: в мини-приложении их нет".to_string())
            }
            _ => None,
        }
    }

    pub fn app_url(&self, key: &str) -> Option<String> {
        let app = self.miniapp.as_deref()?;
        if !self.web && !matches!(key, "menu.btn.app" | "menu.btn.roulette") {
            return None;
        }
        self.app_section(key)
            .map(|section| format!("{app}{}", section.path))
    }

    pub fn email(&self) -> bool {
        self.mail && self.site_login && self.site
    }

    pub fn menu(&self, subscribed: bool, trial_left: bool, money: bool) -> MenuFacts {
        MenuFacts {
            miniapp: self.miniapp.is_some(),
            subscribed,
            trial: self.trial && trial_left,
            balance: self.balance || money,
            promo: self.promo,
            options: self.options,
            gifts: self.gifts,
            referral: self.referral,
            contests: self.contest_open,
            roulette: self.roulette_ready,
            documents: self.documents,
            email: self.email(),
            language: self.english,
        }
    }

    pub fn subscription(&self, link: bool, remna: bool) -> SubscriptionFacts {
        SubscriptionFacts {
            link,
            remna,
            reissue: self.reissue,
            renewal: false,
        }
    }

    fn off(&self, key: &str) -> Option<String> {
        let module = |on: bool, id: ModuleId| {
            (!on).then(|| format!("Модуль «{}» выключен", id.info().title))
        };
        match key {
            "menu.btn.app" => self.miniapp.is_none().then(|| {
                "Мини-приложение выключено в «Кабинет и мини-приложение» или у адреса сервиса нет https".to_string()
            }),
            "menu.btn.trial" => (!self.trial).then(|| "Пробный период выключен в «Тарифы»".to_string()),
            "menu.btn.balance" => (!self.balance).then(|| {
                "Пополнение баланса выключено в «Оплата»: кнопку видят только клиенты с деньгами на балансе".to_string()
            }),
            "menu.btn.promo" => module(self.promo, ModuleId::Promo),
            "menu.btn.options" => module(self.options, ModuleId::Options),
            "menu.btn.gifts" => module(self.gifts, ModuleId::Gifts),
            "menu.btn.referral" => module(self.referral, ModuleId::Referral),
            "menu.btn.contests" => module(self.contests, ModuleId::Contests)
                .or_else(|| (!self.contest_open).then(|| "Сейчас нет открытого конкурса".to_string())),
            "menu.btn.roulette" => module(self.roulette, ModuleId::Roulette)
                .or_else(|| {
                    self.miniapp.is_none().then(|| {
                        "Рулетка открывается в мини-приложении, а оно выключено".to_string()
                    })
                })
                .or_else(|| (!self.roulette_ready).then(|| "Рулетка ещё не настроена".to_string())),
            "menu.btn.documents" => (!self.documents).then(|| "Документы не добавлены в «Документы»".to_string()),
            "menu.btn.email" => (!self.mail)
                .then(|| {
                    module(self.mail_on, ModuleId::Mail).unwrap_or_else(|| {
                        "Письма не отправляются, а «Привязывать почту без кода» в «Почта и уведомления» выключено".to_string()
                    })
                })
                .or_else(|| {
                    (!self.site_login).then(|| {
                        "Выключено «Пускать в кабинет на сайте» в «Кабинет и мини-приложение»".to_string()
                    })
                })
                .or_else(|| (!self.site).then(|| "Не задан адрес сервиса".to_string())),
            "menu.btn.language" => (!self.english).then(|| "Английский выключен в «Языки»".to_string()),
            "sub.btn.reissue" => (!self.reissue).then(|| {
                "Выключено «Выпускать новую ссылку подписки» в «Кабинет и мини-приложение»".to_string()
            }),
            "sub.btn.renewal_off" => Some(
                "Видна, только когда подписку продлевает платёжная система, например Lava.top"
                    .to_string(),
            ),
            _ => None,
        }
    }

    fn kind(&self, key: &str) -> Kind {
        match key {
            "menu.btn.app" | "menu.btn.roulette" => Kind::WebApp,
            "menu.btn.support" if self.support.is_some() => Kind::Link,
            "sub.btn.connect" => Kind::Link,
            _ => Kind::Callback,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientState {
    New,
    Active,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Callback,
    Link,
    WebApp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotView {
    pub key: &'static str,
    pub label: &'static str,
    pub text: String,
    pub text_en: String,
    pub audience: Audience,
    pub when: &'static str,
    pub off: Option<String>,
    pub hidden: bool,
    pub hideable: bool,
    pub style: Option<ButtonStyle>,
    pub emoji: bool,
    pub kind: Kind,
    pub web: Option<&'static str>,
    pub web_note: Option<String>,
    pub shown: Vec<ClientState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScreenView {
    pub key: Screen,
    pub title: &'static str,
    pub rows: Rows,
    pub standard: Rows,
    pub custom: bool,
    pub packed: Option<usize>,
    pub states: Vec<ClientState>,
    pub slots: Vec<SlotView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayoutView {
    pub screens: Vec<ScreenView>,
    pub menu_mode: MenuMode,
    pub web_off: Option<&'static str>,
    pub row_max: usize,
    pub english: bool,
    pub balance: bool,
    pub brand: String,
    pub texts: BTreeMap<&'static str, String>,
    pub texts_en: BTreeMap<&'static str, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct LayoutUpdate {
    #[serde(default)]
    pub layouts: BTreeMap<String, Option<Rows>>,
    #[serde(default)]
    pub hidden: BTreeMap<String, bool>,
    #[serde(default)]
    pub menu_mode: Option<MenuMode>,
}

const PREVIEW_TEXTS: [&str; 20] = [
    "menu.title",
    "menu.greeting",
    "menu.no_subscription",
    "menu.single",
    "menu.single_until",
    "menu.single_ended",
    "menu.single_traffic",
    "menu.single_traffic_unlimited",
    "menu.single_devices",
    "menu.balance",
    "status.active",
    "status.expired",
    "sub.title",
    "sub.until",
    "sub.ended",
    "sub.traffic",
    "sub.devices",
    "sub.autorenew_on",
    "sub.autorenew_off",
    "sub.link",
];

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::rejected("bot_layout.invalid", message)
}

fn shown_in(screen: Screen, install: &Install, key: &str) -> Vec<ClientState> {
    match screen {
        Screen::Menu => [ClientState::New, ClientState::Active, ClientState::Ended]
            .into_iter()
            .filter(|state| {
                install
                    .menu(*state != ClientState::New, true, false)
                    .shows(key)
            })
            .collect(),
        Screen::Subscription => [ClientState::Active, ClientState::Ended]
            .into_iter()
            .filter(|_| install.subscription(true, true).shows(key))
            .collect(),
    }
}

fn screen_view(
    screen: Screen,
    content: &Content,
    plain: &Content,
    plain_en: &Content,
    install: &Install,
) -> ScreenView {
    let config = content.config();
    let custom = config.layouts.get(screen.as_str());
    let standard = screen.standard(content.menu_columns());
    ScreenView {
        key: screen,
        title: screen.title(),
        rows: match custom {
            Some(rows) => layout::normalize(screen, rows),
            None => standard.clone(),
        },
        custom: custom.is_some(),
        packed: (custom.is_none() && screen == Screen::Menu).then(|| content.menu_columns()),
        standard,
        states: match screen {
            Screen::Menu => vec![ClientState::New, ClientState::Active, ClientState::Ended],
            Screen::Subscription => vec![ClientState::Active, ClientState::Ended],
        },
        slots: screen
            .slots()
            .iter()
            .map(|slot| {
                let own = config.buttons.get(slot.key);
                let entry = catalog::entry(slot.key);
                SlotView {
                    key: slot.key,
                    label: entry.map(|e| e.label).unwrap_or(slot.key),
                    text: plain.label(slot.key, &[]),
                    text_en: plain_en.label(slot.key, &[]),
                    audience: slot.audience,
                    when: slot.when,
                    off: install.off(slot.key),
                    hidden: own.is_some_and(|o| o.hidden) && entry.is_some_and(|e| e.optional),
                    hideable: entry.is_some_and(|e| e.optional),
                    style: own.and_then(|o| o.style),
                    emoji: own.is_some_and(|o| o.emoji_id.is_some()),
                    kind: install.kind(slot.key),
                    web: install.app_section(slot.key).map(|s| s.title),
                    web_note: install.web_note(slot.key),
                    shown: shown_in(screen, install, slot.key),
                }
            })
            .collect(),
    }
}

pub struct BotLayoutOps<'a> {
    services: &'a Services,
}

impl<'a> BotLayoutOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        BotLayoutOps { services }
    }

    pub async fn view(&self) -> ServiceResult<LayoutView> {
        let install = Install::load(self.services).await;
        let runtime = self.services.bot_content();
        let content = &runtime.content;
        let mut unhidden: ContentConfig = content.config().clone();
        for own in unhidden.buttons.values_mut() {
            own.hidden = false;
        }
        let plain = Content::new(unhidden);
        let plain_en = plain.clone().in_locale(Locale::En);
        let english = content.clone().in_locale(Locale::En);
        Ok(LayoutView {
            screens: Screen::ALL
                .into_iter()
                .map(|screen| screen_view(screen, content, &plain, &plain_en, &install))
                .collect(),
            menu_mode: content.menu_mode(),
            web_off: miniapp_off(self.services),
            row_max: ROW_MAX,
            english: install.english,
            balance: install.balance,
            brand: self.services.settings().brand_name.clone(),
            texts: PREVIEW_TEXTS
                .into_iter()
                .map(|key| (key, content.template(key).to_string()))
                .collect(),
            texts_en: PREVIEW_TEXTS
                .into_iter()
                .map(|key| (key, english.template(key).to_string()))
                .collect(),
        })
    }

    pub async fn update(&self, update: &LayoutUpdate, admin: AdminId) -> ServiceResult<LayoutView> {
        let current = self.services.bot_content().content.config().clone();
        let mut next = current.clone();
        let mut screens = Vec::new();
        let mut reset = Vec::new();
        for (raw, rows) in &update.layouts {
            let screen = Screen::parse(raw)
                .ok_or_else(|| invalid(format!("Экрана {raw} нет в раскладке бота")))?;
            match rows {
                None => {
                    if next.layouts.remove(screen.as_str()).is_some() {
                        reset.push(screen.as_str());
                    }
                }
                Some(rows) => {
                    let clean =
                        layout::clean(screen, rows).map_err(|p| invalid(p.message(screen)))?;
                    if next.layouts.get(screen.as_str()) != Some(&clean) {
                        screens.push(screen.as_str());
                    }
                    next.layouts.insert(screen.as_str().to_string(), clean);
                }
            }
        }
        let mut hidden = Vec::new();
        let mut shown = Vec::new();
        for (key, hide) in &update.hidden {
            let hideable = Screen::ALL.iter().any(|s| s.slot(key).is_some())
                && catalog::entry(key).is_some_and(|e| e.optional);
            if !hideable {
                return Err(invalid(format!("Кнопку {key} нельзя скрыть")));
            }
            let own = next.buttons.entry(key.clone()).or_default();
            if own.hidden != *hide {
                own.hidden = *hide;
                if *hide {
                    hidden.push(key.as_str());
                } else {
                    shown.push(key.as_str());
                }
            }
            if own.is_empty() {
                next.buttons.remove(key);
            }
        }
        let mut mode = None;
        if let Some(wanted) = update.menu_mode
            && wanted != next.menu_mode
        {
            if wanted == MenuMode::Web
                && let Some(off) = miniapp_off(self.services)
            {
                return Err(invalid(format!(
                    "Веб-режим недоступен. {off}. Проверьте «Кабинет и мини-приложение»"
                )));
            }
            next.menu_mode = wanted;
            mode = Some(wanted);
        }
        if next == current {
            return self.view().await;
        }
        let mut details = json!({
            "screens": screens,
            "reset": reset,
            "hidden": hidden,
            "shown": shown,
        });
        if let Some(mode) = mode {
            details["menu_mode"] = json!(mode);
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(CONTENT_KEY, &next, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit("settings.bot_layout", "settings", None, details))
            .await?;
        tx.commit().await?;
        self.services.reload().await?;
        self.services.notify_settings_changed().await?;
        self.view().await
    }
}

#[cfg(test)]
mod tests;
