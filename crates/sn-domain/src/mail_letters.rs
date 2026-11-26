use crate::bot_content::{escape, normalize_markup};
use crate::client::Locale;
use crate::format;
use crate::money::{Currency, Money, Stars};
use serde::{Deserialize, Serialize};

pub const SUBJECT_MAX: usize = 150;
pub const TEXT_MAX: usize = 4000;
pub const BUTTON_MAX: usize = 40;
pub const SIGN_MAX: usize = 300;
const NAME_MAX: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Login,
    Payments,
    Renewal,
    Support,
    Service,
}

impl Group {
    pub const ALL: [Group; 5] = [
        Group::Login,
        Group::Payments,
        Group::Renewal,
        Group::Support,
        Group::Service,
    ];

    pub const fn title(self) -> &'static str {
        match self {
            Group::Login => "Вход и почта",
            Group::Payments => "Оплата и подарки",
            Group::Renewal => "Автопродление",
            Group::Support => "Поддержка",
            Group::Service => "Безопасность и трафик",
        }
    }

    pub const fn switch(self) -> Option<&'static str> {
        match self {
            Group::Payments => Some("letter_payments"),
            Group::Renewal => Some("letter_expiring"),
            Group::Support => Some("letter_support"),
            Group::Login | Group::Service => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LetterId {
    LoginCode,
    ResetCode,
    EmailCode,
    SignupCode,
    SignupTaken,
    SubscriptionReady,
    SubscriptionRenewed,
    TopupDone,
    PaymentCredited,
    PaymentReview,
    PaymentRefunded,
    StarsRefunded,
    ChargedAfterOff,
    GiftReady,
    GiftRedeemed,
    RenewalShort,
    RenewalFailed,
    RenewalManual,
    RenewalOff,
    RenewalCancelled,
    SupportReply,
    LoginLinked,
    LoginsUnlinked,
    PasswordIssued,
    EmailChanged,
    TrafficWarning,
    TrafficRestricted,
    TrafficLifted,
    SubscriptionLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    Text(&'static str, &'static str),
    Money(i64),
    Stars(i64),
    Brand,
    Support,
    Site,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Var {
    pub name: &'static str,
    pub hint: &'static str,
    pub sample: Sample,
    pub always: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Texts {
    pub subject: &'static [&'static str],
    pub text: &'static [&'static str],
    pub button: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    pub id: LetterId,
    pub group: Group,
    pub title: &'static str,
    pub when: &'static str,
    pub vars: &'static [Var],
    pub required: &'static [&'static str],
    pub path: &'static [&'static str],
    pub ru: Texts,
    pub en: Texts,
}

impl Spec {
    pub fn texts(&self, locale: Locale) -> &Texts {
        match locale {
            Locale::Ru => &self.ru,
            Locale::En => &self.en,
        }
    }

    pub fn has_button(&self) -> bool {
        !self.path.is_empty()
    }

    pub fn all_vars(&self) -> impl Iterator<Item = &'static Var> + Clone + '_ {
        self.vars.iter().chain(COMMON.iter())
    }

    pub fn var(&self, name: &str) -> Option<&'static Var> {
        self.all_vars().find(|v| v.name == name)
    }
}

const fn var(name: &'static str, hint: &'static str, sample: Sample) -> Var {
    Var {
        name,
        hint,
        sample,
        always: true,
    }
}

const fn maybe(name: &'static str, hint: &'static str, sample: Sample) -> Var {
    Var {
        name,
        hint,
        sample,
        always: false,
    }
}

pub const COMMON: [Var; 2] = [
    var("brand", "название сервиса", Sample::Brand),
    var(
        "email",
        "почта клиента",
        Sample::Text("alex@example.com", "alex@example.com"),
    ),
];

pub const SIGN_VARS: [Var; 3] = [
    var("brand", "название сервиса", Sample::Brand),
    maybe("support", "ссылка на поддержку", Sample::Support),
    maybe("site", "адрес сайта", Sample::Site),
];

pub const SIGN_DEFAULT_RU: &str = "Команда {brand}";
pub const SIGN_DEFAULT_EN: &str = "The {brand} team";

pub fn sign_default(locale: Locale) -> &'static str {
    match locale {
        Locale::Ru => SIGN_DEFAULT_RU,
        Locale::En => SIGN_DEFAULT_EN,
    }
}

const CODE: Var = var("code", "код из письма", Sample::Text("482913", "482913"));
const MINUTES: Var = var(
    "minutes",
    "сколько действует код",
    Sample::Text("15 минут", "15 minutes"),
);
const TARIFF: Var = var("tariff", "тариф", Sample::Text("Стандарт", "Standard"));
const TARIFF_MAYBE: Var = maybe("tariff", "тариф", Sample::Text("Стандарт", "Standard"));
const DATE: Var = var(
    "date",
    "до какого числа",
    Sample::Text("14 февраля 2027", "14 February 2027"),
);
const AMOUNT: Var = var("amount", "сумма", Sample::Money(699));
const AMOUNT_MAYBE: Var = maybe("amount", "сумма", Sample::Money(699));
const TOPUP: Var = var("amount", "сумма пополнения", Sample::Money(1000));
const TOPUP_BONUS: Var = maybe("bonus", "бонус за пополнение", Sample::Money(100));
const TOPUP_TOTAL: Var = maybe(
    "total",
    "сколько пришло на баланс вместе с бонусом",
    Sample::Money(1100),
);
const DETAILS_CREDITED: Var = var(
    "details",
    "что случилось с оплатой",
    Sample::Text(
        "Оплата пришла с излишком: лишнее зачислено на баланс.",
        "You paid more than needed: the extra is on your balance.",
    ),
);
const DETAILS_REVIEW: Var = var(
    "details",
    "что случилось с оплатой",
    Sample::Text(
        "Оплата получена, но платёжная система задержала её для проверки. Заказ ждёт ручной проверки, мы свяжемся с вами.",
        "Payment received, but the payment system held it for a check. Your order is waiting for a manual review, we'll get in touch.",
    ),
);
const STARS: Var = var("stars", "сколько звёзд вернули", Sample::Stars(350));
const PURCHASE: Var = maybe(
    "purchase",
    "за что платили",
    Sample::Text("Стандарт · 90 дней", "Standard · 90 days"),
);
const TERM: Var = var("term", "срок", Sample::Text("3 месяца", "3 months"));
const GIFT_CODE: Var = var(
    "code",
    "код подарка",
    Sample::Text("GIFT-7Q4M-2KXP", "GIFT-7Q4M-2KXP"),
);
const GIFT_LINK: Var = maybe(
    "link",
    "ссылка на подарок в боте",
    Sample::Text(
        "https://t.me/your_bot?start=gift_7Q4M2KXP",
        "https://t.me/your_bot?start=gift_7Q4M2KXP",
    ),
);
const GIFT_URL: Var = maybe(
    "url",
    "ссылка на подарок на сайте",
    Sample::Text(
        "https://vpn.example.com/gift/7Q4M2KXP",
        "https://vpn.example.com/gift/7Q4M2KXP",
    ),
);
const GIFT_UNTIL: Var = maybe(
    "date",
    "до какого числа действует код",
    Sample::Text("14 февраля 2027", "14 February 2027"),
);
const MISSING: Var = var("missing", "сколько не хватает", Sample::Money(200));
const PRICE: Var = var("amount", "цена продления", Sample::Money(699));
const TICKET: Var = maybe("ticket", "номер обращения", Sample::Text("128", "128"));
const REPLY: Var = var(
    "reply",
    "текст ответа",
    Sample::Text(
        "Продлили подписку на 3 дня. Обновите подписку в приложении, и всё заработает.",
        "We added 3 days to your subscription. Refresh it in the app and everything will work.",
    ),
);
const PROVIDER: Var = var(
    "provider",
    "Google или Apple",
    Sample::Text("Google", "Google"),
);
const PAY_PROVIDER: Var = var(
    "provider",
    "платёжная система",
    Sample::Text("Lava.top", "Lava.top"),
);
const ACCOUNT: Var = var(
    "account",
    "привязанный аккаунт",
    Sample::Text("alex@example.com", "alex@example.com"),
);
const WHEN: Var = var(
    "when",
    "сегодня или на этой неделе",
    Sample::Text("Сегодня", "Today"),
);
const USED: Var = var("used", "сколько прошло", Sample::Text("112 ГБ", "112 GB"));
const LIMIT: Var = var("limit", "лимит", Sample::Text("100 ГБ", "100 GB"));
const PER: Var = var(
    "per",
    "за день или за неделю",
    Sample::Text("за день", "per day"),
);
const UNTIL: Var = var(
    "until",
    "до какого времени ограничено",
    Sample::Text("15 февраля, 10:00", "15 February, 10:00"),
);

const SUB_URL: Var = var(
    "url",
    "новая ссылка подписки",
    Sample::Text(
        "https://sub.example.com/7Q4M2KXP",
        "https://sub.example.com/7Q4M2KXP",
    ),
);

const NEW_EMAIL: Var = var(
    "new_email",
    "новый адрес, частично скрыт",
    Sample::Text("n***@example.com", "n***@example.com"),
);

pub const CATALOG: [Spec; 29] = [
    Spec {
        id: LetterId::LoginCode,
        group: Group::Login,
        title: "Код для входа",
        when: "Клиент входит на сайт по почте",
        vars: &[CODE, MINUTES],
        required: &["code"],
        path: &[],
        ru: Texts {
            subject: &["Код для входа в {brand}: {code}"],
            text: &[
                "Код для входа в кабинет {brand}:\n{code}\n\nКод действует {minutes}. Если вход запрашивали не вы, просто не используйте его.",
            ],
            button: "",
        },
        en: Texts {
            subject: &["Your {brand} sign-in code: {code}"],
            text: &[
                "Your code to sign in to {brand}:\n{code}\n\nIt works for {minutes}. If you didn't ask to sign in, just ignore it.",
            ],
            button: "",
        },
    },
    Spec {
        id: LetterId::ResetCode,
        group: Group::Login,
        title: "Код для смены пароля",
        when: "Клиент забыл пароль",
        vars: &[CODE, MINUTES],
        required: &["code"],
        path: &[],
        ru: Texts {
            subject: &["Код для смены пароля в {brand}: {code}"],
            text: &[
                "Код для смены пароля в кабинете {brand}:\n{code}\n\nКод действует {minutes}. Если пароль меняете не вы, не используйте код — старый пароль продолжит работать.",
            ],
            button: "",
        },
        en: Texts {
            subject: &["Your {brand} password reset code: {code}"],
            text: &[
                "Your code to reset the {brand} password:\n{code}\n\nIt works for {minutes}. If you didn't ask for this, ignore the code and your old password keeps working.",
            ],
            button: "",
        },
    },
    Spec {
        id: LetterId::EmailCode,
        group: Group::Login,
        title: "Код для смены почты",
        when: "Клиент меняет или добавляет почту",
        vars: &[CODE, MINUTES],
        required: &["code"],
        path: &[],
        ru: Texts {
            subject: &["Код для смены почты в {brand}: {code}"],
            text: &[
                "Код для смены почты в {brand}:\n{code}\n\nКод действует {minutes}. Если почту меняете не вы, просто не используйте код: адрес не привяжется к аккаунту.",
            ],
            button: "",
        },
        en: Texts {
            subject: &["Your {brand} email change code: {code}"],
            text: &[
                "Your code to change the email on {brand}:\n{code}\n\nIt works for {minutes}. If you didn't ask for this, just ignore it and this address won't be added to the account.",
            ],
            button: "",
        },
    },
    Spec {
        id: LetterId::SignupCode,
        group: Group::Login,
        title: "Код для регистрации",
        when: "Клиент регистрируется на сайте по почте и паролю, а в «Почте» включено подтверждение почты",
        vars: &[CODE, MINUTES],
        required: &["code"],
        path: &[],
        ru: Texts {
            subject: &["Код для регистрации в {brand}: {code}"],
            text: &[
                "Код для регистрации в {brand}:\n{code}\n\nКод действует {minutes}. Аккаунт появится, когда вы введёте код на сайте. Если регистрируетесь не вы, просто не используйте код: аккаунт не создастся.",
            ],
            button: "",
        },
        en: Texts {
            subject: &["Your {brand} sign-up code: {code}"],
            text: &[
                "Your code to finish signing up for {brand}:\n{code}\n\nIt works for {minutes}. Your account is created once you enter the code on the site. If you didn't sign up, just ignore it and no account will be created.",
            ],
            button: "",
        },
    },
    Spec {
        id: LetterId::SignupTaken,
        group: Group::Login,
        title: "Почта уже зарегистрирована",
        when: "Кто-то регистрируется на сайте с почтой, у которой уже есть аккаунт, а в «Почте» включено подтверждение почты",
        vars: &[],
        required: &[],
        path: &["/cabinet"],
        ru: Texts {
            subject: &["Аккаунт {brand} с этой почтой уже есть"],
            text: &[
                "С адресом {email} пытаются зарегистрироваться в {brand}. Аккаунт с этой почтой уже есть, второй создать нельзя.\n\nЕсли это вы, войдите в кабинет с паролем. Если пароля нет или вы его не помните, нажмите «Забыли пароль?» на странице входа: на почту придёт код, с ним вы зададите новый пароль.\n\nЕсли регистрируетесь не вы, ничего делать не нужно: доступ к аккаунту не изменился.",
            ],
            button: "Войти",
        },
        en: Texts {
            subject: &["You already have a {brand} account"],
            text: &[
                "Someone is trying to sign up for {brand} with {email}. There is already an account with this email, so a second one can't be created.\n\nIf it's you, sign in with your password. If you don't have one or don't remember it, tap “Forgot password?” on the sign-in page: we'll email you a code to set a new password.\n\nIf it wasn't you, there's nothing to do: access to your account hasn't changed.",
            ],
            button: "Sign in",
        },
    },
    Spec {
        id: LetterId::SubscriptionReady,
        group: Group::Payments,
        title: "Подписка готова",
        when: "Оплата прошла, подписка выдана",
        vars: &[TARIFF_MAYBE, DATE],
        required: &[],
        path: &["/cabinet"],
        ru: Texts {
            subject: &["Подписка готова"],
            text: &[
                "Подписка готова и действует до {date}. Ссылка для подключения — в личном кабинете.",
            ],
            button: "Открыть кабинет",
        },
        en: Texts {
            subject: &["Your subscription is ready"],
            text: &[
                "Your subscription is ready and runs until {date}. The connection link is in your account.",
            ],
            button: "Open your account",
        },
    },
    Spec {
        id: LetterId::SubscriptionRenewed,
        group: Group::Payments,
        title: "Подписка продлена",
        when: "Продление оплачено или дни добавлены вручную",
        vars: &[TARIFF_MAYBE, DATE],
        required: &[],
        path: &["/cabinet"],
        ru: Texts {
            subject: &["Подписка продлена"],
            text: &["Подписка продлена до {date}."],
            button: "Открыть кабинет",
        },
        en: Texts {
            subject: &["Subscription renewed"],
            text: &["Your subscription is renewed until {date}."],
            button: "Open your account",
        },
    },
    Spec {
        id: LetterId::TopupDone,
        group: Group::Payments,
        title: "Баланс пополнен",
        when: "Пополнение баланса оплачено",
        vars: &[TOPUP, TOPUP_BONUS, TOPUP_TOTAL],
        required: &[],
        path: &["/cabinet/payments"],
        ru: Texts {
            subject: &["Баланс пополнен"],
            text: &[
                "На баланс пришло {total}: {amount} и бонус {bonus}.",
                "Баланс пополнен на {amount}.",
            ],
            button: "История оплат",
        },
        en: Texts {
            subject: &["Balance topped up"],
            text: &[
                "{total} is on your balance: {amount} and a {bonus} bonus.",
                "Your balance is topped up by {amount}.",
            ],
            button: "Payment history",
        },
    },
    Spec {
        id: LetterId::PaymentCredited,
        group: Group::Payments,
        title: "Деньги зачислены на баланс",
        when: "Оплата пришла не той суммой или повторно",
        vars: &[AMOUNT, DETAILS_CREDITED],
        required: &[],
        path: &["/cabinet/payments"],
        ru: Texts {
            subject: &["Деньги зачислены на баланс"],
            text: &["{details}"],
            button: "История оплат",
        },
        en: Texts {
            subject: &["Money added to your balance"],
            text: &["{details}"],
            button: "Payment history",
        },
    },
    Spec {
        id: LetterId::PaymentReview,
        group: Group::Payments,
        title: "Оплату проверим вручную",
        when: "Платёж задержали для проверки",
        vars: &[AMOUNT, DETAILS_REVIEW],
        required: &[],
        path: &["/cabinet/payments"],
        ru: Texts {
            subject: &["Оплату проверим вручную"],
            text: &["{details}"],
            button: "История оплат",
        },
        en: Texts {
            subject: &["We'll check your payment by hand"],
            text: &["{details}"],
            button: "Payment history",
        },
    },
    Spec {
        id: LetterId::PaymentRefunded,
        group: Group::Payments,
        title: "Оплата возвращена",
        when: "Оплата от заблокированного аккаунта",
        vars: &[AMOUNT_MAYBE],
        required: &[],
        path: &["/"],
        ru: Texts {
            subject: &["Оплата возвращена"],
            text: &[
                "Оплата {amount} возвращена: аккаунт заблокирован, заказ не выдан.",
                "Оплата возвращена: аккаунт заблокирован, заказ не выдан.",
            ],
            button: "Открыть сайт",
        },
        en: Texts {
            subject: &["Payment refunded"],
            text: &[
                "Payment of {amount} refunded: the account is blocked and the order wasn't delivered.",
                "Payment refunded: the account is blocked and the order wasn't delivered.",
            ],
            button: "Open the website",
        },
    },
    Spec {
        id: LetterId::StarsRefunded,
        group: Group::Payments,
        title: "Звёзды возвращены",
        when: "Telegram вернул звёзды за покупку",
        vars: &[STARS, PURCHASE],
        required: &[],
        path: &["/cabinet/payments"],
        ru: Texts {
            subject: &["Звёзды возвращены"],
            text: &[
                "Звёзды за покупку «{purchase}» возвращены: {stars}.",
                "Звёзды возвращены: {stars}.",
            ],
            button: "История оплат",
        },
        en: Texts {
            subject: &["Stars refunded"],
            text: &[
                "Stars for “{purchase}” refunded: {stars}.",
                "Stars refunded: {stars}.",
            ],
            button: "Payment history",
        },
    },
    Spec {
        id: LetterId::ChargedAfterOff,
        group: Group::Payments,
        title: "Списание до выключения автопродления",
        when: "Деньги списались раньше, чем выключили автопродление",
        vars: &[AMOUNT_MAYBE, DATE],
        required: &[],
        path: &["/cabinet/help"],
        ru: Texts {
            subject: &["Списание прошло до выключения автопродления"],
            text: &[
                "Списание {amount} за продление ушло до того, как автопродление выключили, поэтому подписка продлена до {date}. Если продление не нужно, напишите в поддержку — вернём деньги.",
                "Списание за продление ушло до того, как автопродление выключили, поэтому подписка продлена до {date}. Если продление не нужно, напишите в поддержку — вернём деньги.",
            ],
            button: "Написать в поддержку",
        },
        en: Texts {
            subject: &["Charged before auto-renewal was turned off"],
            text: &[
                "The {amount} renewal charge went through before auto-renewal was turned off, so your subscription is renewed until {date}. If you don't need it, contact support and we'll refund you.",
                "The renewal charge went through before auto-renewal was turned off, so your subscription is renewed until {date}. If you don't need it, contact support and we'll refund you.",
            ],
            button: "Contact support",
        },
    },
    Spec {
        id: LetterId::GiftReady,
        group: Group::Payments,
        title: "Подарок оплачен",
        when: "Клиент купил подписку в подарок",
        vars: &[TARIFF, TERM, GIFT_CODE, GIFT_LINK, GIFT_URL, GIFT_UNTIL],
        required: &[],
        path: &["/cabinet/gifts"],
        ru: Texts {
            subject: &["Подарок оплачен"],
            text: &[
                "Подарок оплачен: «{tariff}» на {term}.\nКод подарка: <code>{code}</code>\n\nПередайте код другу: он активирует подарок в личном кабинете или в боте.",
            ],
            button: "Мои подарки",
        },
        en: Texts {
            subject: &["Gift paid"],
            text: &[
                "Gift paid: “{tariff}” for {term}.\nGift code: <code>{code}</code>\n\nSend the code to your friend: they activate the gift in their account or in the bot.",
            ],
            button: "My gifts",
        },
    },
    Spec {
        id: LetterId::GiftRedeemed,
        group: Group::Payments,
        title: "Подарок активировали",
        when: "Код подарка ввели",
        vars: &[GIFT_CODE, TARIFF, TERM],
        required: &[],
        path: &["/cabinet/gifts"],
        ru: Texts {
            subject: &["Ваш подарок активировали"],
            text: &["Ваш подарок <code>{code}</code> активировали: «{tariff}» на {term}."],
            button: "Мои подарки",
        },
        en: Texts {
            subject: &["Your gift was activated"],
            text: &["Your gift <code>{code}</code> was activated: “{tariff}” for {term}."],
            button: "My gifts",
        },
    },
    Spec {
        id: LetterId::RenewalShort,
        group: Group::Renewal,
        title: "Не хватает на автопродление",
        when: "Продление скоро, а на балансе меньше его цены",
        vars: &[DATE, MISSING, PRICE],
        required: &[],
        path: &["/cabinet/topup"],
        ru: Texts {
            subject: &["Пополните баланс для автопродления"],
            text: &[
                "Подписка действует до {date}. Для автопродления на балансе не хватает {missing} — пополните его, и подписка продлится сама.",
            ],
            button: "Пополнить баланс",
        },
        en: Texts {
            subject: &["Top up your balance for auto-renewal"],
            text: &[
                "Your subscription runs until {date}. Auto-renewal needs {missing} more on your balance — top it up and the subscription renews by itself.",
            ],
            button: "Top up balance",
        },
    },
    Spec {
        id: LetterId::RenewalFailed,
        group: Group::Renewal,
        title: "Автопродление не прошло",
        when: "Списание за продление не прошло",
        vars: &[DATE, PRICE],
        required: &[],
        path: &["/cabinet/topup"],
        ru: Texts {
            subject: &["Подписку не удалось продлить"],
            text: &[
                "Не получилось оплатить продление подписки. Пополните баланс — продлим автоматически. Подписка действует до {date}.",
            ],
            button: "Пополнить баланс",
        },
        en: Texts {
            subject: &["Couldn't renew your subscription"],
            text: &[
                "We couldn't pay for the renewal. Top up your balance and we'll renew it automatically. Your subscription runs until {date}.",
            ],
            button: "Top up balance",
        },
    },
    Spec {
        id: LetterId::RenewalManual,
        group: Group::Renewal,
        title: "Продлите вручную",
        when: "Автопродление не сработает: оплата с баланса выключена",
        vars: &[DATE],
        required: &[],
        path: &["/cabinet/pay"],
        ru: Texts {
            subject: &["Продлите подписку вручную"],
            text: &[
                "Продлить подписку автоматически не получилось. Продлите её до {date}, чтобы VPN не отключился.",
            ],
            button: "Продлить",
        },
        en: Texts {
            subject: &["Renew your subscription by hand"],
            text: &["Automatic renewal didn't go through. Renew by {date} to keep the VPN on."],
            button: "Renew",
        },
    },
    Spec {
        id: LetterId::RenewalOff,
        group: Group::Renewal,
        title: "Автопродление выключено",
        when: "Оплатить продление так и не удалось",
        vars: &[DATE],
        required: &[],
        path: &["/cabinet/pay"],
        ru: Texts {
            subject: &["Автопродление выключено"],
            text: &[
                "Автопродление выключено: оплатить продление не удалось. Подписка действует до {date}, продлите её вручную.",
            ],
            button: "Продлить",
        },
        en: Texts {
            subject: &["Auto-renewal is off"],
            text: &[
                "Auto-renewal is off because the payment failed. Your subscription runs until {date}, renew it by hand.",
            ],
            button: "Renew",
        },
    },
    Spec {
        id: LetterId::RenewalCancelled,
        group: Group::Renewal,
        title: "Автопродление через платёжку отключено",
        when: "Автопродление через платёжную систему отключил оператор, оно отключилось из-за смены тарифа или его отключили в самой платёжной системе",
        vars: &[PAY_PROVIDER, DATE],
        required: &[],
        path: &["/cabinet"],
        ru: Texts {
            subject: &["Автопродление отключено"],
            text: &[
                "Автопродление через {provider} отключено, больше списаний не будет. Подписка действует до {date}, дальше её можно продлить вручную.",
            ],
            button: "Открыть подписку",
        },
        en: Texts {
            subject: &["Auto-renewal is off"],
            text: &[
                "Auto-renewal via {provider} is off, there will be no more charges. Your subscription runs until {date}, after that you can renew it by hand.",
            ],
            button: "Open subscription",
        },
    },
    Spec {
        id: LetterId::SupportReply,
        group: Group::Support,
        title: "Ответ поддержки",
        when: "Ответили на обращение клиента",
        vars: &[TICKET, REPLY],
        required: &[],
        path: &["/cabinet/help/{ticket}", "/cabinet/help"],
        ru: Texts {
            subject: &["Ответ поддержки по обращению №{ticket}", "Ответ поддержки"],
            text: &["{reply}"],
            button: "Открыть переписку",
        },
        en: Texts {
            subject: &["Support reply to request #{ticket}", "Support reply"],
            text: &["{reply}"],
            button: "Open the conversation",
        },
    },
    Spec {
        id: LetterId::LoginLinked,
        group: Group::Service,
        title: "Привязан вход через Google, Apple или Telegram",
        when: "К аккаунту добавили новый способ входа",
        vars: &[PROVIDER, ACCOUNT],
        required: &[],
        path: &["/cabinet/security"],
        ru: Texts {
            subject: &["Привязан вход через {provider}"],
            text: &[
                "К вашему аккаунту привязан вход через {provider}: {account}. Если это сделали не вы, сразу напишите в поддержку.",
            ],
            button: "Безопасность",
        },
        en: Texts {
            subject: &["Sign-in with {provider} linked"],
            text: &[
                "Sign-in with {provider} is now linked to your account: {account}. If this wasn't you, contact support right away.",
            ],
            button: "Security",
        },
    },
    Spec {
        id: LetterId::LoginsUnlinked,
        group: Group::Service,
        title: "Отвязан вход через Google или Apple",
        when: "Клиент подтвердил почту кодом, и способы входа, привязанные до подтверждения, отвязаны",
        vars: &[],
        required: &[],
        path: &["/cabinet/security"],
        ru: Texts {
            subject: &["Мы отвязали вход через Google или Apple"],
            text: &[
                "Вы подтвердили почту кодом, поэтому мы отвязали вход через Google или Apple, привязанный до подтверждения, и завершили все сеансы. Если этот вход привязывали вы, привяжите его снова в разделе «Безопасность».",
            ],
            button: "Безопасность",
        },
        en: Texts {
            subject: &["We unlinked sign-in with Google or Apple"],
            text: &[
                "You confirmed your email with a code, so we unlinked the Google or Apple sign-in that was added before the confirmation and signed out all sessions. If you added it yourself, link it again under Security.",
            ],
            button: "Security",
        },
    },
    Spec {
        id: LetterId::PasswordIssued,
        group: Group::Service,
        title: "Выдан пароль для сайта",
        when: "Администратор выдал клиенту временный пароль для входа на сайт",
        vars: &[],
        required: &[],
        path: &["/cabinet/security"],
        ru: Texts {
            subject: &["Вам выдали пароль для входа на сайт"],
            text: &[
                "Администратор выдал вам временный пароль для входа на сайт. При первом входе с ним задайте свой пароль. Если вы не просили новый пароль, сразу напишите в поддержку.",
            ],
            button: "Безопасность",
        },
        en: Texts {
            subject: &["A password for the website was issued to you"],
            text: &[
                "An administrator issued you a temporary password for the website. Set your own password when you first sign in with it. If you didn't ask for a new password, contact support right away.",
            ],
            button: "Security",
        },
    },
    Spec {
        id: LetterId::EmailChanged,
        group: Group::Service,
        title: "Почта изменена",
        when: "На прежний адрес после смены почты",
        vars: &[NEW_EMAIL],
        required: &[],
        path: &[],
        ru: Texts {
            subject: &["Почта в {brand} изменена"],
            text: &[
                "Почту в аккаунте {brand} сменили на {new_email}. Письма теперь приходят на новый адрес, а этот больше не привязан к аккаунту.\n\nЕсли почту меняли не вы, напишите в поддержку {brand}.",
            ],
            button: "",
        },
        en: Texts {
            subject: &["Your {brand} email was changed"],
            text: &[
                "The email on your {brand} account was changed to {new_email}. Emails now go to the new address, and this one is no longer linked to the account.\n\nIf you didn't change it, please contact {brand} support.",
            ],
            button: "",
        },
    },
    Spec {
        id: LetterId::TrafficWarning,
        group: Group::Service,
        title: "Превышен лимит трафика",
        when: "Трафик выше порога, пока без ограничений",
        vars: &[WHEN, USED, LIMIT, PER],
        required: &[],
        path: &["/cabinet"],
        ru: Texts {
            subject: &["Превышен лимит трафика"],
            text: &[
                "{when} через VPN прошло {used} — больше лимита {limit} {per}. Пожалуйста, снизьте нагрузку, иначе доступ могут временно ограничить.",
            ],
            button: "Открыть кабинет",
        },
        en: Texts {
            subject: &["Traffic limit exceeded"],
            text: &[
                "{when} you've used {used}, over the {limit} {per} limit. Please ease off, or access may be restricted for a while.",
            ],
            button: "Open your account",
        },
    },
    Spec {
        id: LetterId::TrafficRestricted,
        group: Group::Service,
        title: "Доступ временно ограничен",
        when: "Трафик выше лимита, доступ на паузе",
        vars: &[WHEN, USED, LIMIT, PER, UNTIL],
        required: &[],
        path: &["/cabinet"],
        ru: Texts {
            subject: &["Доступ временно ограничен"],
            text: &[
                "{when} через VPN прошло {used} при лимите {limit} {per}. Доступ временно ограничен до {until}, потом включится сам.",
            ],
            button: "Открыть кабинет",
        },
        en: Texts {
            subject: &["Access temporarily restricted"],
            text: &[
                "{when} you've used {used} with a limit of {limit} {per}. Access is restricted until {until} and comes back on its own.",
            ],
            button: "Open your account",
        },
    },
    Spec {
        id: LetterId::TrafficLifted,
        group: Group::Service,
        title: "Ограничение по трафику снято",
        when: "Доступ снова открыт",
        vars: &[],
        required: &[],
        path: &["/cabinet"],
        ru: Texts {
            subject: &["Ограничение по трафику снято"],
            text: &["Ограничение по трафику снято, VPN снова работает."],
            button: "Открыть кабинет",
        },
        en: Texts {
            subject: &["Traffic restriction lifted"],
            text: &["The traffic restriction is lifted, the VPN works again."],
            button: "Open your account",
        },
    },
    Spec {
        id: LetterId::SubscriptionLink,
        group: Group::Service,
        title: "Новая ссылка на подписку",
        when: "После переноса в Remnawave у подписки сменилась ссылка",
        vars: &[SUB_URL],
        required: &["url"],
        path: &["/cabinet"],
        ru: Texts {
            subject: &["Новая ссылка на подписку {brand}"],
            text: &[
                "Ссылка на вашу подписку сменилась, старая больше не работает. Новая ссылка:\n<code>{url}</code>\n\nДобавьте её в приложение заново.",
            ],
            button: "Открыть кабинет",
        },
        en: Texts {
            subject: &["Your new {brand} subscription link"],
            text: &[
                "Your subscription link has changed and the old one no longer works. Your new link:\n<code>{url}</code>\n\nAdd it to your app again.",
            ],
            button: "Open your account",
        },
    },
];

impl LetterId {
    pub const fn as_str(self) -> &'static str {
        match self {
            LetterId::LoginCode => "login_code",
            LetterId::ResetCode => "reset_code",
            LetterId::EmailCode => "email_code",
            LetterId::SignupCode => "signup_code",
            LetterId::SignupTaken => "signup_taken",
            LetterId::SubscriptionReady => "subscription_ready",
            LetterId::SubscriptionRenewed => "subscription_renewed",
            LetterId::TopupDone => "topup_done",
            LetterId::PaymentCredited => "payment_credited",
            LetterId::PaymentReview => "payment_review",
            LetterId::PaymentRefunded => "payment_refunded",
            LetterId::StarsRefunded => "stars_refunded",
            LetterId::ChargedAfterOff => "charged_after_off",
            LetterId::GiftReady => "gift_ready",
            LetterId::GiftRedeemed => "gift_redeemed",
            LetterId::RenewalShort => "renewal_short",
            LetterId::RenewalFailed => "renewal_failed",
            LetterId::RenewalManual => "renewal_manual",
            LetterId::RenewalOff => "renewal_off",
            LetterId::RenewalCancelled => "renewal_cancelled",
            LetterId::SupportReply => "support_reply",
            LetterId::LoginLinked => "login_linked",
            LetterId::LoginsUnlinked => "logins_unlinked",
            LetterId::PasswordIssued => "password_issued",
            LetterId::EmailChanged => "email_changed",
            LetterId::TrafficWarning => "traffic_warning",
            LetterId::TrafficRestricted => "traffic_restricted",
            LetterId::TrafficLifted => "traffic_lifted",
            LetterId::SubscriptionLink => "subscription_link",
        }
    }

    pub fn parse(raw: &str) -> Option<LetterId> {
        CATALOG.iter().map(|s| s.id).find(|id| id.as_str() == raw)
    }

    pub fn spec(self) -> &'static Spec {
        CATALOG.iter().find(|s| s.id == self).unwrap_or(&CATALOG[0])
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values(Vec<(&'static str, String)>);

impl Values {
    pub fn new() -> Self {
        Values(Vec::new())
    }

    pub fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.set(name, value);
        self
    }

    pub fn maybe(mut self, name: &'static str, value: Option<String>) -> Self {
        if let Some(value) = value {
            self.set(name, value);
        }
        self
    }

    pub fn set(&mut self, name: &'static str, value: impl Into<String>) {
        let value = value.into();
        let value = value.trim();
        if value.is_empty() {
            self.0.retain(|(n, _)| *n != name);
            return;
        }
        match self.0.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value.to_string(),
            None => self.0.push((name, value.to_string())),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }
}

fn is_name(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= NAME_MAX
        && raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn braces(template: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find(['{', '}']) {
            Some(end) if after[end..].starts_with('}') => {
                let inner = &after[..end];
                if !inner.is_empty()
                    && inner.chars().count() <= NAME_MAX
                    && !inner.chars().any(char::is_whitespace)
                {
                    found.push(inner);
                }
                rest = &after[end + 1..];
            }
            Some(end) => rest = &after[end..],
            None => break,
        }
    }
    found
}

pub fn used(template: &str) -> Vec<&str> {
    braces(template)
        .into_iter()
        .filter(|n| is_name(n))
        .collect()
}

fn fill(template: &str, values: &Values, html: bool) -> Option<String> {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if is_name(&after[..end]) => {
                let value = values.get(&after[..end])?;
                if html {
                    out.push_str(&escape(value));
                } else {
                    out.push_str(value);
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
    Some(out)
}

pub fn fill_plain(template: &str, values: &Values) -> Option<String> {
    fill(template, values, false)
}

pub fn one_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn fill_html(template: &str, values: &Values) -> Option<String> {
    fill(template, values, true)
}

pub fn first_plain<'a>(
    candidates: impl IntoIterator<Item = &'a str>,
    values: &Values,
) -> Option<String> {
    candidates
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .find_map(|t| fill_plain(t, values))
}

pub fn first_html<'a>(
    candidates: impl IntoIterator<Item = &'a str>,
    values: &Values,
) -> Option<String> {
    candidates
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .find_map(|t| fill_html(t, values))
}

pub fn sample(var: &Var, locale: Locale, currency: Currency, brand: &str) -> String {
    match var.sample {
        Sample::Text(ru, en) => match locale {
            Locale::Ru => ru.to_string(),
            Locale::En => en.to_string(),
        },
        Sample::Money(minor) => format::money_in(Money { minor, currency }, locale),
        Sample::Stars(count) => format::stars_in(Stars(count), locale),
        Sample::Brand => brand.to_string(),
        Sample::Support => "https://t.me/your_support".to_string(),
        Sample::Site => "https://vpn.example.com".to_string(),
    }
}

pub fn samples(spec: &Spec, locale: Locale, currency: Currency, brand: &str) -> Values {
    let mut values = Values::new();
    for var in spec.all_vars() {
        values.set(var.name, sample(var, locale, currency, brand));
    }
    values
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Subject,
    Text,
    Button,
    Sign,
}

impl Field {
    pub const fn title(self) -> &'static str {
        match self {
            Field::Subject => "тема",
            Field::Text => "текст",
            Field::Button => "кнопка",
            Field::Sign => "подпись",
        }
    }

    pub const fn max(self) -> usize {
        match self {
            Field::Subject => SUBJECT_MAX,
            Field::Text => TEXT_MAX,
            Field::Button => BUTTON_MAX,
            Field::Sign => SIGN_MAX,
        }
    }

    const fn one_line(self) -> bool {
        matches!(self, Field::Subject | Field::Button)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub field: Field,
    pub message: String,
}

fn problem(field: Field, message: impl Into<String>) -> Problem {
    Problem {
        field,
        message: message.into(),
    }
}

fn check_names<'v>(
    field: Field,
    raw: &str,
    allowed: impl Iterator<Item = &'v Var> + Clone,
) -> Result<(), Problem> {
    for name in braces(raw) {
        if allowed.clone().any(|v| v.name == name) {
            continue;
        }
        let mut list: Vec<String> = Vec::new();
        for var in allowed.clone() {
            let token = format!("{{{}}}", var.name);
            if !list.contains(&token) {
                list.push(token);
            }
        }
        let start = if is_name(name) {
            format!("подстановки {{{name}}} в этом письме нет")
        } else {
            format!("подстановка {{{name}}} записана неверно")
        };
        return Err(problem(
            field,
            if list.is_empty() {
                start
            } else {
                format!("{start}, можно: {}", list.join(", "))
            },
        ));
    }
    Ok(())
}

fn clean_field<'v>(
    field: Field,
    raw: &str,
    allowed: impl Iterator<Item = &'v Var> + Clone,
) -> Result<Option<String>, Problem> {
    let normalized = raw.replace("\r\n", "\n");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if field.one_line() && trimmed.contains('\n') {
        return Err(problem(field, "нужна одна строка без переносов"));
    }
    if trimmed.chars().count() > field.max() {
        return Err(problem(field, format!("длиннее {} символов", field.max())));
    }
    check_names(field, trimmed, allowed)?;
    if field.one_line() {
        return Ok(Some(trimmed.to_string()));
    }
    normalize_markup(trimmed)
        .map(Some)
        .map_err(|p| problem(field, p.message()))
}

pub fn clean(spec: &Spec, field: Field, raw: &str) -> Result<Option<String>, Problem> {
    if field == Field::Button && !spec.has_button() {
        return Ok(None);
    }
    let cleaned = clean_field(field, raw, spec.all_vars())?;
    if field == Field::Text
        && let Some(text) = &cleaned
    {
        let names = used(text);
        for name in spec.required {
            if !names.contains(name) {
                return Err(problem(
                    field,
                    format!(
                        "нужна подстановка {{{name}}}: без неё клиент не увидит {}",
                        spec.var(name).map(|v| v.hint).unwrap_or("значение")
                    ),
                ));
            }
        }
    }
    Ok(cleaned)
}

pub fn clean_sign(raw: &str) -> Result<Option<String>, Problem> {
    clean_field(Field::Sign, raw, SIGN_VARS.iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> Values {
        Values::new()
            .with("tariff", "Стандарт <VPN> & ко")
            .with("date", "14 февраля 2027")
            .with("brand", "STEALTHNET")
            .with("email", "alex@example.com")
    }

    #[test]
    fn подстановки_заменяются_а_в_html_экранируются() {
        let template = "Подписка «{tariff}» до {date}";
        assert_eq!(
            fill_html(template, &values()).unwrap(),
            "Подписка «Стандарт &lt;VPN&gt; &amp; ко» до 14 февраля 2027"
        );
        assert_eq!(
            fill_plain(template, &values()).unwrap(),
            "Подписка «Стандарт <VPN> & ко» до 14 февраля 2027"
        );
        assert_eq!(
            fill_plain("{ без подстановки }", &values()).unwrap(),
            "{ без подстановки }"
        );
    }

    #[test]
    fn без_значения_берётся_следующий_вариант() {
        let spec = LetterId::SupportReply.spec();
        let without = Values::new().with("reply", "Ответ");
        assert_eq!(
            first_plain(spec.ru.subject.iter().copied(), &without).unwrap(),
            "Ответ поддержки"
        );
        let with = without.with("ticket", "41");
        assert_eq!(
            first_plain(spec.ru.subject.iter().copied(), &with).unwrap(),
            "Ответ поддержки по обращению №41"
        );
        assert_eq!(
            first_plain(["Тариф {tariff}", "Без тарифа"], &Values::new()).unwrap(),
            "Без тарифа"
        );
        assert!(first_plain(["{tariff}"], &Values::new()).is_none());
    }

    #[test]
    fn неизвестная_подстановка_даёт_понятную_ошибку() {
        let spec = LetterId::SubscriptionReady.spec();
        let typo = clean(spec, Field::Text, "Подписка «{tarif}» готова").unwrap_err();
        assert_eq!(typo.field, Field::Text);
        assert_eq!(
            typo.message,
            "подстановки {tarif} в этом письме нет, можно: {tariff}, {date}, {brand}, {email}"
        );
        let russian = clean(spec, Field::Subject, "Подписка {тариф}").unwrap_err();
        assert!(
            russian
                .message
                .starts_with("подстановка {тариф} записана неверно"),
            "{}",
            russian.message
        );
        let alien = clean(LetterId::LoginCode.spec(), Field::Subject, "Код {tariff}").unwrap_err();
        assert!(
            alien.message.contains("{code}, {minutes}"),
            "{}",
            alien.message
        );
        assert_eq!(
            clean(spec, Field::Text, "Подписка «{tariff}» до {date}").unwrap(),
            Some("Подписка «{tariff}» до {date}".into())
        );
        assert!(clean(spec, Field::Text, "Скобки { как текст }").is_ok());
    }

    #[test]
    fn код_нельзя_убрать_из_письма_с_кодом() {
        let spec = LetterId::LoginCode.spec();
        let lost = clean(spec, Field::Text, "Войдите по коду из приложения").unwrap_err();
        assert!(
            lost.message.starts_with("нужна подстановка {code}"),
            "{}",
            lost.message
        );
        assert!(clean(spec, Field::Text, "Ваш код: {code}").is_ok());
        assert!(clean(spec, Field::Subject, "Код для входа").is_ok());
    }

    #[test]
    fn длина_строки_и_разметка_проверяются() {
        let spec = LetterId::SubscriptionReady.spec();
        let long = "а".repeat(SUBJECT_MAX + 1);
        assert_eq!(
            clean(spec, Field::Subject, &long).unwrap_err().message,
            format!("длиннее {SUBJECT_MAX} символов")
        );
        assert!(clean(spec, Field::Text, &"б".repeat(TEXT_MAX)).is_ok());
        assert!(clean(spec, Field::Text, &"б".repeat(TEXT_MAX + 1)).is_err());
        assert_eq!(
            clean(spec, Field::Subject, "Две\nстроки")
                .unwrap_err()
                .message,
            "нужна одна строка без переносов"
        );
        assert_eq!(
            clean(spec, Field::Text, "<b>жирный").unwrap_err().message,
            "тег <b> не закрыт"
        );
        assert_eq!(
            clean(spec, Field::Text, "<script>x</script> & <b>да</b>").unwrap(),
            Some("&lt;script&gt;x&lt;/script&gt; &amp; <b>да</b>".into())
        );
        assert_eq!(clean(spec, Field::Text, "   \n ").unwrap(), None);
        assert_eq!(
            clean(LetterId::LoginCode.spec(), Field::Button, "Кнопка").unwrap(),
            None
        );
        assert!(clean_sign("Команда {brand}\n{support}").is_ok());
        assert!(clean_sign("Команда {tariff}").is_err());
    }

    #[test]
    fn тексты_по_умолчанию_собираются_без_необязательных_подстановок() {
        for spec in &CATALOG {
            for locale in [Locale::Ru, Locale::En] {
                let texts = spec.texts(locale);
                let always = spec
                    .all_vars()
                    .filter(|v| v.always)
                    .fold(Values::new(), |values, v| {
                        values.with(v.name, sample(v, locale, Currency::Usd, "STEALTHNET"))
                    });
                assert!(
                    first_plain(texts.subject.iter().copied(), &always).is_some(),
                    "{:?} {locale:?}: тема",
                    spec.id
                );
                assert!(
                    first_html(texts.text.iter().copied(), &always).is_some(),
                    "{:?} {locale:?}: текст",
                    spec.id
                );
                assert_eq!(texts.button.is_empty(), !spec.has_button(), "{:?}", spec.id);
                for candidate in texts.subject.iter().chain(texts.text) {
                    let field = if texts.subject.contains(candidate) {
                        Field::Subject
                    } else {
                        Field::Text
                    };
                    assert_eq!(
                        clean(spec, field, candidate).map(|c| c.is_some()),
                        Ok(true),
                        "{:?} {locale:?}: {candidate}",
                        spec.id
                    );
                }
            }
            if spec.has_button() {
                assert!(
                    first_plain(spec.path.iter().copied(), &Values::new()).is_some(),
                    "{:?}: ссылка кнопки",
                    spec.id
                );
            }
            assert_eq!(LetterId::parse(spec.id.as_str()), Some(spec.id));
            assert_eq!(spec.id.spec().id, spec.id);
        }
        assert!(clean_sign(SIGN_DEFAULT_RU).unwrap().is_some());
        assert!(clean_sign(SIGN_DEFAULT_EN).unwrap().is_some());
    }

    #[test]
    fn английские_примеры_и_деньги_в_валюте_установки() {
        let spec = LetterId::TopupDone.spec();
        let ru = samples(spec, Locale::Ru, Currency::Rub, "STEALTHNET");
        let en = samples(spec, Locale::En, Currency::Usd, "STEALTHNET");
        assert_eq!(
            fill_plain(spec.ru.text[1], &ru).unwrap(),
            format!(
                "Баланс пополнен на {}.",
                format::money_in(
                    Money {
                        minor: 1000,
                        currency: Currency::Rub
                    },
                    Locale::Ru
                )
            )
        );
        assert_eq!(
            fill_plain(spec.en.text[1], &en).unwrap(),
            "Your balance is topped up by $10.00."
        );
        assert_eq!(
            fill_plain(spec.en.text[0], &en).unwrap(),
            "$11.00 is on your balance: $10.00 and a $1.00 bonus."
        );
        assert_eq!(
            fill_plain(spec.ru.text[0], &Values::new().with("amount", "10")),
            None
        );
        assert_eq!(
            fill_plain(
                LetterId::SubscriptionReady.spec().en.text[0],
                &samples(
                    LetterId::SubscriptionReady.spec(),
                    Locale::En,
                    Currency::Usd,
                    "S"
                )
            )
            .unwrap(),
            "Your subscription is ready and runs until 14 February 2027. The connection link is in your account."
        );
    }
}
