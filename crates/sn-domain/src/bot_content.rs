use crate::client::Locale;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const TEXT_MAX: usize = 4096;
pub const CAPTION_MAX: usize = 1024;
pub const BUTTON_MAX: usize = 64;
pub const EMOJI_ID_MAX: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Menu,
    Subscription,
    Devices,
    Buy,
    Payment,
    Balance,
    Support,
    Promo,
    Referral,
    Options,
    Gifts,
    PromoLinks,
    Contests,
    Login,
    Documents,
    Notify,
    Common,
}

impl Section {
    pub const ALL: [Section; 17] = [
        Section::Menu,
        Section::Subscription,
        Section::Devices,
        Section::Buy,
        Section::Payment,
        Section::Balance,
        Section::Support,
        Section::Promo,
        Section::Referral,
        Section::Options,
        Section::Gifts,
        Section::PromoLinks,
        Section::Contests,
        Section::Login,
        Section::Documents,
        Section::Notify,
        Section::Common,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Menu => "Главное меню",
            Section::Subscription => "Подписка",
            Section::Devices => "Устройства",
            Section::Buy => "Покупка",
            Section::Payment => "Счёт и оплата",
            Section::Balance => "Баланс",
            Section::Support => "Поддержка",
            Section::Promo => "Промокод",
            Section::Referral => "Пригласить друга",
            Section::Options => "Доп. устройства",
            Section::Gifts => "Подарки",
            Section::PromoLinks => "Промо-ссылки",
            Section::Contests => "Конкурсы",
            Section::Login => "Вход в кабинет",
            Section::Documents => "Документы",
            Section::Notify => "Уведомления",
            Section::Common => "Общие кнопки",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Section::Menu => "menu",
            Section::Subscription => "subscription",
            Section::Devices => "devices",
            Section::Buy => "buy",
            Section::Payment => "payment",
            Section::Balance => "balance",
            Section::Support => "support",
            Section::Promo => "promo",
            Section::Referral => "referral",
            Section::Options => "options",
            Section::Gifts => "gifts",
            Section::PromoLinks => "promo_links",
            Section::Contests => "contests",
            Section::Login => "login",
            Section::Documents => "documents",
            Section::Notify => "notify",
            Section::Common => "common",
        }
    }

    pub fn image(self) -> Option<ImageSlot> {
        match self {
            Section::Menu => Some(ImageSlot::Menu),
            Section::Subscription | Section::Devices => Some(ImageSlot::Subscription),
            Section::Buy | Section::Payment => Some(ImageSlot::Buy),
            Section::Balance => Some(ImageSlot::Balance),
            Section::Support => Some(ImageSlot::Support),
            Section::Promo => Some(ImageSlot::Promo),
            Section::Referral => Some(ImageSlot::Referral),
            Section::Options => Some(ImageSlot::Options),
            Section::Gifts => Some(ImageSlot::Buy),
            Section::PromoLinks => Some(ImageSlot::Promo),
            Section::Contests
            | Section::Login
            | Section::Documents
            | Section::Notify
            | Section::Common => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSlot {
    Common,
    Menu,
    Subscription,
    Buy,
    Balance,
    Support,
    Promo,
    Referral,
    Options,
}

impl ImageSlot {
    pub const ALL: [ImageSlot; 9] = [
        ImageSlot::Common,
        ImageSlot::Menu,
        ImageSlot::Subscription,
        ImageSlot::Buy,
        ImageSlot::Balance,
        ImageSlot::Support,
        ImageSlot::Promo,
        ImageSlot::Referral,
        ImageSlot::Options,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ImageSlot::Common => "common",
            ImageSlot::Menu => "menu",
            ImageSlot::Subscription => "subscription",
            ImageSlot::Buy => "buy",
            ImageSlot::Balance => "balance",
            ImageSlot::Support => "support",
            ImageSlot::Promo => "promo",
            ImageSlot::Referral => "referral",
            ImageSlot::Options => "options",
        }
    }

    pub fn parse(raw: &str) -> Option<ImageSlot> {
        ImageSlot::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub fn title(self) -> &'static str {
        match self {
            ImageSlot::Common => "Общая картинка",
            ImageSlot::Menu => "Главное меню",
            ImageSlot::Subscription => "Подписка и устройства",
            ImageSlot::Buy => "Покупка и оплата",
            ImageSlot::Balance => "Баланс",
            ImageSlot::Support => "Поддержка",
            ImageSlot::Promo => "Промокод",
            ImageSlot::Referral => "Пригласить друга",
            ImageSlot::Options => "Доп. устройства",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageMode {
    #[default]
    Common,
    Own,
    Off,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagesConfig {
    #[serde(default)]
    pub sections: BTreeMap<ImageSlot, ImageMode>,
}

impl ImagesConfig {
    pub fn mode(&self, slot: ImageSlot) -> ImageMode {
        self.sections.get(&slot).copied().unwrap_or_default()
    }

    pub fn resolve(
        &self,
        section: Section,
        has_image: impl Fn(ImageSlot) -> bool,
    ) -> Option<ImageSlot> {
        let slot = section.image()?;
        match self.mode(slot) {
            ImageMode::Off => None,
            ImageMode::Own if has_image(slot) => Some(slot),
            ImageMode::Own | ImageMode::Common => {
                has_image(ImageSlot::Common).then_some(ImageSlot::Common)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Text,
    Button,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Var {
    pub name: &'static str,
    pub hint: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub key: &'static str,
    pub section: Section,
    pub kind: EntryKind,
    pub label: &'static str,
    pub default: &'static str,
    pub vars: &'static [Var],
    pub optional: bool,
}

const fn text(
    key: &'static str,
    section: Section,
    label: &'static str,
    default: &'static str,
    vars: &'static [Var],
) -> Entry {
    Entry {
        key,
        section,
        kind: EntryKind::Text,
        label,
        default,
        vars,
        optional: false,
    }
}

const fn line(
    key: &'static str,
    section: Section,
    label: &'static str,
    default: &'static str,
    vars: &'static [Var],
) -> Entry {
    Entry {
        key,
        section,
        kind: EntryKind::Text,
        label,
        default,
        vars,
        optional: true,
    }
}

const fn button(
    key: &'static str,
    section: Section,
    label: &'static str,
    default: &'static str,
    vars: &'static [Var],
) -> Entry {
    Entry {
        key,
        section,
        kind: EntryKind::Button,
        label,
        default,
        vars,
        optional: false,
    }
}

const fn hideable(
    key: &'static str,
    section: Section,
    label: &'static str,
    default: &'static str,
) -> Entry {
    Entry {
        key,
        section,
        kind: EntryKind::Button,
        label,
        default,
        vars: &[],
        optional: true,
    }
}

const BRAND: Var = Var {
    name: "brand",
    hint: "название сервиса",
};
const NAME: Var = Var {
    name: "name",
    hint: "имя клиента в Telegram",
};
const TARIFF: Var = Var {
    name: "tariff",
    hint: "название тарифа",
};
const STATUS: Var = Var {
    name: "status",
    hint: "состояние подписки словом",
};
const MARK: Var = Var {
    name: "mark",
    hint: "🟢 — подписка работает, 🔴 — нет",
};
const DATE: Var = Var {
    name: "date",
    hint: "дата",
};
const CURRENT: Var = Var {
    name: "current",
    hint: "тариф текущей подписки",
};
const DAYS: Var = Var {
    name: "days",
    hint: "число дней со словом",
};
const USED: Var = Var {
    name: "used",
    hint: "израсходовано трафика",
};
const LIMIT: Var = Var {
    name: "limit",
    hint: "лимит",
};
const RESET: Var = Var {
    name: "reset",
    hint: "когда обновится трафик",
};
const COUNT: Var = Var {
    name: "count",
    hint: "количество",
};
const URL: Var = Var {
    name: "url",
    hint: "ссылка",
};
const ERROR: Var = Var {
    name: "error",
    hint: "текст ошибки",
};
const TITLE: Var = Var {
    name: "title",
    hint: "название",
};
const NUMBER: Var = Var {
    name: "n",
    hint: "порядковый номер",
};
const DETAILS: Var = Var {
    name: "details",
    hint: "система и приложение",
};
const AMOUNT: Var = Var {
    name: "amount",
    hint: "сумма",
};
const PAY_PROVIDER: Var = Var {
    name: "provider",
    hint: "платёжная система, например Lava.top",
};
const TOPUP_BONUS: Var = Var {
    name: "bonus",
    hint: "бонус за пополнение",
};
const TOTAL_BONUS: Var = Var {
    name: "total",
    hint: "сумма вместе с бонусом",
};
const PRICE: Var = Var {
    name: "price",
    hint: "цена",
};
const PER_MONTH: Var = Var {
    name: "per_month",
    hint: "цена в месяц",
};
const SAVINGS: Var = Var {
    name: "savings",
    hint: "выгода в процентах",
};
const CATEGORY: Var = Var {
    name: "category",
    hint: "название категории",
};
const INCLUDED: Var = Var {
    name: "included",
    hint: "устройств в тарифе",
};
const LABEL: Var = Var {
    name: "label",
    hint: "что оплачивается",
};
const TOTAL: Var = Var {
    name: "total",
    hint: "итого",
};
const STARS: Var = Var {
    name: "stars",
    hint: "цена в звёздах",
};
const METHOD: Var = Var {
    name: "method",
    hint: "способ оплаты",
};
const HINT: Var = Var {
    name: "hint",
    hint: "пояснение к способу оплаты",
};
const MISSING: Var = Var {
    name: "missing",
    hint: "сколько не хватает",
};
const BALANCE: Var = Var {
    name: "balance",
    hint: "сумма на балансе",
};
const WHAT: Var = Var {
    name: "what",
    hint: "что оплачивается",
};
const EXAMPLE: Var = Var {
    name: "example",
    hint: "пример суммы",
};
const ID: Var = Var {
    name: "id",
    hint: "номер",
};
const MEGABYTES: Var = Var {
    name: "mb",
    hint: "предельный размер файла, МБ",
};
const CODE: Var = Var {
    name: "code",
    hint: "промокод",
};
const PERCENT: Var = Var {
    name: "percent",
    hint: "процент с оплат друга",
};
const LEVEL2_PERCENT: Var = Var {
    name: "percent",
    hint: "процент с оплат тех, кого пригласят друзья",
};
const LEVEL3_PERCENT: Var = Var {
    name: "percent",
    hint: "процент с оплат тех, кого пригласят друзья друзей",
};
const LINK: Var = Var {
    name: "link",
    hint: "ссылка-приглашение",
};
const MINIMUM: Var = Var {
    name: "minimum",
    hint: "минимальная сумма вывода",
};
const EXTRA: Var = Var {
    name: "extra",
    hint: "докуплено устройств",
};
const CAN_ADD: Var = Var {
    name: "can_add",
    hint: "сколько ещё можно добавить",
};
const STEPS: Var = Var {
    name: "steps",
    hint: "ступени скидки: от скольких устройств и сколько процентов",
};
const GB: Var = Var {
    name: "gb",
    hint: "гигабайт в пакете",
};
const NOTE: Var = Var {
    name: "note",
    hint: "что будет с неизрасходованным остатком",
};
const TERM: Var = Var {
    name: "term",
    hint: "срок словами",
};
const GIFT_CODE: Var = Var {
    name: "code",
    hint: "код подарка",
};
const GIFT_LINK: Var = Var {
    name: "link",
    hint: "ссылка для друга",
};
const WHO: Var = Var {
    name: "who",
    hint: "аккаунт Telegram",
};
const IP: Var = Var {
    name: "ip",
    hint: "адрес, откуда вход",
};
const AGENT: Var = Var {
    name: "agent",
    hint: "браузер",
};
const BODY: Var = Var {
    name: "body",
    hint: "текст ответа",
};
const COMMENT: Var = Var {
    name: "comment",
    hint: "причина",
};
const WHEN: Var = Var {
    name: "when",
    hint: "«Сегодня» или «На этой неделе»",
};
const PER: Var = Var {
    name: "per",
    hint: "«в сутки» или «в неделю»",
};
const UNTIL: Var = Var {
    name: "until",
    hint: "когда снимется ограничение",
};
const PROMOTION: Var = Var {
    name: "promotion",
    hint: "название акции",
};
const PROMO_PERCENT: Var = Var {
    name: "percent",
    hint: "скидка по акции в процентах",
};
const PROMO_UNTIL: Var = Var {
    name: "ends",
    hint: "до какого числа идёт акция, например «до 30 ноября»",
};
const FULL_PRICE: Var = Var {
    name: "full_price",
    hint: "цена без скидки",
};
const FULL_PER_MONTH: Var = Var {
    name: "full_per_month",
    hint: "цена в месяц без скидки",
};
const PRIZE: Var = Var {
    name: "prize",
    hint: "приз",
};
const PROVIDER: Var = Var {
    name: "provider",
    hint: "Google или Apple",
};
const ACCOUNT: Var = Var {
    name: "account",
    hint: "почта аккаунта Google или Apple",
};
const SITE: Var = Var {
    name: "site",
    hint: "адрес сайта",
};
const MASKED: Var = Var {
    name: "account",
    hint: "почта аккаунта, часть букв скрыта",
};
const WINNERS: Var = Var {
    name: "winners",
    hint: "число победителей",
};
const PLACE: Var = Var {
    name: "place",
    hint: "место победителя",
};
const PLACES: Var = Var {
    name: "places",
    hint: "места, например «2–3 места»",
};
const CONDITION: Var = Var {
    name: "condition",
    hint: "условие участия",
};
const HASH: Var = Var {
    name: "hash",
    hint: "отпечаток жеребьёвки",
};
const RESULT: Var = Var {
    name: "result",
    hint: "как выдан приз",
};
const REFUNDED_STARS: Var = Var {
    name: "stars",
    hint: "сколько звёзд вернули",
};
const PURCHASE: Var = Var {
    name: "label",
    hint: "за что платили",
};
const EMAIL: Var = Var {
    name: "email",
    hint: "адрес почты",
};
const MINUTES: Var = Var {
    name: "minutes",
    hint: "сколько действует код",
};
const CABINET: Var = Var {
    name: "url",
    hint: "адрес кабинета на сайте",
};
const VARIANT: Var = Var {
    name: "variant",
    hint: "название варианта пробного периода",
};

use Section::*;

pub static CATALOG: &[Entry] = &[
    text(
        "menu.title",
        Menu,
        "Заголовок",
        "<b>{brand}</b>",
        &[BRAND, NAME],
    ),
    line(
        "menu.greeting",
        Menu,
        "Приветствие под заголовком",
        "",
        &[BRAND, NAME],
    ),
    line(
        "menu.no_subscription",
        Menu,
        "Если подписки нет",
        "Подписки пока нет.",
        &[],
    ),
    text(
        "menu.single",
        Menu,
        "Одна подписка: тариф и состояние",
        "\n<b>{tariff}</b> · {status}",
        &[TARIFF, STATUS],
    ),
    line(
        "menu.single_until",
        Menu,
        "Одна подписка: срок",
        "До {date}, осталось {days}",
        &[DATE, DAYS],
    ),
    line(
        "menu.single_ended",
        Menu,
        "Одна подписка: срок закончился",
        "Закончилась {date}",
        &[DATE],
    ),
    line(
        "menu.single_traffic",
        Menu,
        "Одна подписка: трафик с лимитом",
        "Трафик: {used} из {limit}",
        &[USED, LIMIT],
    ),
    line(
        "menu.single_traffic_unlimited",
        Menu,
        "Одна подписка: трафик без лимита",
        "Трафик: без лимита",
        &[],
    ),
    line(
        "menu.single_devices",
        Menu,
        "Одна подписка: устройства",
        "Устройства: {count} из {limit}",
        &[COUNT, LIMIT],
    ),
    line(
        "menu.single_devices_limit",
        Menu,
        "Одна подписка: лимит устройств",
        "Устройств можно подключить: {limit}",
        &[LIMIT],
    ),
    line(
        "menu.balance",
        Menu,
        "Баланс, если оплата с баланса включена",
        "Баланс: {balance}",
        &[BALANCE],
    ),
    text(
        "menu.subscription",
        Menu,
        "Строка подписки, если их несколько",
        "{mark} {tariff}: {status}, до {date}",
        &[MARK, TARIFF, STATUS, DATE],
    ),
    text(
        "menu.subscription_open",
        Menu,
        "Строка подписки без даты, если их несколько",
        "{mark} {tariff}: {status}",
        &[MARK, TARIFF, STATUS],
    ),
    hideable(
        "menu.btn.subscription",
        Menu,
        "Кнопка «Подписка»",
        "Подписка",
    ),
    hideable(
        "menu.btn.trial",
        Menu,
        "Кнопка пробного периода",
        "Попробовать бесплатно",
    ),
    hideable(
        "menu.btn.buy",
        Menu,
        "Кнопка покупки, если подписки нет",
        "Купить подписку",
    ),
    hideable(
        "menu.btn.extend",
        Menu,
        "Кнопка продления",
        "Продлить или сменить тариф",
    ),
    hideable(
        "menu.btn.app",
        Menu,
        "Кнопка мини-приложения",
        "Открыть приложение",
    ),
    hideable("menu.btn.balance", Menu, "Кнопка «Баланс»", "Баланс"),
    hideable("menu.btn.promo", Menu, "Кнопка промокода", "Промокод"),
    hideable(
        "menu.btn.referral",
        Menu,
        "Кнопка приглашения друзей",
        "Пригласить друга",
    ),
    hideable("menu.btn.options", Menu, "Кнопка докупки", "Докупить"),
    hideable("menu.btn.contests", Menu, "Кнопка конкурсов", "Конкурсы"),
    hideable(
        "menu.btn.roulette",
        Menu,
        "Кнопка рулетки в мини-приложении",
        "Рулетка",
    ),
    hideable(
        "menu.btn.gifts",
        Menu,
        "Кнопка подарков",
        "Подарить подписку",
    ),
    hideable(
        "menu.btn.documents",
        Menu,
        "Кнопка «Документы»",
        "Документы",
    ),
    hideable("menu.btn.support", Menu, "Кнопка «Поддержка»", "Поддержка"),
    hideable(
        "menu.btn.email",
        Menu,
        "Кнопка «Вход по почте», если настроена «Почта» и открыт вход на сайт",
        "Вход по почте",
    ),
    hideable(
        "menu.btn.language",
        Menu,
        "Кнопка выбора языка, если включён английский",
        "Язык",
    ),
    text("lang.choose", Menu, "Выбор языка", "Выберите язык:", &[]),
    text(
        "status.active",
        Subscription,
        "Состояние: активна",
        "активна",
        &[],
    ),
    text(
        "status.trial",
        Subscription,
        "Состояние: пробный период",
        "пробный период",
        &[],
    ),
    text(
        "status.limited",
        Subscription,
        "Состояние: трафик закончился",
        "трафик закончился",
        &[],
    ),
    text(
        "status.expired",
        Subscription,
        "Состояние: истекла",
        "истекла",
        &[],
    ),
    text(
        "status.disabled",
        Subscription,
        "Состояние: заблокирована",
        "заблокирована",
        &[],
    ),
    text(
        "status.pending",
        Subscription,
        "Состояние: оформляется",
        "оформляется",
        &[],
    ),
    text(
        "sub.none",
        Subscription,
        "Если подписки нет",
        "Подписки пока нет.",
        &[],
    ),
    text(
        "sub.choose",
        Subscription,
        "Выбор из нескольких подписок",
        "Выберите подписку:",
        &[],
    ),
    button(
        "sub.btn.item",
        Subscription,
        "Кнопка подписки в списке",
        "{tariff} · до {date}",
        &[TARIFF, DATE],
    ),
    button(
        "sub.btn.item_open",
        Subscription,
        "Кнопка подписки без даты",
        "{tariff}",
        &[TARIFF],
    ),
    text(
        "sub.not_found",
        Subscription,
        "Подписка не найдена",
        "Подписка не найдена.",
        &[],
    ),
    text(
        "sub.title",
        Subscription,
        "Заголовок подписки",
        "<b>{tariff}</b> · {status}",
        &[TARIFF, STATUS],
    ),
    line(
        "sub.until",
        Subscription,
        "Срок",
        "До {date}, осталось {days}",
        &[DATE, DAYS],
    ),
    line(
        "sub.ended",
        Subscription,
        "Срок закончился",
        "Закончилась {date}",
        &[DATE],
    ),
    line(
        "sub.traffic",
        Subscription,
        "Трафик с лимитом",
        "Трафик: {used} из {limit}{reset}",
        &[USED, LIMIT, RESET],
    ),
    text(
        "sub.traffic_reset_date",
        Subscription,
        "Когда обновится трафик",
        ", обновится {date}",
        &[DATE],
    ),
    text(
        "sub.traffic_reset_payment",
        Subscription,
        "Трафик обновится при продлении",
        ", обновится при продлении",
        &[],
    ),
    line(
        "sub.traffic_unlimited",
        Subscription,
        "Трафик без лимита",
        "Трафик: без лимита",
        &[],
    ),
    line(
        "sub.devices",
        Subscription,
        "Устройства",
        "Устройства: {count} из {limit}",
        &[COUNT, LIMIT],
    ),
    line(
        "sub.devices_limit",
        Subscription,
        "Лимит устройств",
        "Устройств можно подключить: {limit}",
        &[LIMIT],
    ),
    line(
        "sub.autorenew_on",
        Subscription,
        "Автопродление включено",
        "Автопродление: включено",
        &[],
    ),
    line(
        "sub.autorenew_off",
        Subscription,
        "Автопродление выключено",
        "Автопродление: выключено",
        &[],
    ),
    text(
        "sub.link",
        Subscription,
        "Ссылка подписки",
        "\nСсылка подписки:\n<code>{url}</code>",
        &[URL],
    ),
    text(
        "sub.link_error",
        Subscription,
        "Ссылку не получить",
        "\nСсылку сейчас не получить. {error}",
        &[ERROR],
    ),
    hideable(
        "sub.btn.connect",
        Subscription,
        "Кнопка страницы подключения",
        "Открыть страницу подключения",
    ),
    hideable(
        "sub.btn.copy_link",
        Subscription,
        "Кнопка «Скопировать ссылку»",
        "Скопировать ссылку",
    ),
    hideable(
        "sub.btn.devices",
        Subscription,
        "Кнопка «Устройства»",
        "Устройства",
    ),
    hideable(
        "sub.btn.reissue",
        Subscription,
        "Кнопка новой ссылки",
        "Выпустить новую ссылку",
    ),
    button(
        "sub.btn.extend",
        Subscription,
        "Кнопка «Продлить»",
        "Продлить",
        &[],
    ),
    button(
        "sub.btn.all",
        Subscription,
        "Кнопка «Все подписки»",
        "Все подписки",
        &[],
    ),
    button(
        "sub.btn.buy",
        Subscription,
        "Кнопка покупки",
        "Купить подписку",
        &[],
    ),
    text(
        "sub.reissue_confirm",
        Subscription,
        "Подтверждение новой ссылки",
        "Старая ссылка перестанет работать, а с ней все устройства. Нужные устройства подключите заново по новой ссылке.",
        &[],
    ),
    button(
        "sub.btn.reissue_yes",
        Subscription,
        "Кнопка подтверждения новой ссылки",
        "Выпустить новую ссылку",
        &[],
    ),
    text(
        "sub.renewal_on",
        Subscription,
        "Автопродление через платёжку",
        "Автопродление: {provider} спишет {amount} около {date}",
        &[PAY_PROVIDER, AMOUNT, DATE],
    ),
    text(
        "sub.renewal_cancelling",
        Subscription,
        "Автопродление через платёжку отключается",
        "Автопродление: отключаем в {provider}",
        &[PAY_PROVIDER],
    ),
    hideable(
        "sub.btn.renewal_off",
        Subscription,
        "Кнопка отключения автопродления через платёжку",
        "Отключить автопродление",
    ),
    text(
        "sub.renewal_confirm",
        Subscription,
        "Подтверждение отключения автопродления через платёжку",
        "Отключить автопродление через {provider}? Больше списаний не будет, подписка доработает до {date}.",
        &[PAY_PROVIDER, DATE],
    ),
    button(
        "sub.btn.renewal_off_yes",
        Subscription,
        "Кнопка подтверждения отключения автопродления",
        "Отключить",
        &[],
    ),
    text(
        "sub.renewal_done",
        Subscription,
        "Автопродление через платёжку отключено",
        "Автопродление через {provider} отключено. Подписка действует до {date}.",
        &[PAY_PROVIDER, DATE],
    ),
    text(
        "sub.renewal_pending",
        Subscription,
        "Автопродление через платёжку ещё отключается",
        "Отключаем автопродление в {provider}. Если сразу не получится, панель повторит сама.",
        &[PAY_PROVIDER],
    ),
    text(
        "sub.reissued",
        Subscription,
        "Новая ссылка выпущена",
        "Новая ссылка подписки:\n<code>{url}</code>\nДобавьте её в приложение заново.",
        &[URL],
    ),
    text(
        "sub.trial_started",
        Subscription,
        "Пробный период оформляется",
        "Пробный период оформляется. Через минуту подписка появится в разделе «Подписка».",
        &[],
    ),
    text(
        "sub.trial_confirm",
        Subscription,
        "Проверка перед пробным периодом",
        "Чтобы получить пробный период, подтвердите, что вы не робот: нажмите кнопку ниже.",
        &[],
    ),
    button(
        "sub.btn.trial_confirm",
        Subscription,
        "Кнопка «Я не робот»",
        "Я не робот",
        &[],
    ),
    text(
        "sub.trial_choose",
        Subscription,
        "Выбор варианта пробного периода",
        "<b>Выберите пробный период</b>",
        &[],
    ),
    text(
        "sub.trial_variant_line",
        Subscription,
        "Вариант пробного периода в списке",
        "{variant} — {days}, устройств: {count}, трафик: {limit}",
        &[VARIANT, DAYS, COUNT, LIMIT],
    ),
    text(
        "sub.trial_variant_unlimited",
        Subscription,
        "Вариант пробного периода без лимита трафика",
        "{variant} — {days}, устройств: {count}, трафик без лимита",
        &[VARIANT, DAYS, COUNT],
    ),
    button(
        "sub.btn.trial_variant",
        Subscription,
        "Кнопка варианта пробного периода",
        "{variant} · {days}",
        &[VARIANT, DAYS],
    ),
    text(
        "sub.trial_verify",
        Subscription,
        "Проверка капчей перед пробным периодом",
        "Чтобы получить пробный период, пройдите проверку «Я не робот». Она откроется по кнопке ниже, пробный период включится сразу после неё.",
        &[],
    ),
    button(
        "sub.btn.trial_verify",
        Subscription,
        "Кнопка страницы проверки",
        "Пройти проверку",
        &[],
    ),
    text(
        "sub.trial_verify_off",
        Subscription,
        "Проверка не открывается",
        "Пробный период сейчас недоступен: страница проверки не открывается. Попробуйте позже или напишите в поддержку.",
        &[],
    ),
    text(
        "dev.empty",
        Devices,
        "Устройств нет",
        "Устройств пока нет: они появятся, когда вы добавите подписку в приложение.",
        &[],
    ),
    text("dev.title", Devices, "Заголовок", "<b>Устройства</b>", &[]),
    text(
        "dev.line",
        Devices,
        "Строка устройства",
        "{n}. {title} — {details}, добавлено {date}",
        &[NUMBER, TITLE, DETAILS, DATE],
    ),
    text(
        "dev.line_plain",
        Devices,
        "Строка устройства без подробностей",
        "{n}. {title}, добавлено {date}",
        &[NUMBER, TITLE, DATE],
    ),
    button(
        "dev.btn.disconnect",
        Devices,
        "Кнопка отключения устройства",
        "Отключить {title}",
        &[TITLE],
    ),
    text(
        "dev.confirm",
        Devices,
        "Подтверждение отключения",
        "Отключить {title}? Место освободится, но если подписка на нём осталась, при обновлении он займёт его снова. Чтобы закрыть доступ, выпустите новую ссылку.",
        &[TITLE],
    ),
    button(
        "dev.btn.disconnect_yes",
        Devices,
        "Кнопка подтверждения",
        "Отключить",
        &[],
    ),
    text(
        "dev.disconnected",
        Devices,
        "Устройство отключено",
        "{title} отключено.",
        &[TITLE],
    ),
    text(
        "dev.stale",
        Devices,
        "Список устарел",
        "Список устройств устарел. Откройте его заново.",
        &[],
    ),
    text(
        "buy.which",
        Buy,
        "Какую подписку продлить",
        "Какую подписку продлить?",
        &[],
    ),
    button(
        "buy.btn.extend_item",
        Buy,
        "Кнопка продления подписки из списка",
        "Продлить {tariff} · до {date}",
        &[TARIFF, DATE],
    ),
    button(
        "buy.btn.extend_item_open",
        Buy,
        "Кнопка продления без даты",
        "Продлить {tariff}",
        &[TARIFF],
    ),
    button(
        "buy.btn.new",
        Buy,
        "Кнопка новой подписки",
        "Новая подписка",
        &[],
    ),
    text(
        "buy.category_renew",
        Buy,
        "Подписка категории уже есть, оплата продлит её",
        "В категории «{category}» у вас уже есть подписка: оплата продлит её.",
        &[CATEGORY],
    ),
    text(
        "buy.category_choose",
        Buy,
        "Новую подписку категории оформить нельзя",
        "В категории «{category}» новую подписку оформить нельзя. Какую подписку продлить?",
        &[CATEGORY],
    ),
    text(
        "buy.empty",
        Buy,
        "Нет тарифов",
        "Сейчас нет тарифов в продаже.",
        &[],
    ),
    text(
        "buy.title",
        Buy,
        "Заголовок выбора тарифа",
        "<b>Выберите тариф</b>",
        &[],
    ),
    text(
        "buy.category",
        Buy,
        "Категория тарифов",
        "\n<b>{category}</b>",
        &[CATEGORY],
    ),
    text(
        "buy.tariff_line",
        Buy,
        "Тариф в списке",
        "{tariff} — от {per_month} в месяц",
        &[TARIFF, PER_MONTH],
    ),
    text(
        "buy.tariff_line_plain",
        Buy,
        "Тариф в списке без цены",
        "{tariff}",
        &[TARIFF],
    ),
    text(
        "buy.tariff_line_promo",
        Buy,
        "Тариф в списке по акции",
        "{tariff} — от <s>{full_per_month}</s> <b>{per_month}</b> в месяц, −{percent}% · {promotion} {ends}",
        &[
            TARIFF,
            PER_MONTH,
            FULL_PER_MONTH,
            PROMO_PERCENT,
            PROMOTION,
            PROMO_UNTIL,
        ],
    ),
    button(
        "buy.btn.tariff",
        Buy,
        "Кнопка тарифа",
        "{tariff}",
        &[TARIFF],
    ),
    text(
        "buy.tariff_title",
        Buy,
        "Заголовок тарифа",
        "<b>{tariff}</b>",
        &[TARIFF],
    ),
    line(
        "buy.tariff_devices",
        Buy,
        "Устройств в тарифе",
        "Устройств в тарифе: {count}",
        &[COUNT],
    ),
    line(
        "buy.tariff_traffic",
        Buy,
        "Трафик тарифа",
        "Трафик: {limit}, {reset}",
        &[LIMIT, RESET],
    ),
    line(
        "buy.tariff_unlimited",
        Buy,
        "Трафик без лимита",
        "Трафик: без лимита",
        &[],
    ),
    text(
        "buy.choose_period",
        Buy,
        "Выбор срока",
        "\nВыберите срок:",
        &[],
    ),
    button(
        "buy.btn.period",
        Buy,
        "Кнопка срока",
        "{days} — {price}",
        &[DAYS, PRICE],
    ),
    button(
        "buy.btn.period_monthly",
        Buy,
        "Кнопка срока с ценой в месяц",
        "{days} — {price}, {per_month} в месяц",
        &[DAYS, PRICE, PER_MONTH],
    ),
    button(
        "buy.btn.period_savings",
        Buy,
        "Кнопка срока с выгодой",
        "{days} — {price}, {per_month} в месяц, выгоднее на {savings}%",
        &[DAYS, PRICE, PER_MONTH, SAVINGS],
    ),
    text(
        "buy.promotion",
        Buy,
        "Акция на тарифе",
        "\n<b>{promotion}</b> {ends}: −{percent}%",
        &[PROMOTION, PROMO_UNTIL, PROMO_PERCENT],
    ),
    line(
        "buy.promotion_period",
        Buy,
        "Срок по акции",
        "{days} — <s>{full_price}</s> <b>{price}</b>",
        &[DAYS, PRICE, FULL_PRICE],
    ),
    button(
        "buy.btn.period_promo",
        Buy,
        "Кнопка срока по акции",
        "{days} — {price}, −{percent}%",
        &[DAYS, PRICE, PROMO_PERCENT],
    ),
    text(
        "buy.extra_devices",
        Buy,
        "Предложение доп. устройств",
        "В тарифе {included}. Добавить ещё? Каждое — {price} в месяц.",
        &[INCLUDED, PRICE],
    ),
    button(
        "buy.btn.no_extra",
        Buy,
        "Кнопка без доп. устройств",
        "Без доп. устройств",
        &[],
    ),
    button(
        "buy.btn.extra",
        Buy,
        "Кнопка числа доп. устройств",
        "+{count}",
        &[COUNT],
    ),
    text(
        "buy.device_discounts",
        Buy,
        "Скидка за доп. устройства",
        "Скидка за доп. устройства: {steps}.",
        &[STEPS],
    ),
    text(
        "buy.summary_title",
        Buy,
        "Заголовок оплаты",
        "<b>Оплата</b>",
        &[],
    ),
    text(
        "buy.summary_line",
        Buy,
        "Строка счёта",
        "{label} — {amount}",
        &[LABEL, AMOUNT],
    ),
    text(
        "buy.summary_total",
        Buy,
        "Итого",
        "К оплате: <b>{total}</b>",
        &[TOTAL],
    ),
    text(
        "buy.summary_total_stars",
        Buy,
        "Итого со звёздами",
        "К оплате: <b>{total}</b> (или {stars})",
        &[TOTAL, STARS],
    ),
    text(
        "buy.summary_renewal",
        Buy,
        "Продление",
        "Продление: подписка будет действовать до {date}.",
        &[DATE],
    ),
    text(
        "buy.summary_change",
        Buy,
        "Переход на тариф другой категории",
        "<b>Будет переход на тариф «{tariff}».</b> Текущая подписка ({current}) будет удалена, новая станет доступна сразу после оплаты и будет действовать до {date}. Оставшиеся дни не переносятся, ссылка подписки останется прежней.",
        &[TARIFF, CURRENT, DATE],
    ),
    text(
        "buy.summary_from_trial",
        Buy,
        "Оплата во время пробного периода",
        "Пробный период закончится сразу после оплаты, тариф «{tariff}» будет действовать до {date}.",
        &[TARIFF, DATE],
    ),
    text(
        "buy.summary_new",
        Buy,
        "Новая подписка",
        "Подписка будет действовать до {date}.",
        &[DATE],
    ),
    button(
        "buy.btn.method",
        Buy,
        "Кнопка способа оплаты",
        "{method} — {amount}",
        &[METHOD, AMOUNT],
    ),
    line(
        "buy.balance_short",
        Buy,
        "На балансе не хватает",
        "С баланса: не хватает {missing} (на балансе {balance})",
        &[MISSING, BALANCE],
    ),
    text(
        "buy.no_methods",
        Buy,
        "Нет способов оплаты",
        "Сейчас нет доступных способов оплаты.",
        &[],
    ),
    text(
        "buy.promo_removed",
        Buy,
        "Промокод не подошёл",
        "{error} Промокод убран.",
        &[ERROR],
    ),
    text(
        "buy.promo_beaten",
        Buy,
        "Промокод меньше скидки по акции",
        "Промокод не применён: скидка по акции «{promotion}» больше.",
        &[PROMOTION],
    ),
    text(
        "buy.promotion_beaten",
        Buy,
        "Промокод больше скидки по акции",
        "Применён промокод: он выгоднее акции «{promotion}».",
        &[PROMOTION],
    ),
    hideable(
        "buy.btn.change_devices",
        Buy,
        "Кнопка «Изменить доп. устройства» перед оплатой",
        "Изменить доп. устройства",
    ),
    hideable(
        "buy.btn.promo",
        Buy,
        "Кнопка ввода промокода",
        "Ввести промокод",
    ),
    button(
        "buy.btn.promo_remove",
        Buy,
        "Кнопка «Убрать промокод»",
        "Убрать промокод",
        &[],
    ),
    text(
        "buy.ask_promo",
        Buy,
        "Просьба ввести промокод",
        "Введите промокод:",
        &[],
    ),
    text(
        "buy.email_ask",
        Buy,
        "Просьба прислать почту для чека",
        "Для оплаты через {provider} нужна почта: на неё придут чек и письма об оплате. Пришлите почту одним сообщением.",
        &[PAY_PROVIDER],
    ),
    text(
        "buy.email_change_ask",
        Buy,
        "Просьба прислать новую почту для чеков",
        "Пришлите новую почту одним сообщением: на неё будут приходить чеки.",
        &[],
    ),
    text(
        "buy.email_invalid",
        Buy,
        "Почта для чека с опечаткой",
        "Похоже, в почте опечатка. Пришлите адрес вида name@example.com.",
        &[],
    ),
    text(
        "buy.email_line",
        Buy,
        "Куда придёт чек",
        "Чек придёт на {email}",
        &[EMAIL],
    ),
    button(
        "buy.btn.email_change",
        Buy,
        "Изменить почту для чеков",
        "Изменить почту",
        &[],
    ),
    button(
        "buy.btn.promo_activate",
        Buy,
        "Кнопка активации промокода на дни",
        "Активировать промокод",
        &[],
    ),
    button(
        "buy.btn.back_to_payment",
        Buy,
        "Кнопка возврата к оплате",
        "Вернуться к оплате",
        &[],
    ),
    button(
        "buy.btn.promo_other",
        Buy,
        "Кнопка другого промокода",
        "Ввести другой",
        &[],
    ),
    button(
        "buy.btn.no_promo",
        Buy,
        "Кнопка «Без промокода»",
        "Без промокода",
        &[],
    ),
    text(
        "pay.what_tariff",
        Payment,
        "Что оплачивается: тариф",
        "{tariff}, {term}.",
        &[TARIFF, TERM],
    ),
    text(
        "pay.what_topup",
        Payment,
        "Что оплачивается: пополнение",
        "Пополнение на {amount}.",
        &[AMOUNT],
    ),
    text(
        "pay.what_topup_bonus",
        Payment,
        "Что оплачивается: пополнение с бонусом",
        "Пополнение на {amount}. На баланс придёт {total}: {amount} и бонус {bonus}.",
        &[AMOUNT, TOTAL_BONUS, TOPUP_BONUS],
    ),
    text(
        "pay.ready",
        Payment,
        "Счёт готов",
        "{what}\nСчёт готов: нажмите «Оплатить». После оплаты пришлём подтверждение сюда.",
        &[WHAT],
    ),
    text(
        "pay.link",
        Payment,
        "Ссылка на оплату без кнопки",
        "{what}\nСсылка на оплату: {url}",
        &[WHAT, URL],
    ),
    button("pay.btn.pay", Payment, "Кнопка «Оплатить»", "Оплатить", &[]),
    text(
        "pay.gram_ready",
        Payment,
        "Счёт в GRAM готов",
        "{what}\nСчёт готов — оплатите его в сообщении с кнопкой «Оплатить в Telegram».",
        &[WHAT],
    ),
    text(
        "pay.gram_caption",
        Payment,
        "Подпись к оплате кошельком Telegram",
        "{what}\nК оплате {amount}. Кошелёк Telegram откроется с готовой суммой и комментарием. Если кошелька в Telegram нет, нажмите «Другой кошелёк».",
        &[WHAT, AMOUNT],
    ),
    text(
        "pay.gram_what",
        Payment,
        "Что оплачивается, если тариф неизвестен",
        "Подписка.",
        &[],
    ),
    button(
        "pay.btn.gram_other",
        Payment,
        "Кнопка «Другой кошелёк»",
        "Другой кошелёк",
        &[],
    ),
    text(
        "pay.balance_done",
        Payment,
        "Оплачено с баланса",
        "Оплачено с баланса. Подписка обновится в течение минуты.",
        &[],
    ),
    text(
        "pay.stars_done",
        Payment,
        "Оплата звёздами получена",
        "Оплата звёздами получена. Подписка обновится в течение минуты.",
        &[],
    ),
    text(
        "pay.stars_repeat",
        Payment,
        "Повторная оплата звёздами",
        "Оплата звёздами получена, но этот счёт уже был оплачен. Повторную оплату проверим вручную и напишем вам.",
        &[],
    ),
    text(
        "pay.stars_deferred",
        Payment,
        "Оплата звёздами отложена",
        "Оплату получили, но не смогли сразу её провести. Проверим и выдадим подписку; если она не появится в течение часа, напишите /paysupport.",
        &[],
    ),
    text(
        "pay.paysupport",
        Payment,
        "Вопросы по оплате звёздами",
        "Вопросы по оплате звёздами Telegram: напишите в поддержку, ответим в течение дня. Если подписка после оплаты не выдана, вернём звёзды.",
        &[],
    ),
    button(
        "pay.btn.write_support",
        Payment,
        "Кнопка «Написать в поддержку»",
        "Написать в поддержку",
        &[],
    ),
    button(
        "pay.btn.support_bot",
        Payment,
        "Кнопка «Поддержка в боте»",
        "Поддержка в боте",
        &[],
    ),
    text(
        "bal.amount",
        Balance,
        "Сумма на балансе",
        "На балансе: <b>{amount}</b>",
        &[AMOUNT],
    ),
    line(
        "bal.referral",
        Balance,
        "Реферальный баланс",
        "Реферальный баланс: {amount} (его можно вывести в разделе «Пригласить друга»)",
        &[AMOUNT],
    ),
    line(
        "bal.note",
        Balance,
        "Пояснение",
        "С баланса оплачивается вся сумма сразу, без доплаты картой.",
        &[],
    ),
    line(
        "bal.off_note",
        Balance,
        "Пояснение, когда пополнение выключено",
        "Пополнить баланс сейчас нельзя. Остатком можно оплатить подписку, если его хватает на всю сумму.",
        &[],
    ),
    button(
        "bal.btn.preset",
        Balance,
        "Кнопка суммы пополнения",
        "+{amount}",
        &[AMOUNT],
    ),
    button(
        "bal.btn.custom",
        Balance,
        "Кнопка «Другая сумма»",
        "Другая сумма",
        &[],
    ),
    text(
        "bal.bad_amount",
        Balance,
        "Сумма не подходит",
        "{error} Введите другую сумму.",
        &[ERROR],
    ),
    text(
        "bal.no_methods",
        Balance,
        "Пополнение недоступно",
        "Пополнение сейчас недоступно: не подключена ни одна платёжная система.",
        &[],
    ),
    text(
        "bal.choose_method",
        Balance,
        "Выбор способа пополнения",
        "Пополнение на {amount}. Выберите способ оплаты:",
        &[AMOUNT],
    ),
    button(
        "bal.btn.method",
        Balance,
        "Кнопка способа пополнения",
        "{method}",
        &[METHOD],
    ),
    button(
        "bal.btn.method_hint",
        Balance,
        "Кнопка способа с пояснением",
        "{method} · {hint}",
        &[METHOD, HINT],
    ),
    text(
        "bal.enter_amount",
        Balance,
        "Просьба ввести сумму",
        "Введите сумму пополнения, {example}.",
        &[EXAMPLE],
    ),
    text(
        "bal.not_understood",
        Balance,
        "Сумма не распознана",
        "Сумма не распознана. Введите число, например 10.",
        &[],
    ),
    text(
        "sup.prompt",
        Support,
        "Просьба описать вопрос",
        "Опишите вопрос одним сообщением — ответим здесь, в боте.",
        &[],
    ),
    hideable(
        "sup.btn.other",
        Support,
        "Кнопка других способов связи",
        "Другие способы связи",
    ),
    text(
        "sup.link",
        Support,
        "Поддержка по ссылке",
        "Напишите нам — кнопка ниже откроет чат поддержки.",
        &[],
    ),
    button(
        "sup.btn.write",
        Support,
        "Кнопка перехода в чат поддержки",
        "Написать в поддержку",
        &[],
    ),
    text(
        "sup.sent",
        Support,
        "Обращение отправлено",
        "Обращение №{id} отправлено. Ответим здесь, в боте.",
        &[ID],
    ),
    text(
        "sup.added",
        Support,
        "Сообщение добавлено в открытое обращение",
        "Сообщение добавлено в обращение №{id}. Ответим здесь, в боте.",
        &[ID],
    ),
    text(
        "sup.reply_prompt",
        Support,
        "Просьба написать ответ в обращение",
        "Напишите ответ одним сообщением — он попадёт в обращение №{id}.",
        &[ID],
    ),
    text(
        "sup.file_big",
        Support,
        "Файл в обращение слишком большой",
        "Файл больше {mb} МБ, такой не примем. Пришлите файл поменьше.",
        &[MEGABYTES],
    ),
    text(
        "sup.file_type",
        Support,
        "Файл в обращение не подходит",
        "Такой файл не примем. Подойдут фото JPG, PNG, WebP, GIF и файлы PDF, TXT, LOG.",
        &[],
    ),
    text(
        "sup.file_off",
        Support,
        "Файлы в обращения не принимаются",
        "Файлы в обращения сейчас не принимаются. Опишите вопрос текстом.",
        &[],
    ),
    text(
        "sup.file_failed",
        Support,
        "Файл не получилось скачать",
        "Не получилось получить файл. Пришлите его ещё раз.",
        &[],
    ),
    text(
        "promo.prompt",
        Promo,
        "Просьба ввести промокод",
        "Введите промокод. Скидку применим при оплате, а бесплатные дни сразу добавим к подписке.",
        &[],
    ),
    text(
        "promo.saved",
        Promo,
        "Промокод на скидку сохранён",
        "{code} сохранён: скидка применится при оплате.",
        &[CODE],
    ),
    button(
        "promo.btn.choose_tariff",
        Promo,
        "Кнопка «Выбрать тариф»",
        "Выбрать тариф",
        &[],
    ),
    text(
        "promo.which_sub",
        Promo,
        "К какой подписке добавить дни",
        "К какой подписке добавить дни?",
        &[],
    ),
    text(
        "promo.days_added",
        Promo,
        "Дни добавлены",
        "Добавили {days} к подписке.",
        &[DAYS],
    ),
    text(
        "ref.title",
        Referral,
        "Заголовок",
        "<b>Пригласить друга</b>",
        &[],
    ),
    text(
        "ref.percent",
        Referral,
        "Условия",
        "Ваш процент с каждой оплаты:\n{percent}% — друзья",
        &[PERCENT],
    ),
    line(
        "ref.level2",
        Referral,
        "Второй уровень",
        "{percent}% — их друзья",
        &[LEVEL2_PERCENT],
    ),
    line(
        "ref.level3",
        Referral,
        "Третий уровень",
        "{percent}% — друзья их друзей",
        &[LEVEL3_PERCENT],
    ),
    text(
        "ref.link",
        Referral,
        "Ссылка-приглашение",
        "<code>{link}</code>",
        &[LINK],
    ),
    line(
        "ref.invited",
        Referral,
        "Приглашено",
        "\nПриглашено: {count}",
        &[COUNT],
    ),
    line(
        "ref.earned",
        Referral,
        "Заработано",
        "Заработано: {amount}",
        &[AMOUNT],
    ),
    line(
        "ref.balance",
        Referral,
        "На реферальном балансе",
        "На реферальном балансе: {amount}",
        &[AMOUNT],
    ),
    text(
        "ref.withdrawal",
        Referral,
        "Заявка на вывод",
        "Заявка №{id} на {amount}: {status}",
        &[ID, AMOUNT, STATUS],
    ),
    button(
        "ref.btn.cancel_withdrawal",
        Referral,
        "Кнопка отмены заявки",
        "Отменить заявку №{id}",
        &[ID],
    ),
    button(
        "ref.btn.withdraw",
        Referral,
        "Кнопка «Вывести деньги»",
        "Вывести деньги",
        &[],
    ),
    hideable(
        "ref.btn.share",
        Referral,
        "Кнопка «Поделиться ссылкой»",
        "Поделиться ссылкой",
    ),
    text(
        "ref.how",
        Referral,
        "Выбор способа вывода",
        "Как получить деньги?",
        &[],
    ),
    text(
        "ref.amount",
        Referral,
        "Сколько вывести",
        "Сколько вывести? На реферальном балансе {amount}.{minimum}",
        &[AMOUNT, MINIMUM],
    ),
    text(
        "ref.minimum",
        Referral,
        "Минимум для вывода",
        " Минимум — {amount}.",
        &[AMOUNT],
    ),
    text(
        "ref.cancelled",
        Referral,
        "Заявка отменена",
        "Заявка №{id} отменена, {amount} вернулись на реферальный баланс.",
        &[ID, AMOUNT],
    ),
    text(
        "ref.details",
        Referral,
        "Реквизиты",
        "Укажите реквизиты. {hint}",
        &[HINT],
    ),
    text(
        "ref.requested",
        Referral,
        "Заявка принята",
        "Заявка №{id} на {amount} принята. Сумма списана с реферального баланса и вернётся, если заявку отклонят.",
        &[ID, AMOUNT],
    ),
    button(
        "ref.btn.retry",
        Referral,
        "Кнопка «Попробовать снова»",
        "Попробовать снова",
        &[],
    ),
    text(
        "contest.list",
        Contests,
        "Список конкурсов",
        "<b>Конкурсы</b>\nВыберите конкурс:",
        &[],
    ),
    text(
        "contest.none",
        Contests,
        "Конкурсов нет",
        "Сейчас конкурсов нет. Когда начнётся новый, бот пришлёт сообщение.",
        &[],
    ),
    text(
        "contest.title",
        Contests,
        "Заголовок конкурса",
        "<b>{title}</b>",
        &[TITLE],
    ),
    text(
        "contest.prize",
        Contests,
        "Приз",
        "Приз: {prize}. Победителей: {winners}.",
        &[PRIZE, WINNERS],
    ),
    text(
        "contest.prizes",
        Contests,
        "Заголовок призов по местам",
        "Призы:",
        &[],
    ),
    text(
        "contest.prize_line",
        Contests,
        "Приз за места",
        "{places} — {prize}",
        &[PLACES, PRIZE],
    ),
    text(
        "contest.winners",
        Contests,
        "Число победителей при призах по местам",
        "Победителей: {winners}.",
        &[WINNERS],
    ),
    text(
        "contest.until",
        Contests,
        "Когда итоги",
        "Итоги — {date}.",
        &[DATE],
    ),
    text(
        "contest.conditions",
        Contests,
        "Заголовок условий",
        "\n<b>Условия</b>",
        &[],
    ),
    text(
        "contest.condition_ok",
        Contests,
        "Выполненное условие",
        "✅ {condition}",
        &[CONDITION],
    ),
    text(
        "contest.condition",
        Contests,
        "Условие после итогов",
        "• {condition}",
        &[CONDITION],
    ),
    text(
        "contest.condition_no",
        Contests,
        "Невыполненное условие",
        "▫️ {condition}",
        &[CONDITION],
    ),
    text(
        "contest.open",
        Contests,
        "Без условий",
        "Участвовать может любой клиент.",
        &[],
    ),
    line(
        "contest.entries",
        Contests,
        "Число участников",
        "\nУчастников: {count}",
        &[COUNT],
    ),
    text(
        "contest.joined",
        Contests,
        "Клиент участвует",
        "\nВы участвуете. Итоги придут сюда, в бот.",
        &[],
    ),
    text(
        "contest.joined_now",
        Contests,
        "Клиент только что вступил",
        "Готово, вы участвуете в конкурсе «{title}». Итоги — {date}.",
        &[TITLE, DATE],
    ),
    text(
        "contest.not_eligible",
        Contests,
        "Условия не выполнены",
        "\nЧтобы участвовать, выполните условия, отмеченные ▫️, и нажмите «Участвовать» ещё раз.",
        &[],
    ),
    text(
        "contest.closed",
        Contests,
        "Приём закончился",
        "Приём участников в этом конкурсе закончился.",
        &[],
    ),
    text(
        "contest.confirm",
        Contests,
        "Проверка перед участием",
        "Чтобы участвовать в конкурсе, подтвердите, что вы не робот: нажмите кнопку ниже.",
        &[],
    ),
    text(
        "contest.denied",
        Contests,
        "Участие недоступно",
        "Участие в конкурсах для этого аккаунта недоступно. Если это ошибка, напишите в поддержку.",
        &[],
    ),
    line(
        "contest.fair",
        Contests,
        "Отпечаток жеребьёвки",
        "\nОтпечаток жеребьёвки: <code>{hash}</code>",
        &[HASH],
    ),
    text(
        "contest.drawing",
        Contests,
        "Подводим итоги",
        "\nПриём закончился, бот подводит итоги.",
        &[],
    ),
    text(
        "contest.won",
        Contests,
        "Клиент победил",
        "\nВы победили! Приз: {prize}. {result}",
        &[PRIZE, RESULT],
    ),
    text(
        "contest.won_place",
        Contests,
        "Клиент занял призовое место",
        "\nВы заняли {place} место! Приз: {prize}. {result}",
        &[PLACE, PRIZE, RESULT],
    ),
    text(
        "contest.won_code",
        Contests,
        "Промокод победителя",
        "Ваш промокод: <code>{code}</code> — введите его при оплате.",
        &[CODE],
    ),
    text(
        "contest.lost",
        Contests,
        "Клиент не победил",
        "\nИтоги подведены. В этот раз приз достался другим участникам — спасибо, что участвовали!",
        &[],
    ),
    text(
        "contest.excluded",
        Contests,
        "Клиент не прошёл проверку",
        "\nИтоги подведены. В розыгрыш вы не попали: {comment}.",
        &[COMMENT],
    ),
    button(
        "contest.btn.join",
        Contests,
        "Кнопка «Участвовать»",
        "Участвовать",
        &[],
    ),
    button(
        "contest.btn.confirm",
        Contests,
        "Кнопка «Я не робот»",
        "Я не робот",
        &[],
    ),
    button(
        "contest.btn.open",
        Contests,
        "Кнопка конкурса в списке",
        "{title}",
        &[TITLE],
    ),
    hideable(
        "contest.btn.buy",
        Contests,
        "Кнопка «Купить подписку»",
        "Купить подписку",
    ),
    hideable(
        "contest.btn.invite",
        Contests,
        "Кнопка «Пригласить друга»",
        "Пригласить друга",
    ),
    button(
        "contest.btn.all",
        Contests,
        "Кнопка «Все конкурсы»",
        "Все конкурсы",
        &[],
    ),
    text(
        "opt.no_subscription",
        Options,
        "Нет подписки",
        "Докупить устройства или трафик можно к действующей подписке.",
        &[],
    ),
    text(
        "opt.which_sub",
        Options,
        "К какой подписке докупить",
        "К какой подписке докупить?",
        &[],
    ),
    text(
        "opt.menu",
        Options,
        "Что докупить",
        "Подписка действует до {date}. Что докупить?",
        &[DATE],
    ),
    button(
        "opt.btn.devices",
        Options,
        "Кнопка устройств",
        "Устройства",
        &[],
    ),
    button("opt.btn.traffic", Options, "Кнопка трафика", "Трафик", &[]),
    text(
        "opt.max",
        Options,
        "Больше не добавить",
        "К этой подписке больше устройств не докупить.",
        &[],
    ),
    text(
        "opt.offer",
        Options,
        "Устройства",
        "Сейчас {included} в тарифе и {extra} докуплено, можно добавить ещё {can_add}.\nДо конца подписки {days}, одно устройство на этот срок стоит {price}{stars}.",
        &[INCLUDED, EXTRA, CAN_ADD, DAYS, PRICE, STARS],
    ),
    text(
        "opt.discounts",
        Options,
        "Скидка за доп. устройства",
        "Скидка за доп. устройства: {steps}. Ступень считается по числу доп. устройств у подписки после покупки.",
        &[STEPS],
    ),
    text(
        "opt.stars",
        Options,
        "Цена в звёздах",
        " или {stars}",
        &[STARS],
    ),
    button(
        "opt.btn.count",
        Options,
        "Кнопка числа устройств",
        "+{count}",
        &[COUNT],
    ),
    text(
        "opt.traffic",
        Options,
        "Трафик",
        "Израсходовано {used} из {limit}. Докупленный трафик расходуется после основного.\n{note}\nВыберите пакет:",
        &[USED, LIMIT, NOTE],
    ),
    text(
        "opt.traffic_burn_payment",
        Options,
        "Остаток сгорает при продлении",
        "Что не израсходуете до продления, сгорит.",
        &[],
    ),
    text(
        "opt.traffic_carry_payment",
        Options,
        "Остаток переносится при продлении",
        "Неизрасходованный остаток сохранится после продления.",
        &[],
    ),
    text(
        "opt.traffic_burn_reset",
        Options,
        "Остаток сгорает при сбросе",
        "Что не израсходуете до сброса трафика, сгорит.",
        &[],
    ),
    text(
        "opt.traffic_carry_reset",
        Options,
        "Остаток переносится после сброса",
        "Неизрасходованный остаток перейдёт на следующий период после сброса трафика.",
        &[],
    ),
    button(
        "opt.btn.pack",
        Options,
        "Кнопка пакета трафика",
        "+{gb} ГБ · {price}",
        &[GB, PRICE],
    ),
    text(
        "opt.quote",
        Options,
        "Расчёт",
        "{label} — {total}{stars}.\nКак оплатить?",
        &[LABEL, TOTAL, STARS],
    ),
    text(
        "gift.intro",
        Gifts,
        "Что такое подарок",
        "<b>Подписка в подарок</b>\nВыберите тариф и срок, оплатите — и получите код. Друг активирует его в этом боте или в кабинете на сайте.",
        &[],
    ),
    button(
        "gift.btn.buy",
        Gifts,
        "Кнопка «Выбрать подарок»",
        "Выбрать подарок",
        &[],
    ),
    button(
        "gift.btn.mine",
        Gifts,
        "Кнопка «Мои подарки»",
        "Мои подарки",
        &[],
    ),
    button(
        "gift.btn.redeem",
        Gifts,
        "Кнопка «У меня есть код»",
        "У меня есть код",
        &[],
    ),
    text(
        "gift.choose_tariff",
        Gifts,
        "Выбор тарифа",
        "Какую подписку подарить?",
        &[],
    ),
    text(
        "gift.no_tariffs",
        Gifts,
        "Нечего подарить",
        "Сейчас подарить подписку нельзя: нет тарифов для подарка.",
        &[],
    ),
    text(
        "gift.choose_period",
        Gifts,
        "Выбор срока",
        "Подписка «{tariff}». На какой срок подарить?",
        &[TARIFF],
    ),
    button(
        "gift.btn.period",
        Gifts,
        "Кнопка срока",
        "{term} · {price}",
        &[TERM, PRICE],
    ),
    button(
        "gift.btn.period_promo",
        Gifts,
        "Кнопка срока по акции",
        "{term} · {price}, −{percent}%",
        &[TERM, PRICE, PROMO_PERCENT],
    ),
    text(
        "gift.quote",
        Gifts,
        "Расчёт",
        "{label} — {total}{stars}.\nКод для друга придёт сюда сразу после оплаты. Как оплатить?",
        &[LABEL, TOTAL, STARS],
    ),
    text(
        "gift.stars",
        Gifts,
        "Цена в звёздах",
        " или {stars}",
        &[STARS],
    ),
    text(
        "gift.mine_title",
        Gifts,
        "Заголовок списка",
        "<b>Ваши подарки</b>",
        &[],
    ),
    text(
        "gift.mine_empty",
        Gifts,
        "Подарков нет",
        "Вы ещё ничего не дарили.",
        &[],
    ),
    text(
        "gift.mine_item",
        Gifts,
        "Подарок в списке",
        "<code>{code}</code> — «{tariff}» на {term}\n{status}",
        &[GIFT_CODE, TARIFF, TERM, STATUS],
    ),
    text(
        "gift.mine_link",
        Gifts,
        "Ссылка для друга в списке",
        "Ссылка для друга: {link}",
        &[GIFT_LINK],
    ),
    text(
        "gift.state_active",
        Gifts,
        "Ждёт активации",
        "Ждёт активации до {date}",
        &[DATE],
    ),
    text(
        "gift.state_active_open",
        Gifts,
        "Ждёт активации, без срока",
        "Ждёт активации",
        &[],
    ),
    text(
        "gift.state_redeemed",
        Gifts,
        "Активирован",
        "Активирован {date}",
        &[DATE],
    ),
    text(
        "gift.state_expired",
        Gifts,
        "Срок кода истёк",
        "Срок кода истёк {date}",
        &[DATE],
    ),
    text(
        "gift.state_canceled",
        Gifts,
        "Отменён возвратом",
        "Отменён: деньги вернули",
        &[],
    ),
    text(
        "gift.ask_code",
        Gifts,
        "Просьба прислать код",
        "Пришлите код подарка одним сообщением. Он выглядит так: ABCD-EFGH-JKLM.",
        &[],
    ),
    text(
        "gift.preview",
        Gifts,
        "Что в подарке",
        "<b>Подарок:</b> подписка «{tariff}» на {term}.",
        &[TARIFF, TERM],
    ),
    text(
        "gift.plan_new",
        Gifts,
        "Будет новая подписка",
        "Подписка начнёт работать сразу после активации.",
        &[],
    ),
    text(
        "gift.plan_separate",
        Gifts,
        "Будет ещё одна подписка",
        "Это будет отдельная подписка со своей ссылкой, текущие останутся как есть.",
        &[],
    ),
    text(
        "gift.plan_renewal",
        Gifts,
        "Продлит подписку",
        "Срок добавится к вашей подписке «{current}».",
        &[CURRENT],
    ),
    text(
        "gift.plan_change",
        Gifts,
        "Заменит подписку",
        "Будет переход на тариф «{tariff}». Текущая подписка ({current}) будет заменена, оставшиеся дни не переносятся.",
        &[TARIFF, CURRENT],
    ),
    text(
        "gift.plan_trial",
        Gifts,
        "Заменит пробный период",
        "Пробный период закончится, подписка «{tariff}» начнёт работать сразу.",
        &[TARIFF],
    ),
    text(
        "gift.own",
        Gifts,
        "Свой подарок",
        "Это ваш подарок: после активации код другу уже не пригодится.",
        &[],
    ),
    text(
        "gift.choose_target",
        Gifts,
        "Куда зачесть подарок",
        "Продлить одну из подписок или оформить новую?",
        &[],
    ),
    text(
        "gift.plan_limit",
        Gifts,
        "Новую подписку этой категории оформить нельзя",
        "Новую подписку этой категории оформить нельзя: срок добавится к одной из ваших подписок.",
        &[],
    ),
    text(
        "gift.choose_existing",
        Gifts,
        "Какую подписку продлить подарком",
        "Какую подписку продлить?",
        &[],
    ),
    button(
        "gift.btn.activate",
        Gifts,
        "Кнопка «Активировать»",
        "Активировать",
        &[],
    ),
    button(
        "gift.btn.extend",
        Gifts,
        "Кнопка «Продлить подписку»",
        "Продлить «{tariff}»",
        &[TARIFF],
    ),
    button(
        "gift.btn.new",
        Gifts,
        "Кнопка «Новая подписка»",
        "Новая подписка",
        &[],
    ),
    text(
        "gift.paid_balance",
        Gifts,
        "Подарок оплачен с баланса",
        "Оплачено с баланса. Код подарка придёт сюда через минуту.",
        &[],
    ),
    text(
        "gift.redeemed",
        Gifts,
        "Подарок активирован",
        "Подарок активирован: «{tariff}» на {term}. Подписка будет готова через минуту — пришлём сообщение.",
        &[TARIFF, TERM],
    ),
    text(
        "pl.done",
        PromoLinks,
        "Подписка по ссылке оформлена",
        "Подписка на {days} оформлена. Пришлём сообщение, как только она заработает.",
        &[DAYS],
    ),
    text(
        "pl.not_found",
        PromoLinks,
        "Ссылки нет",
        "Такой промо-ссылки нет. Проверьте ссылку.",
        &[],
    ),
    text(
        "pl.disabled",
        PromoLinks,
        "Ссылка выключена",
        "Эта промо-ссылка больше не работает.",
        &[],
    ),
    text(
        "pl.expired",
        PromoLinks,
        "Срок ссылки истёк",
        "Срок действия этой промо-ссылки закончился.",
        &[],
    ),
    text(
        "pl.exhausted",
        PromoLinks,
        "Активации закончились",
        "Подписки по этой ссылке закончились: её уже активировали все, кто успел.",
        &[],
    ),
    text(
        "pl.already_used",
        PromoLinks,
        "Уже активирована",
        "Вы уже получили подписку по этой ссылке. Одна ссылка срабатывает один раз.",
        &[],
    ),
    text(
        "pl.has_subscription",
        PromoLinks,
        "Уже есть подписка",
        "У вас уже есть действующая подписка, а эта ссылка выдаёт новую. Воспользоваться ею можно будет, когда подписка закончится.",
        &[],
    ),
    text(
        "pl.confirm",
        PromoLinks,
        "Проверка перед подпиской по ссылке",
        "Чтобы получить подписку по промо-ссылке, подтвердите, что вы не робот: нажмите кнопку ниже.",
        &[],
    ),
    button(
        "pl.btn.confirm",
        PromoLinks,
        "Кнопка «Я не робот»",
        "Я не робот",
        &[],
    ),
    text(
        "pl.denied",
        PromoLinks,
        "Подписка по ссылке недоступна",
        "Подписка по промо-ссылкам для этого аккаунта недоступна. Если это ошибка, напишите в поддержку.",
        &[],
    ),
    text(
        "pl.again",
        PromoLinks,
        "Откройте ссылку ещё раз",
        "Откройте промо-ссылку ещё раз, чтобы получить подписку.",
        &[],
    ),
    text(
        "pay.what_devices",
        Payment,
        "Что оплачивается: устройства",
        "доп. устройства: +{count}",
        &[COUNT],
    ),
    text(
        "pay.what_gift",
        Payment,
        "Что оплачивается: подарок",
        "подарок: «{tariff}» на {term}",
        &[TARIFF, TERM],
    ),
    text(
        "pay.what_traffic",
        Payment,
        "Что оплачивается: трафик",
        "трафик: +{gb} ГБ",
        &[GB],
    ),
    text(
        "login.title",
        Login,
        "Заголовок",
        "<b>Вход в кабинет</b>",
        &[],
    ),
    text("login.account", Login, "Аккаунт", "Аккаунт: {who}", &[WHO]),
    line("login.ip", Login, "Откуда вход", "Откуда: {ip}", &[IP]),
    line(
        "login.browser",
        Login,
        "Браузер",
        "Браузер: {agent}",
        &[AGENT],
    ),
    text(
        "login.hint",
        Login,
        "Подсказка",
        "Если вход запросили вы — подтвердите. Если нет — нажмите «Это не я».",
        &[],
    ),
    button(
        "login.btn.confirm",
        Login,
        "Кнопка подтверждения",
        "Подтвердить вход",
        &[],
    ),
    button(
        "login.btn.reject",
        Login,
        "Кнопка «Это не я»",
        "Это не я",
        &[],
    ),
    text(
        "login.confirmed",
        Login,
        "Вход подтверждён",
        "Вход подтверждён. Вернитесь в кабинет.",
        &[],
    ),
    text(
        "login.rejected",
        Login,
        "Вход отклонён",
        "Вход отклонён. Если это были не вы, ничего делать не нужно.",
        &[],
    ),
    text(
        "login.expired",
        Login,
        "Запрос устарел",
        "Запрос на вход устарел. Начните вход в кабинете заново.",
        &[],
    ),
    text(
        "login.error",
        Login,
        "Ошибка входа",
        "Вход в кабинет: {error}",
        &[ERROR],
    ),
    text(
        "login.limit",
        Login,
        "Новый аккаунт с сайта: предел с адреса",
        "Для входа на сайт нужен аккаунт, а создать новый с вашего адреса сейчас нельзя. Отправьте /start, чтобы пользоваться ботом, или попробуйте войти на сайт позже.",
        &[],
    ),
    text(
        "login.busy",
        Login,
        "Новый аккаунт с сайта: слишком много сразу",
        "Сейчас на сайте создаётся слишком много новых аккаунтов. Попробуйте войти ещё раз через минуту или отправьте /start, чтобы пользоваться ботом.",
        &[],
    ),
    text(
        "link.title",
        Login,
        "Привязка Telegram: заголовок",
        "<b>Привязка Telegram</b>",
        &[],
    ),
    text(
        "link.account",
        Login,
        "Привязка Telegram: аккаунт на сайте",
        "Аккаунт на {site}: {account}",
        &[SITE, MASKED],
    ),
    text(
        "link.who",
        Login,
        "Привязка Telegram: ваш Telegram",
        "Ваш Telegram: {who}",
        &[WHO],
    ),
    text(
        "link.empty",
        Login,
        "Привязка Telegram: пустой аккаунт закроется",
        "У этого Telegram есть свой аккаунт в боте, но он пустой: без подписок и оплат. После привязки Telegram перейдёт к аккаунту на сайте, а пустой аккаунт бота закроется.",
        &[],
    ),
    text(
        "link.hint",
        Login,
        "Привязка Telegram: подсказка",
        "Это не вход на сайт. После привязки бот и приложение откроют этот аккаунт, и через ваш Telegram можно будет войти в него на сайте. Если привязку начинали не вы, нажмите «Отменить».",
        &[],
    ),
    button(
        "link.btn.confirm",
        Login,
        "Привязка Telegram: кнопка «Привязать»",
        "Привязать",
        &[],
    ),
    button(
        "link.btn.cancel",
        Login,
        "Привязка Telegram: кнопка «Отменить»",
        "Отменить",
        &[],
    ),
    text(
        "link.done",
        Login,
        "Привязка Telegram: готово",
        "Telegram привязан. Подписка и оплата теперь доступны и здесь, в боте.",
        &[],
    ),
    text(
        "link.same",
        Login,
        "Привязка Telegram: уже привязан",
        "Этот Telegram уже привязан к вашему аккаунту.",
        &[],
    ),
    text(
        "link.canceled",
        Login,
        "Привязка Telegram: отмена",
        "Привязка отменена, ничего не изменилось.",
        &[],
    ),
    text(
        "link.error",
        Login,
        "Привязка Telegram: ошибка",
        "Привязка Telegram: {error}",
        &[ERROR],
    ),
    text(
        "email.title",
        Login,
        "Вход по почте: заголовок",
        "<b>Вход по почте</b>",
        &[],
    ),
    text(
        "email.none",
        Login,
        "Вход по почте: почта не привязана",
        "Почта не привязана. Привяжите её, чтобы входить в кабинет на сайте, если Telegram не работает.",
        &[],
    ),
    text(
        "email.current",
        Login,
        "Вход по почте: привязанная почта",
        "Почта: {email}",
        &[EMAIL],
    ),
    text(
        "email.unconfirmed",
        Login,
        "Вход по почте: почта не подтверждена",
        "Почта не подтверждена. Подтвердите её кодом из письма, чтобы восстанавливать по ней пароль.",
        &[],
    ),
    text(
        "email.password_yes",
        Login,
        "Вход по почте: пароль задан",
        "Пароль для сайта задан.",
        &[],
    ),
    text(
        "email.password_no",
        Login,
        "Вход по почте: пароля нет",
        "Пароль для сайта не задан.",
        &[],
    ),
    text(
        "email.how_password",
        Login,
        "Как войти по почте и паролю",
        "Если Telegram не работает, откройте {url}, выберите «Почта» и войдите с адресом {email} и паролем. Забыли пароль — нажмите «Забыли пароль?», код для нового пароля придёт на почту.",
        &[CABINET, EMAIL],
    ),
    text(
        "email.how_no_letters",
        Login,
        "Как войти по почте и паролю, пока письма не отправляются",
        "Если Telegram не работает, откройте {url}, выберите «Почта» и войдите с адресом {email} и паролем. Забыли пароль — напишите в поддержку.",
        &[CABINET, EMAIL],
    ),
    text(
        "email.how_code",
        Login,
        "Как войти по коду из письма",
        "Если Telegram не работает, откройте {url}, выберите «Почта», введите {email} и нажмите «Прислать код»: код для входа придёт в письме.",
        &[CABINET, EMAIL],
    ),
    text(
        "email.ask",
        Login,
        "Просьба прислать почту",
        "Пришлите адрес почты одним сообщением. На него придёт код подтверждения.",
        &[],
    ),
    text(
        "email.ask_no_code",
        Login,
        "Просьба прислать почту без кода",
        "Пришлите адрес почты одним сообщением. Кода подтверждения не будет, поэтому проверьте адрес.",
        &[],
    ),
    text(
        "email.invalid",
        Login,
        "Адрес почты с ошибкой",
        "В адресе ошибка. Пришлите адрес целиком, например name@example.com.",
        &[],
    ),
    text(
        "email.ask_app_code",
        Login,
        "Просьба прислать код из приложения",
        "Вход в аккаунт защищён кодом из приложения. Чтобы привязать {email}, пришлите шестизначный код из приложения-аутентификатора.",
        &[EMAIL],
    ),
    text(
        "email.ask_password",
        Login,
        "Просьба прислать пароль для смены почты",
        "Чтобы сменить почту на {email}, пришлите текущий пароль для сайта. Бот сразу удалит сообщение с паролем, но его текст может остаться в уведомлениях на телефоне.",
        &[EMAIL],
    ),
    text(
        "email.change_in_app",
        Login,
        "Смена почты в приложении",
        "Чтобы сменить почту, нужен пароль для сайта. Введите его в приложении: так пароль не останется в переписке.",
        &[],
    ),
    text(
        "email.code_sent",
        Login,
        "Код отправлен",
        "Код отправлен на {email}. Пришлите его сюда, он действует {minutes}.",
        &[EMAIL, MINUTES],
    ),
    text(
        "email.code_again",
        Login,
        "Просьба прислать код ещё раз",
        "Пришлите код из письма на {email} или запросите новый.",
        &[EMAIL],
    ),
    text(
        "email.linked",
        Login,
        "Почта привязана",
        "Почта {email} привязана.",
        &[EMAIL],
    ),
    text(
        "email.linked_no_code",
        Login,
        "Почта привязана без кода",
        "Почта {email} привязана без проверки: письма сейчас не отправляются. Войти на сайт по ней можно только с паролем.",
        &[EMAIL],
    ),
    text(
        "email.confirmed",
        Login,
        "Почта подтверждена",
        "Почта {email} подтверждена.",
        &[EMAIL],
    ),
    text(
        "email.set_password_app",
        Login,
        "Просьба задать пароль в приложении",
        "Теперь задайте пароль для сайта: кнопка ниже откроет окно в приложении.",
        &[],
    ),
    text(
        "email.set_password_chat",
        Login,
        "Просьба прислать пароль в чат",
        "Теперь задайте пароль для сайта: пришлите его следующим сообщением, не короче 10 символов. Бот сразу удалит сообщение с паролем, но его текст может остаться в уведомлениях на телефоне. Если не хотите присылать пароль в чат, задайте его в кабинете: {url}",
        &[CABINET],
    ),
    text(
        "email.password_saved",
        Login,
        "Пароль задан",
        "Пароль для сайта задан.",
        &[],
    ),
    text(
        "email.secret_left",
        Login,
        "Сообщение с паролем не удалилось",
        "Не получилось удалить ваше сообщение из чата. Удалите его сами.",
        &[],
    ),
    text(
        "email.off",
        Login,
        "Почту привязать нельзя",
        "Привязать почту сейчас нельзя: вход на сайт выключен или письма не отправляются. Если нужна помощь, напишите в поддержку.",
        &[],
    ),
    text(
        "email.error",
        Login,
        "Ошибка при привязке почты",
        "{error}",
        &[ERROR],
    ),
    button(
        "email.btn.link",
        Login,
        "Кнопка «Привязать почту»",
        "Привязать почту",
        &[],
    ),
    button(
        "email.btn.confirm",
        Login,
        "Кнопка «Подтвердить почту»",
        "Подтвердить почту",
        &[],
    ),
    button(
        "email.btn.change",
        Login,
        "Кнопка «Сменить почту»",
        "Сменить почту",
        &[],
    ),
    button(
        "email.btn.change_app",
        Login,
        "Кнопка смены почты в приложении",
        "Сменить почту в приложении",
        &[],
    ),
    button(
        "email.btn.password",
        Login,
        "Кнопка «Задать пароль»",
        "Задать пароль",
        &[],
    ),
    button(
        "email.btn.resend",
        Login,
        "Кнопка нового кода",
        "Отправить код ещё раз",
        &[],
    ),
    button(
        "email.btn.other",
        Login,
        "Кнопка другой почты",
        "Указать другую почту",
        &[],
    ),
    text(
        "docs.intro",
        Documents,
        "Перед первым входом",
        "Прежде чем начать, ознакомьтесь с документами:",
        &[],
    ),
    text(
        "docs.item",
        Documents,
        "Строка документа",
        "• <a href=\"{url}\">{title}</a>",
        &[TITLE, URL],
    ),
    text(
        "docs.agree",
        Documents,
        "Согласие",
        "\nНажимая «Принимаю», вы соглашаетесь с ними.",
        &[],
    ),
    button(
        "docs.btn.accept",
        Documents,
        "Кнопка согласия",
        "Принимаю",
        &[],
    ),
    text(
        "docs.list_title",
        Documents,
        "Заголовок списка документов",
        "<b>Документы</b>",
        &[],
    ),
    text(
        "notify.payment_paid",
        Notify,
        "Оплата получена",
        "Оплата {amount} получена. Подписка обновится в течение минуты.",
        &[AMOUNT],
    ),
    text(
        "notify.payment_paid_plain",
        Notify,
        "Оплата получена, без суммы",
        "Оплата получена. Подписка обновится в течение минуты.",
        &[],
    ),
    text(
        "notify.topup_done",
        Notify,
        "Баланс пополнен",
        "Баланс пополнен на {amount}.",
        &[AMOUNT],
    ),
    text(
        "notify.topup_done_bonus",
        Notify,
        "Баланс пополнен с бонусом",
        "На баланс пришло {total}: {amount} и бонус {bonus}.",
        &[TOTAL_BONUS, AMOUNT, TOPUP_BONUS],
    ),
    text(
        "notify.topup_done_plain",
        Notify,
        "Баланс пополнен, без суммы",
        "Баланс пополнен.",
        &[],
    ),
    text(
        "notify.subscription_ready",
        Notify,
        "Подписка готова",
        "Подписка готова и действует до {date}. Ссылка — в разделе «Подписка».",
        &[DATE],
    ),
    text(
        "notify.subscription_ready_plain",
        Notify,
        "Подписка готова, без даты",
        "Подписка готова. Ссылка — в разделе «Подписка».",
        &[],
    ),
    text(
        "notify.subscription_renewed",
        Notify,
        "Подписка продлена",
        "Подписка продлена до {date}.",
        &[DATE],
    ),
    text(
        "notify.subscription_renewed_plain",
        Notify,
        "Подписка продлена, без даты",
        "Подписка продлена.",
        &[],
    ),
    text(
        "notify.subscription_expiring",
        Notify,
        "Подписка заканчивается",
        "Подписка закончится {date}. Продлите её, чтобы VPN не отключился.",
        &[DATE],
    ),
    text(
        "notify.subscription_expiring_plain",
        Notify,
        "Подписка заканчивается, без даты",
        "Подписка скоро закончится. Продлите её, чтобы VPN не отключился.",
        &[],
    ),
    text(
        "notify.autorenew_upcoming",
        Notify,
        "Скоро автопродление",
        "{date} спишем {amount} за продление подписки. Если на балансе не хватит, пополните его заранее.",
        &[DATE, AMOUNT],
    ),
    text(
        "notify.autorenew_upcoming_plain",
        Notify,
        "Скоро автопродление, без суммы",
        "Скоро продлим подписку. Если на балансе не хватит, пополните его заранее.",
        &[],
    ),
    text(
        "notify.autorenew_failed",
        Notify,
        "Автопродление не прошло",
        "Не получилось продлить подписку: не хватает {missing}. Пополните баланс — продлим автоматически.",
        &[MISSING],
    ),
    text(
        "notify.autorenew_failed_plain",
        Notify,
        "Автопродление не прошло, без суммы",
        "Не получилось продлить подписку. Пополните баланс — продлим автоматически.",
        &[],
    ),
    text(
        "notify.autorenew_pay",
        Notify,
        "Продлите вручную (баланс выключен)",
        "Продлить подписку автоматически не получилось. Продлите её до {date}, чтобы VPN не отключился.",
        &[DATE],
    ),
    text(
        "notify.autorenew_pay_plain",
        Notify,
        "Продлите вручную, без даты",
        "Продлить подписку автоматически не получилось. Продлите её вручную, чтобы VPN не отключился.",
        &[],
    ),
    text(
        "notify.autorenew_disabled",
        Notify,
        "Автопродление выключено",
        "Автопродление выключено: оплатить не удалось. Продлите подписку вручную.",
        &[],
    ),
    text(
        "notify.renewal_cancelled",
        Notify,
        "Автопродление через платёжку отключено",
        "Автопродление через {provider} отключено, больше списаний не будет. Подписка действует до {date}.",
        &[PAY_PROVIDER, DATE],
    ),
    text(
        "notify.renewal_cancelled_plain",
        Notify,
        "Автопродление через платёжку отключено, без даты",
        "Автопродление через {provider} отключено, больше списаний не будет.",
        &[PAY_PROVIDER],
    ),
    text(
        "notify.autorenew_after_off",
        Notify,
        "Списание ушло до выключения автопродления",
        "Списание {amount} за продление ушло до того, как автопродление выключили, поэтому подписка продлена до {date}. Если продление не нужно, напишите в поддержку — вернём деньги.",
        &[AMOUNT, DATE],
    ),
    text(
        "notify.autorenew_after_off_plain",
        Notify,
        "Списание ушло до выключения автопродления, без суммы",
        "Списание за продление ушло до того, как автопродление выключили, поэтому подписка продлена. Если продление не нужно, напишите в поддержку — вернём деньги.",
        &[],
    ),
    text(
        "notify.payment_refunded_blocked",
        Notify,
        "Оплата возвращена: аккаунт заблокирован",
        "Оплата {amount} возвращена: аккаунт заблокирован, заказ не выдан.",
        &[AMOUNT],
    ),
    text(
        "notify.payment_refunded_blocked_plain",
        Notify,
        "Оплата возвращена, без суммы",
        "Оплата возвращена: аккаунт заблокирован, заказ не выдан.",
        &[],
    ),
    text(
        "notify.stars_refunded",
        Notify,
        "Звёзды за покупку возвращены",
        "Звёзды за покупку «{label}» возвращены: {stars}.",
        &[PURCHASE, REFUNDED_STARS],
    ),
    text(
        "notify.stars_refunded_plain",
        Notify,
        "Звёзды возвращены, покупка неизвестна",
        "Звёзды возвращены: {stars}.",
        &[REFUNDED_STARS],
    ),
    text(
        "notify.support_reply",
        Notify,
        "Ответ поддержки",
        "<b>Ответ поддержки</b>\n\n{body}",
        &[BODY],
    ),
    text(
        "notify.gift_ready",
        Notify,
        "Подарок оплачен",
        "Подарок оплачен: «{tariff}» на {term}.\nКод: <code>{code}</code>",
        &[TARIFF, TERM, GIFT_CODE],
    ),
    line(
        "notify.gift_ready_link",
        Notify,
        "Подарок: ссылка для бота",
        "Отправьте другу ссылку — подарок активируется в боте:\n{link}",
        &[GIFT_LINK],
    ),
    line(
        "notify.gift_ready_cabinet",
        Notify,
        "Подарок: ссылка для кабинета",
        "Или на сайте: {url}",
        &[URL],
    ),
    line(
        "notify.gift_ready_until",
        Notify,
        "Подарок: до какого числа действует код",
        "Код действует до {date}.",
        &[DATE],
    ),
    text(
        "notify.gift_redeemed",
        Notify,
        "Подарок активирован",
        "Ваш подарок <code>{code}</code> активировали: «{tariff}» на {term}.",
        &[GIFT_CODE, TARIFF, TERM],
    ),
    hideable(
        "notify.btn.gifts",
        Notify,
        "Кнопка «Мои подарки»",
        "Мои подарки",
    ),
    text(
        "notify.referral_credit",
        Notify,
        "Начисление за друга",
        "Начислено {amount} за оплату приглашённого друга.",
        &[AMOUNT],
    ),
    text(
        "notify.withdrawal_paid",
        Notify,
        "Вывод выплачен",
        "Заявка на вывод {amount} выплачена.",
        &[AMOUNT],
    ),
    text(
        "notify.withdrawal_rejected",
        Notify,
        "Вывод отклонён с причиной",
        "Заявка на вывод {amount} отклонена: {comment}. Деньги вернулись на реферальный баланс.",
        &[AMOUNT, COMMENT],
    ),
    text(
        "notify.contest_won",
        Notify,
        "Победа в конкурсе",
        "Вы победили в конкурсе «{title}»!\nПриз: {prize}. {result}",
        &[TITLE, PRIZE, RESULT],
    ),
    text(
        "notify.contest_won_code",
        Notify,
        "Промокод победителя",
        "Ваш промокод: <code>{code}</code> — введите его при оплате.",
        &[CODE],
    ),
    text(
        "notify.contest_waiting",
        Notify,
        "Победа, приз ждёт подписку",
        "Вы победили в конкурсе «{title}»!\nПриз: {prize}. Дни добавятся, как только у вас появится подписка.",
        &[TITLE, PRIZE],
    ),
    text(
        "notify.contest_won_place",
        Notify,
        "Призовое место в конкурсе",
        "Вы заняли {place} место в конкурсе «{title}»!\nПриз: {prize}. {result}",
        &[PLACE, TITLE, PRIZE, RESULT],
    ),
    text(
        "notify.contest_waiting_place",
        Notify,
        "Призовое место, приз ждёт подписку",
        "Вы заняли {place} место в конкурсе «{title}»!\nПриз: {prize}. Дни добавятся, как только у вас появится подписка.",
        &[PLACE, TITLE, PRIZE],
    ),
    text(
        "notify.withdrawal_rejected_plain",
        Notify,
        "Вывод отклонён",
        "Заявка на вывод {amount} отклонена. Деньги вернулись на реферальный баланс.",
        &[AMOUNT],
    ),
    text(
        "notify.traffic_warning",
        Notify,
        "Превышен порог трафика",
        "{when} через VPN прошло {used} — больше лимита {limit} {per}. Пожалуйста, снизьте нагрузку, иначе доступ могут временно ограничить.",
        &[WHEN, USED, LIMIT, PER],
    ),
    text(
        "notify.traffic_restricted",
        Notify,
        "Доступ ограничен из-за трафика",
        "{when} через VPN прошло {used} при лимите {limit} {per}. Доступ временно ограничен до {until}, потом включится сам.",
        &[WHEN, USED, LIMIT, PER, UNTIL],
    ),
    text(
        "notify.traffic_lifted",
        Notify,
        "Ограничение по трафику снято",
        "Ограничение по трафику снято, VPN снова работает.",
        &[],
    ),
    text(
        "notify.oauth_linked",
        Notify,
        "Привязан вход через Google или Apple",
        "К вашему аккаунту привязан вход через {provider}: {account}. Если это сделали не вы, сразу напишите в поддержку.",
        &[PROVIDER, ACCOUNT],
    ),
    text(
        "notify.oauth_unlinked",
        Notify,
        "Отвязан вход через Google или Apple",
        "Вы подтвердили почту кодом, поэтому мы отвязали вход через Google или Apple, привязанный до подтверждения, и завершили все сеансы на сайте. Если этот вход привязывали вы, привяжите его снова в разделе «Безопасность».",
        &[],
    ),
    text(
        "notify.password_issued",
        Notify,
        "Выдан пароль для сайта",
        "Администратор выдал вам временный пароль для входа на сайт. При первом входе с ним задайте свой пароль. Если вы не просили новый пароль, сразу напишите в поддержку.",
        &[],
    ),
    text(
        "notify.email_set",
        Notify,
        "Почта указана в админке",
        "Через поддержку в ваш аккаунт добавлена почта {email}. Если вы об этом не просили, сразу напишите в поддержку.",
        &[EMAIL],
    ),
    text(
        "notify.email_removed",
        Notify,
        "Почта отвязана в админке",
        "Через поддержку от вашего аккаунта отвязана почта {email}: входить по ней на сайт больше нельзя. Если вы об этом не просили, сразу напишите в поддержку.",
        &[EMAIL],
    ),
    hideable(
        "notify.btn.subscription",
        Notify,
        "Кнопка «Подписка»",
        "Подписка",
    ),
    hideable("notify.btn.balance", Notify, "Кнопка «Баланс»", "Баланс"),
    hideable("notify.btn.extend", Notify, "Кнопка «Продлить»", "Продлить"),
    hideable(
        "notify.btn.ticket",
        Notify,
        "Кнопка «Открыть переписку»",
        "Открыть переписку",
    ),
    hideable("notify.btn.reply", Notify, "Кнопка «Ответить»", "Ответить"),
    hideable(
        "notify.btn.support",
        Notify,
        "Кнопка «Поддержка»",
        "Поддержка",
    ),
    hideable(
        "notify.btn.referral",
        Notify,
        "Кнопка «Пригласить друга»",
        "Пригласить друга",
    ),
    hideable(
        "notify.btn.contests",
        Notify,
        "Кнопка «Конкурсы»",
        "Конкурсы",
    ),
    hideable(
        "notify.btn.email",
        Notify,
        "Кнопка «Вход по почте»",
        "Вход по почте",
    ),
    button("common.btn.menu", Common, "Кнопка «В меню»", "В меню", &[]),
    button("common.btn.back", Common, "Кнопка «Назад»", "Назад", &[]),
    button(
        "common.btn.cancel",
        Common,
        "Кнопка «Отменить»",
        "Отменить",
        &[],
    ),
    button(
        "common.btn.subscription",
        Common,
        "Кнопка «Подписка» после оплаты",
        "Подписка",
        &[],
    ),
    text(
        "common.blocked",
        Common,
        "Аккаунт заблокирован",
        "Аккаунт заблокирован. Напишите в поддержку.",
        &[],
    ),
    text(
        "common.flood",
        Common,
        "Слишком много сообщений подряд",
        "Слишком много сообщений подряд. Бот ненадолго перестанет отвечать, попробуйте чуть позже.",
        &[],
    ),
    text(
        "common.busy",
        Common,
        "Слишком много новых пользователей",
        "Сейчас к боту приходит слишком много новых пользователей. Попробуйте ещё раз через минуту.",
        &[],
    ),
];

pub fn entry(key: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.key == key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonStyle {
    Primary,
    Success,
    Danger,
}

impl ButtonStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            ButtonStyle::Primary => "primary",
            ButtonStyle::Success => "success",
            ButtonStyle::Danger => "danger",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<ButtonStyle>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

impl ButtonOverride {
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.emoji_id.is_none() && self.style.is_none() && !self.hidden
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Translation {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub texts: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buttons: BTreeMap<String, String>,
}

impl Translation {
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty() && self.buttons.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentConfig {
    #[serde(default)]
    pub texts: BTreeMap<String, String>,
    #[serde(default)]
    pub buttons: BTreeMap<String, ButtonOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_columns: Option<u8>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub layouts: BTreeMap<String, crate::bot_layout::Rows>,
    #[serde(
        default,
        skip_serializing_if = "crate::bot_layout::MenuMode::is_classic"
    )]
    pub menu_mode: crate::bot_layout::MenuMode,
    #[serde(default, skip_serializing_if = "Translation::is_empty")]
    pub en: Translation,
}

impl ContentConfig {
    pub fn translation(&self, locale: Locale) -> Option<&Translation> {
        match locale {
            Locale::Ru => None,
            Locale::En => Some(&self.en),
        }
    }

    pub fn translation_mut(&mut self, locale: Locale) -> Option<&mut Translation> {
        match locale {
            Locale::Ru => None,
            Locale::En => Some(&mut self.en),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value<'a> {
    Text(&'a str),
    Html(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonText {
    pub text: String,
    pub emoji_id: Option<String>,
    pub style: Option<ButtonStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Content {
    config: ContentConfig,
    locale: Locale,
}

pub fn default_text(key: &str, locale: Locale) -> Option<&'static str> {
    match locale {
        Locale::Ru => entry(key).map(|e| e.default),
        Locale::En => crate::bot_content_en::default(key),
    }
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

fn fill(template: &str, vars: &[(&str, Value<'_>)], html: bool) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if is_var_name(&after[..end]) => {
                let name = &after[..end];
                match vars.iter().find(|(n, _)| *n == name) {
                    Some((_, Value::Text(value))) => {
                        if html {
                            out.push_str(&escape(value));
                        } else {
                            out.push_str(value);
                        }
                    }
                    Some((_, Value::Html(value))) => out.push_str(value),
                    None => {}
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_var_name(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 32
        && raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

pub fn placeholders(template: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if is_var_name(&after[..end]) => {
                found.push(after[..end].to_string());
                rest = &after[end + 1..];
            }
            _ => rest = after,
        }
    }
    found
}

impl Content {
    pub fn new(config: ContentConfig) -> Self {
        Content {
            config,
            locale: Locale::Ru,
        }
    }

    pub fn in_locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    pub fn config(&self) -> &ContentConfig {
        &self.config
    }

    fn russian(&self, key: &str) -> Option<&str> {
        self.config
            .texts
            .get(key)
            .map(String::as_str)
            .or_else(|| entry(key).map(|e| e.default))
    }

    fn russian_label<'a>(
        &'a self,
        key: &str,
        custom: Option<&'a ButtonOverride>,
    ) -> Option<&'a str> {
        custom
            .and_then(|c| c.text.as_deref())
            .or_else(|| entry(key).map(|e| e.default))
    }

    pub fn menu_columns(&self) -> usize {
        match self.config.menu_columns {
            Some(2) => 2,
            _ => 1,
        }
    }

    pub fn layout(&self, screen: crate::bot_layout::Screen) -> crate::bot_layout::Rows {
        match self.config.layouts.get(screen.as_str()) {
            Some(rows) => rows.clone(),
            None => screen.standard(self.menu_columns()),
        }
    }

    pub fn has_own_layout(&self, screen: crate::bot_layout::Screen) -> bool {
        self.config.layouts.contains_key(screen.as_str())
    }

    pub fn menu_mode(&self) -> crate::bot_layout::MenuMode {
        self.config.menu_mode
    }

    pub fn template(&self, key: &str) -> &str {
        match self.config.translation(self.locale) {
            None => self.russian(key),
            Some(own) => own
                .texts
                .get(key)
                .map(String::as_str)
                .or_else(|| default_text(key, self.locale))
                .or_else(|| self.russian(key)),
        }
        .unwrap_or("")
    }

    pub fn text(&self, key: &str, vars: &[(&str, Value<'_>)]) -> String {
        fill(self.template(key), vars, true)
    }

    pub fn button(&self, key: &str, vars: &[(&str, Value<'_>)]) -> Option<ButtonText> {
        let custom = self.config.buttons.get(key);
        if custom.is_some_and(|c| c.hidden) && entry(key).is_some_and(|e| e.optional) {
            return None;
        }
        let template = match self.config.translation(self.locale) {
            None => self.russian_label(key, custom),
            Some(own) => own
                .buttons
                .get(key)
                .map(String::as_str)
                .or_else(|| default_text(key, self.locale))
                .or_else(|| self.russian_label(key, custom)),
        }
        .unwrap_or(key);
        let text = fill(template, vars, false);
        let text = if text.trim().is_empty() {
            let standard = default_text(key, self.locale)
                .or_else(|| entry(key).map(|e| e.default))
                .unwrap_or(key);
            fill(standard, vars, false)
        } else {
            text
        };
        Some(ButtonText {
            text,
            emoji_id: custom.and_then(|c| c.emoji_id.clone()),
            style: custom.and_then(|c| c.style),
        })
    }

    pub fn label(&self, key: &str, vars: &[(&str, Value<'_>)]) -> String {
        self.button(key, vars).map(|b| b.text).unwrap_or_default()
    }
}

const ALLOWED_TAGS: [&str; 15] = [
    "b",
    "strong",
    "i",
    "em",
    "u",
    "ins",
    "s",
    "strike",
    "del",
    "span",
    "tg-spoiler",
    "a",
    "code",
    "pre",
    "blockquote",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkupProblem {
    Unclosed(String),
    Unexpected(String),
    BadLink,
    BadEmoji,
    BadSpan,
}

impl MarkupProblem {
    pub fn message(&self) -> String {
        match self {
            MarkupProblem::Unclosed(tag) => format!("тег <{tag}> не закрыт"),
            MarkupProblem::Unexpected(tag) => format!("лишний закрывающий тег </{tag}>"),
            MarkupProblem::BadLink => {
                "у ссылки должен быть адрес вида <a href=\"https://…\">текст</a>".into()
            }
            MarkupProblem::BadEmoji => {
                "премиум-эмодзи пишется так: <tg-emoji emoji-id=\"5368324170671202286\">🔥</tg-emoji>"
                    .into()
            }
            MarkupProblem::BadSpan => "у <span> допустим только class=\"tg-spoiler\"".into(),
        }
    }
}

fn attribute<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    let at = raw.find(&format!("{name}="))?;
    let value = &raw[at + name.len() + 1..];
    let quote = value.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &value[1..];
    let end = value.find(quote)?;
    Some(&value[..end])
}

fn is_entity(raw: &str) -> bool {
    let Some(end) = raw.find(';') else {
        return false;
    };
    let name = &raw[1..end];
    matches!(name, "amp" | "lt" | "gt" | "quot")
        || (name.starts_with('#')
            && name.len() > 1
            && name.len() <= 9
            && (name[1..].bytes().all(|b| b.is_ascii_digit())
                || (name[1..].starts_with(['x', 'X'])
                    && name.len() > 2
                    && name[2..].bytes().all(|b| b.is_ascii_hexdigit()))))
}

fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (index, ch) in value.char_indices() {
        match ch {
            '&' if is_entity(&value[index..]) => out.push('&'),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

pub fn normalize_markup(raw: &str) -> Result<String, MarkupProblem> {
    let mut out = String::with_capacity(raw.len() + 16);
    let mut open: Vec<String> = Vec::new();
    let mut rest = raw;
    while let Some(index) = rest.find(['<', '>', '&']) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        if let Some(after) = tail.strip_prefix('&') {
            if is_entity(tail) {
                let end = tail.find(';').unwrap_or(0);
                out.push_str(&tail[..=end]);
                rest = &tail[end + 1..];
            } else {
                out.push_str("&amp;");
                rest = after;
            }
            continue;
        }
        if let Some(after) = tail.strip_prefix('>') {
            out.push_str("&gt;");
            rest = after;
            continue;
        }
        let Some(close) = tail.find('>') else {
            out.push_str("&lt;");
            rest = &tail[1..];
            continue;
        };
        let inner = &tail[1..close];
        let closing = inner.starts_with('/');
        let body = inner.trim_start_matches('/');
        let name: String = body
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
            .to_ascii_lowercase();
        let known = ALLOWED_TAGS.contains(&name.as_str()) || name == "tg-emoji";
        if name.is_empty() || !known || inner.contains('<') {
            out.push_str("&lt;");
            rest = &tail[1..];
            continue;
        }
        if closing {
            match open.pop() {
                Some(last) if last == name => {
                    out.push_str(&format!("</{name}>"));
                }
                _ => return Err(MarkupProblem::Unexpected(name)),
            }
        } else {
            let attributes = &body[name.len()..];
            let tag = match name.as_str() {
                "a" => {
                    let href = attribute(attributes, "href")
                        .filter(|h| {
                            h.starts_with("https://")
                                || h.starts_with("http://")
                                || h.starts_with("tg://")
                                || h.starts_with('{')
                        })
                        .ok_or(MarkupProblem::BadLink)?;
                    format!("<a href=\"{}\">", escape_attr(href))
                }
                "tg-emoji" => {
                    let id = attribute(attributes, "emoji-id")
                        .filter(|id| {
                            !id.is_empty()
                                && id.len() <= EMOJI_ID_MAX
                                && id.bytes().all(|b| b.is_ascii_digit())
                        })
                        .ok_or(MarkupProblem::BadEmoji)?;
                    format!("<tg-emoji emoji-id=\"{id}\">")
                }
                "span" => {
                    if attribute(attributes, "class") != Some("tg-spoiler") {
                        return Err(MarkupProblem::BadSpan);
                    }
                    "<span class=\"tg-spoiler\">".to_string()
                }
                "blockquote" if attributes.contains("expandable") => {
                    "<blockquote expandable>".to_string()
                }
                "code" => match attribute(attributes, "class")
                    .filter(|c| c.starts_with("language-") && c.len() < 40)
                {
                    Some(class) => format!("<code class=\"{class}\">"),
                    None => "<code>".to_string(),
                },
                other => format!("<{other}>"),
            };
            out.push_str(&tag);
            open.push(name);
        }
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    if let Some(last) = open.pop() {
        return Err(MarkupProblem::Unclosed(last));
    }
    Ok(out)
}

pub fn strip_custom_emoji(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<tg-emoji") {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match tail.find('>') {
            Some(end) => rest = &tail[end + 1..],
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.replace("</tg-emoji>", "")
}

pub fn visible_len(html: &str) -> usize {
    let mut count = 0usize;
    let mut chars = html.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '<' => {
                for next in chars.by_ref() {
                    if next == '>' {
                        break;
                    }
                }
            }
            '&' => {
                let mut taken = 0;
                while let Some(&next) = chars.peek() {
                    chars.next();
                    taken += 1;
                    if next == ';' || taken > 10 {
                        break;
                    }
                }
                count += 1;
            }
            other => count += other.len_utf16(),
        }
    }
    count
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryProblem {
    pub key: String,
    pub message: String,
}

fn problem(key: &str, message: impl Into<String>) -> EntryProblem {
    EntryProblem {
        key: key.to_string(),
        message: message.into(),
    }
}

fn check_vars(entry: &Entry, template: &str) -> Result<(), EntryProblem> {
    for name in placeholders(template) {
        if !entry.vars.iter().any(|v| v.name == name) {
            let allowed: Vec<String> = entry
                .vars
                .iter()
                .map(|v| format!("{{{}}}", v.name))
                .collect();
            let message = if allowed.is_empty() {
                format!("подстановки {{{name}}} здесь нет")
            } else {
                format!(
                    "подстановки {{{name}}} здесь нет, можно: {}",
                    allowed.join(", ")
                )
            };
            return Err(problem(entry.key, message));
        }
    }
    Ok(())
}

pub fn clean_text(key: &str, raw: &str) -> Result<String, EntryProblem> {
    let entry = entry(key)
        .filter(|e| e.kind == EntryKind::Text)
        .ok_or_else(|| problem(key, "такого текста нет"))?;
    let raw = raw.replace("\r\n", "\n");
    if raw.trim().is_empty() {
        return if entry.optional {
            Ok(String::new())
        } else {
            Err(problem(key, "текст не может быть пустым"))
        };
    }
    if raw.chars().count() > TEXT_MAX {
        return Err(problem(
            key,
            format!("слишком длинно: больше {TEXT_MAX} символов"),
        ));
    }
    check_vars(entry, &raw)?;
    normalize_markup(&raw).map_err(|p| problem(key, p.message()))
}

pub fn clean_button(key: &str, raw: &ButtonOverride) -> Result<ButtonOverride, EntryProblem> {
    let entry = entry(key)
        .filter(|e| e.kind == EntryKind::Button)
        .ok_or_else(|| problem(key, "такой кнопки нет"))?;
    let text = match raw.text.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(text) if text.chars().count() > BUTTON_MAX => {
            return Err(problem(
                key,
                format!("надпись на кнопке длиннее {BUTTON_MAX} символов"),
            ));
        }
        Some(text) if text.contains('\n') => {
            return Err(problem(key, "надпись на кнопке должна быть в одну строку"));
        }
        Some(text) => {
            check_vars(entry, text)?;
            Some(text.to_string())
        }
    };
    let emoji_id = match raw.emoji_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(id) if id.len() <= EMOJI_ID_MAX && id.bytes().all(|b| b.is_ascii_digit()) => {
            Some(id.to_string())
        }
        Some(_) => {
            return Err(problem(
                key,
                "код премиум-эмодзи — только цифры, его присылает бот по команде /emoji",
            ));
        }
    };
    if raw.hidden && !entry.optional {
        return Err(problem(key, "эту кнопку нельзя скрыть"));
    }
    Ok(ButtonOverride {
        text: text.filter(|t| t != entry.default),
        emoji_id,
        style: raw.style,
        hidden: raw.hidden,
    })
}

pub fn clean_label(key: &str, raw: &str) -> Result<Option<String>, EntryProblem> {
    let entry = entry(key)
        .filter(|e| e.kind == EntryKind::Button)
        .ok_or_else(|| problem(key, "такой кнопки нет"))?;
    let text = raw.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().count() > BUTTON_MAX {
        return Err(problem(
            key,
            format!("надпись на кнопке длиннее {BUTTON_MAX} символов"),
        ));
    }
    if text.contains('\n') {
        return Err(problem(key, "надпись на кнопке должна быть в одну строку"));
    }
    check_vars(entry, text)?;
    Ok(Some(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn sorted(mut names: Vec<String>) -> Vec<String> {
        names.sort();
        names.dedup();
        names
    }

    #[test]
    fn у_каждого_текста_бота_есть_английский_с_теми_же_подстановками() {
        for entry in CATALOG {
            let english = crate::bot_content_en::default(entry.key)
                .unwrap_or_else(|| panic!("{}: нет английского текста", entry.key));
            assert_eq!(
                sorted(placeholders(english)),
                sorted(placeholders(entry.default)),
                "{}: подстановки в английском не те",
                entry.key
            );
            assert_eq!(
                english.is_empty(),
                entry.default.is_empty(),
                "{}: пустой только в одном языке",
                entry.key
            );
            match entry.kind {
                EntryKind::Text if !english.is_empty() => {
                    assert_eq!(
                        clean_text(entry.key, english).unwrap(),
                        english,
                        "{}",
                        entry.key
                    );
                }
                EntryKind::Text => {}
                EntryKind::Button => {
                    assert_eq!(
                        clean_label(entry.key, english).unwrap().as_deref(),
                        Some(english),
                        "{}",
                        entry.key
                    );
                }
            }
            assert!(
                !english
                    .chars()
                    .any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')),
                "{}: в английском тексте кириллица",
                entry.key
            );
        }
    }

    #[test]
    fn английский_берёт_свой_текст_потом_стандартный_а_русский_не_меняется() {
        let mut config = ContentConfig::default();
        config
            .texts
            .insert("menu.no_subscription".into(), "Пока пусто.".into());
        config
            .texts
            .insert("menu.greeting".into(), "Привет, {name}!".into());
        config
            .en
            .texts
            .insert("sub.none".into(), "Nothing here, {brand}".into());
        config.buttons.insert(
            "menu.btn.buy".into(),
            ButtonOverride {
                text: Some("🚀 Купить".into()),
                emoji_id: Some("5368324170671202286".into()),
                style: Some(ButtonStyle::Success),
                hidden: false,
            },
        );
        config.buttons.insert(
            "menu.btn.balance".into(),
            ButtonOverride {
                hidden: true,
                ..ButtonOverride::default()
            },
        );
        config
            .en
            .buttons
            .insert("menu.btn.extend".into(), "Renew".into());
        let ru = Content::new(config.clone());
        let en = Content::new(config).in_locale(Locale::En);
        assert_eq!(ru.locale(), Locale::Ru);
        assert_eq!(en.locale(), Locale::En);
        assert_eq!(ru.text("menu.no_subscription", &[]), "Пока пусто.");
        assert_eq!(en.text("menu.no_subscription", &[]), "No subscription yet.");
        assert_eq!(
            en.text("menu.greeting", &[("name", Value::Text("Ann"))]),
            ""
        );
        assert_eq!(en.template("sub.none"), "Nothing here, {brand}");
        assert_eq!(ru.text("sub.none", &[]), "Подписки пока нет.");
        let buy = en.button("menu.btn.buy", &[]).unwrap();
        assert_eq!(buy.text, "Buy a subscription");
        assert_eq!(buy.emoji_id.as_deref(), Some("5368324170671202286"));
        assert_eq!(buy.style, Some(ButtonStyle::Success));
        assert_eq!(ru.label("menu.btn.buy", &[]), "🚀 Купить");
        assert!(en.button("menu.btn.balance", &[]).is_none());
        assert_eq!(en.label("menu.btn.extend", &[]), "Renew");
        assert_eq!(
            ru.label("menu.btn.extend", &[]),
            "Продлить или сменить тариф"
        );
        assert_eq!(
            en.label("dev.btn.disconnect", &[("title", Value::Text("iPhone"))]),
            "Disconnect iPhone"
        );
    }

    #[test]
    fn без_английского_перевода_ключа_остаётся_русский() {
        let mut config = ContentConfig::default();
        config
            .texts
            .insert("menu.title".into(), "<b>Свой</b>".into());
        let en = Content::new(config).in_locale(Locale::En);
        assert_eq!(en.template("menu.title"), "<b>{brand}</b>");
        assert_eq!(default_text("нет.такого", Locale::En), None, "чужой ключ");
        assert_eq!(en.template("нет.такого"), "");
        assert_eq!(en.label("нет.такого", &[]), "нет.такого");
    }

    #[test]
    fn английские_надписи_кнопок_проверяются() {
        assert_eq!(
            clean_label("menu.btn.buy", "  Buy  ").unwrap().as_deref(),
            Some("Buy")
        );
        assert_eq!(clean_label("menu.btn.buy", "  ").unwrap(), None);
        assert!(clean_label("menu.btn.buy", &"x".repeat(BUTTON_MAX + 1)).is_err());
        assert!(clean_label("menu.btn.buy", "a\nb").is_err());
        assert!(clean_label("dev.btn.disconnect", "Drop {device}").is_err());
        assert!(clean_label("menu.title", "x").is_err());
    }

    #[test]
    fn в_каталоге_нет_повторов_и_умолчания_проходят_проверку() {
        let mut seen = HashSet::new();
        for entry in CATALOG {
            assert!(seen.insert(entry.key), "повтор ключа {}", entry.key);
            for name in placeholders(entry.default) {
                assert!(
                    entry.vars.iter().any(|v| v.name == name),
                    "{}: подстановка {name} не описана",
                    entry.key
                );
            }
            match entry.kind {
                EntryKind::Text if !entry.default.is_empty() => {
                    assert_eq!(
                        clean_text(entry.key, entry.default).unwrap(),
                        entry.default.replace("\r\n", "\n"),
                        "{}",
                        entry.key
                    );
                }
                EntryKind::Text => assert!(entry.optional, "{}", entry.key),
                EntryKind::Button => {
                    assert!(!entry.default.is_empty(), "{}", entry.key);
                    assert!(entry.default.chars().count() <= BUTTON_MAX, "{}", entry.key);
                }
            }
        }
    }

    #[test]
    fn подстановки_экранируются_а_готовая_разметка_нет() {
        let content = Content::default();
        assert_eq!(
            content.text("menu.title", &[("brand", Value::Text("A&B <VPN>"))]),
            "<b>A&amp;B &lt;VPN&gt;</b>"
        );
        assert_eq!(
            content.text(
                "sub.traffic",
                &[
                    ("used", Value::Text("1 ГБ")),
                    ("limit", Value::Text("10 ГБ")),
                    ("reset", Value::Html(", обновится <b>1 мая</b>")),
                ]
            ),
            "Трафик: 1 ГБ из 10 ГБ, обновится <b>1 мая</b>"
        );
        assert_eq!(
            content.label("dev.btn.disconnect", &[("title", Value::Text("<iPhone>"))]),
            "Отключить <iPhone>"
        );
        assert_eq!(
            content.text(
                "menu.subscription_open",
                &[("mark", Value::Text("🟢")), ("tariff", Value::Text("X"))]
            ),
            "🟢 X: "
        );
    }

    #[test]
    fn свои_тексты_и_кнопки_важнее_умолчаний() {
        let mut config = ContentConfig::default();
        config
            .texts
            .insert("menu.greeting".into(), "Привет, {name}!".into());
        config.buttons.insert(
            "menu.btn.buy".into(),
            ButtonOverride {
                text: Some("🚀 Купить".into()),
                emoji_id: Some("5368324170671202286".into()),
                style: Some(ButtonStyle::Success),
                hidden: false,
            },
        );
        config.buttons.insert(
            "menu.btn.balance".into(),
            ButtonOverride {
                hidden: true,
                ..ButtonOverride::default()
            },
        );
        config.buttons.insert(
            "common.btn.menu".into(),
            ButtonOverride {
                hidden: true,
                ..ButtonOverride::default()
            },
        );
        let content = Content::new(config);
        assert_eq!(
            content.text("menu.greeting", &[("name", Value::Text("Аня"))]),
            "Привет, Аня!"
        );
        let buy = content.button("menu.btn.buy", &[]).unwrap();
        assert_eq!(buy.text, "🚀 Купить");
        assert_eq!(buy.emoji_id.as_deref(), Some("5368324170671202286"));
        assert_eq!(buy.style, Some(ButtonStyle::Success));
        assert!(content.button("menu.btn.balance", &[]).is_none());
        assert_eq!(content.label("common.btn.menu", &[]), "В меню");
        assert_eq!(
            content.text("menu.no_subscription", &[]),
            "Подписки пока нет."
        );
    }

    #[test]
    fn разметка_чинится_и_проверяется() {
        assert_eq!(
            normalize_markup("Цена < 100 & скидка > 5").unwrap(),
            "Цена &lt; 100 &amp; скидка &gt; 5"
        );
        assert_eq!(
            normalize_markup("<B>жирно</B> &amp; <i>курсив</i>").unwrap(),
            "<b>жирно</b> &amp; <i>курсив</i>"
        );
        assert_eq!(
            normalize_markup("<tg-emoji emoji-id='5368324170671202286'>🔥</tg-emoji> Огонь")
                .unwrap(),
            "<tg-emoji emoji-id=\"5368324170671202286\">🔥</tg-emoji> Огонь"
        );
        assert_eq!(
            normalize_markup("<a href=\"https://example.com/offer\">Оферта</a>").unwrap(),
            "<a href=\"https://example.com/offer\">Оферта</a>"
        );
        assert_eq!(
            normalize_markup("<a href=\"https://example.com/?a=1&b=2&amp;c=3\">x</a>").unwrap(),
            "<a href=\"https://example.com/?a=1&amp;b=2&amp;c=3\">x</a>"
        );
        assert_eq!(
            normalize_markup("<script>alert(1)</script>").unwrap(),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
        assert_eq!(
            normalize_markup("<b>не закрыт"),
            Err(MarkupProblem::Unclosed("b".into()))
        );
        assert_eq!(
            normalize_markup("<i>a</b>"),
            Err(MarkupProblem::Unexpected("b".into()))
        );
        assert_eq!(
            normalize_markup("<a href=\"javascript:alert(1)\">x</a>"),
            Err(MarkupProblem::BadLink)
        );
        assert_eq!(
            normalize_markup("<tg-emoji emoji-id=\"abc\">🔥</tg-emoji>"),
            Err(MarkupProblem::BadEmoji)
        );
    }

    #[test]
    fn проверка_текстов_и_кнопок_для_админки() {
        assert_eq!(
            clean_text("menu.title", "<b>{brand}</b> 👋").unwrap(),
            "<b>{brand}</b> 👋"
        );
        let unknown = clean_text("menu.title", "{price}").unwrap_err();
        assert!(unknown.message.contains("{brand}"), "{}", unknown.message);
        assert!(clean_text("menu.title", "  ").is_err());
        assert_eq!(clean_text("menu.greeting", "").unwrap(), "");
        assert!(clean_text("menu.btn.buy", "x").is_err());
        assert!(clean_text("nope", "x").is_err());
        let long = "а".repeat(TEXT_MAX + 1);
        assert!(clean_text("menu.title", &long).is_err());

        let ok = clean_button(
            "menu.btn.buy",
            &ButtonOverride {
                text: Some(" 🛒 Купить VPN ".into()),
                emoji_id: Some(" 5368324170671202286 ".into()),
                style: Some(ButtonStyle::Primary),
                hidden: false,
            },
        )
        .unwrap();
        assert_eq!(ok.text.as_deref(), Some("🛒 Купить VPN"));
        assert_eq!(ok.emoji_id.as_deref(), Some("5368324170671202286"));
        let same = clean_button(
            "menu.btn.buy",
            &ButtonOverride {
                text: Some("Купить подписку".into()),
                ..ButtonOverride::default()
            },
        )
        .unwrap();
        assert!(same.is_empty());
        assert!(
            clean_button(
                "sub.btn.extend",
                &ButtonOverride {
                    hidden: true,
                    ..ButtonOverride::default()
                }
            )
            .is_err()
        );
        for key in ["menu.btn.buy", "menu.btn.extend"] {
            let hidden = clean_button(
                key,
                &ButtonOverride {
                    hidden: true,
                    ..ButtonOverride::default()
                },
            )
            .unwrap();
            assert!(hidden.hidden, "{key}");
            let mut config = ContentConfig::default();
            config.buttons.insert(key.into(), hidden);
            assert!(Content::new(config.clone()).button(key, &[]).is_none());
            assert!(
                Content::new(config)
                    .in_locale(Locale::En)
                    .button(key, &[])
                    .is_none()
            );
        }
        assert!(
            clean_button(
                "menu.btn.balance",
                &ButtonOverride {
                    hidden: true,
                    ..ButtonOverride::default()
                }
            )
            .unwrap()
            .hidden
        );
        assert!(
            clean_button(
                "menu.btn.buy",
                &ButtonOverride {
                    emoji_id: Some("🔥".into()),
                    ..ButtonOverride::default()
                }
            )
            .is_err()
        );
        assert!(
            clean_button(
                "dev.btn.disconnect",
                &ButtonOverride {
                    text: Some("Убрать {device}".into()),
                    ..ButtonOverride::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn картинки_берутся_по_режиму_раздела() {
        let mut config = ImagesConfig::default();
        let only_common = |slot: ImageSlot| slot == ImageSlot::Common;
        let all = |_: ImageSlot| true;
        let none = |_: ImageSlot| false;
        assert_eq!(
            config.resolve(Section::Menu, only_common),
            Some(ImageSlot::Common)
        );
        assert_eq!(config.resolve(Section::Menu, none), None);
        assert_eq!(config.resolve(Section::Notify, all), None);
        config.sections.insert(ImageSlot::Menu, ImageMode::Own);
        config.sections.insert(ImageSlot::Buy, ImageMode::Off);
        assert_eq!(config.resolve(Section::Menu, all), Some(ImageSlot::Menu));
        assert_eq!(
            config.resolve(Section::Menu, only_common),
            Some(ImageSlot::Common)
        );
        assert_eq!(config.resolve(Section::Payment, all), None);
        assert_eq!(
            config.resolve(Section::Devices, all),
            Some(ImageSlot::Common)
        );
    }

    #[test]
    fn длина_подписи_считается_без_разметки() {
        assert_eq!(visible_len("<b>Привет</b> &amp; 🔥"), 11);
        assert_eq!(
            strip_custom_emoji("<tg-emoji emoji-id=\"1\">🔥</tg-emoji> ок"),
            "🔥 ок"
        );
    }
}
