use crate::BotError;
use crate::handler::{Screen, Turn, id_arg, live, main_menu, subscription_names};
use crate::session::{Awaiting, WithdrawalDraft};
use crate::telegram::{Button, Keyboard};
use crate::text;
use crate::ui::{self, Html, Lines, Text};
use sn_domain::access::ClientChannel;
use sn_domain::bot_content::Section;
use sn_domain::client::Client;
use sn_domain::ids::SubscriptionId;
use sn_domain::lang;
use sn_domain::module::ModuleId;
use sn_domain::payment::providers;
use sn_domain::pricing::QuoteLineKind;
use sn_domain::tariff::TrafficLeftover;
use sn_domain::traffic::TrafficReset;
use sn_modules::ModuleError;
use sn_modules::catalog::contests;
use sn_modules::catalog::options::{
    self, AddonsOffer, DevicesOffer, DevicesRequest, ITEM_DEVICES, ITEM_TRAFFIC, OptionsError,
    TrafficOffer, TrafficRequest,
};
use sn_modules::catalog::promo::{self, PromoError, PromoRejection, Purpose};
use sn_modules::catalog::referral::{
    self, PayoutOption, ReferralError, ReferralRejection, WithdrawalRequest, WithdrawalStatus,
};
use sn_payments::Unavailable;
use sn_services::ServiceError;
use sn_services::checkout::{Channel, CheckoutResult, ItemCheckoutRequest, ItemRequest};

fn module_error(err: ModuleError) -> String {
    text::user_error(&ServiceError::Module(err))
}

pub async fn entry(turn: &mut Turn<'_>, client: &Client, key: &str) -> Result<Screen, BotError> {
    let modules = turn.services().modules().clone();
    match key {
        promo::BOT_ENTRY if modules.is_enabled(ModuleId::Promo) => Ok(promo_prompt(turn)),
        referral::BOT_ENTRY if modules.is_enabled(ModuleId::Referral) => {
            referral_screen(turn, client).await
        }
        options::BOT_ENTRY if modules.is_enabled(ModuleId::Options) => {
            options_start(turn, client).await
        }
        sn_modules::catalog::gifts::BOT_ENTRY if modules.is_enabled(ModuleId::Gifts) => {
            crate::gifts::home(turn, client).await
        }
        contests::BOT_ENTRY if modules.is_enabled(ModuleId::Contests) => {
            crate::contests::entry(turn, client).await
        }
        _ => main_menu(turn.ctx, client).await,
    }
}

fn promo_prompt(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::PromoCode);
    let content = turn.content();
    Screen::new(ui::text(&content, "promo.prompt", &[]))
        .keyboard(Keyboard::new().push(ui::cancel(&content, "m")))
        .section(Section::Promo)
}

pub async fn promo_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let ctx = services.module_context();
    let checked = promo::check(
        &ctx,
        client.id,
        raw,
        Purpose::Check {
            tariff: None,
            currency: services.settings().currency,
        },
    )
    .await;
    let code = match checked {
        Ok(code) => code,
        Err(PromoError::Rejected(rejection)) => {
            return Ok(Screen::new(rejection.message())
                .keyboard(
                    Keyboard::new()
                        .push(ui::button(
                            c,
                            "buy.btn.promo_other",
                            &[],
                            format!("x:{}", promo::BOT_ENTRY),
                        ))
                        .push(ui::menu(c)),
                )
                .section(Section::Promo));
        }
        Err(PromoError::Module(err)) => return Ok(Screen::notice(module_error(err))),
    };
    if code.is_discount() {
        let label = code.shown_label();
        turn.session.promo = Some(code.code);
        return Ok(
            Screen::new(ui::text(c, "promo.saved", &[("code", Text(&label))]))
                .keyboard(
                    Keyboard::new()
                        .push(ui::button(c, "promo.btn.choose_tariff", &[], "buy"))
                        .push(ui::menu(c)),
                )
                .section(Section::Promo),
        );
    }
    let subscriptions: Vec<_> = live(services.subscriptions().list_for_client(client.id).await?)
        .into_iter()
        .filter(|s| s.remna.is_some())
        .collect();
    match subscriptions.as_slice() {
        [] => Ok(Screen::notice(PromoRejection::NoSubscription.message()).section(Section::Promo)),
        [only] => {
            turn.session.days_code = Some(code.code);
            activate_days(turn, client, only.id.0).await
        }
        many => {
            turn.session.days_code = Some(code.code);
            let names = subscription_names(&services, many).await;
            let mut keyboard = Keyboard::new();
            for (sub, name) in many.iter().zip(names) {
                keyboard = keyboard.push(ui::button(
                    c,
                    "sub.btn.item_open",
                    &[("tariff", Text(&name))],
                    format!("pc:{}", sub.id),
                ));
            }
            Ok(Screen::new(ui::text(c, "promo.which_sub", &[]))
                .keyboard(keyboard.push(ui::cancel(c, "m")))
                .section(Section::Promo))
        }
    }
}

pub async fn activate_days(
    turn: &mut Turn<'_>,
    client: &Client,
    subscription_id: i64,
) -> Result<Screen, BotError> {
    let Some(code) = turn.session.days_code.take() else {
        return Ok(promo_prompt(turn));
    };
    let content = turn.content();
    let c = &*content;
    let ctx = turn.services().module_context();
    match promo::activate(&ctx, client.id, &code, SubscriptionId(subscription_id)).await {
        Ok(done) => Ok(Screen::new(ui::text(
            c,
            "promo.days_added",
            &[("days", Text(&text::days(done.days)))],
        ))
        .keyboard(
            Keyboard::new()
                .push(ui::button(
                    c,
                    "common.btn.subscription",
                    &[],
                    format!("sub:{subscription_id}"),
                ))
                .push(ui::menu(c)),
        )
        .section(Section::Promo)),
        Err(PromoError::Rejected(rejection)) => {
            Ok(Screen::notice(rejection.message()).section(Section::Promo))
        }
        Err(PromoError::Module(err)) => Ok(Screen::notice(module_error(err))),
    }
}

fn back_to_referral(turn: &Turn<'_>, key: &str) -> Keyboard {
    let content = turn.content();
    Keyboard::new().push(ui::button(
        &content,
        key,
        &[],
        format!("x:{}", referral::BOT_ENTRY),
    ))
}

async fn referral_screen(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let content = turn.content();
    let c = &*content;
    let ctx = turn.services().module_context();
    let overview = referral::overview(&ctx, client.id).await?;
    let link = overview.bot_link.clone().or_else(|| {
        turn.ctx
            .username
            .as_deref()
            .map(|username| referral::invite_link(username, &overview.code))
    });
    let mut lines = Lines::new();
    lines.add(ui::text(c, "ref.title", &[]));
    lines.add(ui::text(
        c,
        "ref.percent",
        &[("percent", Text(&overview.levels[0].to_string()))],
    ));
    if let Some(link) = &link {
        lines.add(ui::text(c, "ref.link", &[("link", Text(link))]));
    }
    lines.add(ui::text(
        c,
        "ref.invited",
        &[("count", Text(&overview.invited.to_string()))],
    ));
    lines.add(ui::text(
        c,
        "ref.earned",
        &[("amount", Text(&text::money(overview.earned)))],
    ));
    lines.add(ui::text(
        c,
        "ref.balance",
        &[("amount", Text(&text::money(overview.balance)))],
    ));
    let mut keyboard = Keyboard::new();
    let pending: Vec<_> = overview
        .withdrawals
        .iter()
        .filter(|w| {
            matches!(
                w.status,
                WithdrawalStatus::Pending | WithdrawalStatus::Holding
            )
        })
        .collect();
    if !pending.is_empty() {
        lines.blank();
        for withdrawal in &pending {
            let id = withdrawal.id.to_string();
            let status = withdrawal.status.title().to_lowercase();
            lines.add(ui::text(
                c,
                "ref.withdrawal",
                &[
                    ("id", Text(&id)),
                    ("amount", Text(&text::money(withdrawal.amount))),
                    ("status", Text(&status)),
                ],
            ));
            if withdrawal.status == WithdrawalStatus::Pending {
                keyboard = keyboard.push(ui::button(
                    c,
                    "ref.btn.cancel_withdrawal",
                    &[("id", Text(&id))],
                    format!("ref:c:{}", withdrawal.id),
                ));
            }
        }
    }
    if overview.withdrawals_enabled && overview.balance.minor > 0 {
        keyboard = keyboard.push(ui::button(c, "ref.btn.withdraw", &[], "ref:w"));
    }
    if let Some(link) = link {
        keyboard = keyboard.push(ui::link(
            c,
            "ref.btn.share",
            &[],
            format!("https://t.me/share/url?url={}", text::url_encode(&link)),
        ));
    }
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Referral))
}

pub async fn referral_action(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Referral) {
        return main_menu(turn.ctx, client).await;
    }
    let content = turn.content();
    let c = &*content;
    let (action, arg) = rest.split_once(':').unwrap_or((rest, ""));
    match action {
        "w" => {
            let ctx = turn.services().module_context();
            let methods = referral::payout_options(&ctx).await?;
            if methods.is_empty() {
                return referral_screen(turn, client).await;
            }
            let mut keyboard = Keyboard::new();
            for method in &methods {
                keyboard = keyboard.row(vec![Button::callback(
                    method.title.as_str(),
                    format!("ref:m:{}", method.id),
                )]);
            }
            Ok(Screen::new(ui::text(c, "ref.how", &[]))
                .keyboard(keyboard.push(ui::cancel(c, format!("x:{}", referral::BOT_ENTRY))))
                .section(Section::Referral))
        }
        "m" => {
            let ctx = turn.services().module_context();
            let Some(method) = payout_option(&ctx, arg).await? else {
                return referral_screen(turn, client).await;
            };
            turn.session.withdrawal = Some(WithdrawalDraft {
                method: Some(method.id.clone()),
                amount: None,
            });
            turn.session.wait_for(Awaiting::WithdrawalAmount);
            let overview = referral::overview(&ctx, client.id).await?;
            let minimum = method
                .min
                .map(|m| ui::text(c, "ref.minimum", &[("amount", Text(&text::money(m)))]))
                .unwrap_or_default();
            Ok(Screen::new(ui::text(
                c,
                "ref.amount",
                &[
                    ("amount", Text(&text::money(overview.balance))),
                    ("minimum", Html(&minimum)),
                ],
            ))
            .keyboard(Keyboard::new().push(ui::cancel(c, format!("x:{}", referral::BOT_ENTRY))))
            .section(Section::Referral))
        }
        "c" => {
            let Some(id) = id_arg(arg) else {
                return referral_screen(turn, client).await;
            };
            let ctx = turn.services().module_context();
            match referral::cancel_withdrawal(&ctx, client.id, id).await {
                Ok(withdrawal) => Ok(Screen::new(ui::text(
                    c,
                    "ref.cancelled",
                    &[
                        ("id", Text(&withdrawal.id.to_string())),
                        ("amount", Text(&text::money(withdrawal.amount))),
                    ],
                ))
                .keyboard(back_to_referral(turn, "common.btn.back"))
                .section(Section::Referral)),
                Err(err) => Ok(Screen::notice(referral_error(err))),
            }
        }
        _ => referral_screen(turn, client).await,
    }
}

fn referral_error(err: ReferralError) -> String {
    match err {
        ReferralError::Rejected(rejection) => rejection.message(),
        ReferralError::Module(err) => module_error(err),
    }
}

async fn payout_option(
    ctx: &sn_modules::ModuleContext,
    id: &str,
) -> Result<Option<PayoutOption>, BotError> {
    Ok(referral::payout_options(ctx)
        .await?
        .into_iter()
        .find(|m| m.id == id))
}

fn withdrawal_retry(turn: &Turn<'_>, text: String) -> Screen {
    let content = turn.services().bot_content();
    let c = &*content;
    Screen::new(text)
        .keyboard(
            Keyboard::new()
                .push(ui::button(c, "ref.btn.retry", &[], "ref:w"))
                .push(ui::menu(c)),
        )
        .section(Section::Referral)
}

pub async fn withdrawal_amount(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let Some(id) = turn
        .session
        .withdrawal
        .as_ref()
        .and_then(|w| w.method.clone())
    else {
        return referral_screen(turn, client).await;
    };
    let ctx = turn.services().module_context();
    let Some(method) = payout_option(&ctx, &id).await? else {
        turn.session.withdrawal = None;
        return Ok(withdrawal_retry(
            turn,
            ReferralRejection::MethodUnavailable.message(),
        ));
    };
    let amount = raw
        .trim()
        .trim_start_matches('$')
        .trim_end_matches(['$', '₽'])
        .replace(',', ".")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}')
        .collect::<String>();
    turn.session.withdrawal = Some(WithdrawalDraft {
        method: Some(method.id),
        amount: Some(amount),
    });
    turn.session.wait_for(Awaiting::WithdrawalDetails);
    let content = turn.content();
    Ok(Screen::new(ui::text(
        &content,
        "ref.details",
        &[("hint", Text(&method.hint))],
    ))
    .keyboard(back_to_referral(turn, "common.btn.cancel"))
    .section(Section::Referral))
}

pub async fn withdrawal_details(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let Some(WithdrawalDraft {
        method: Some(method),
        amount: Some(amount),
    }) = turn.session.withdrawal.take()
    else {
        return referral_screen(turn, client).await;
    };
    let content = turn.content();
    let c = &*content;
    let ctx = turn.services().module_context();
    match referral::request_withdrawal(
        &ctx,
        client.id,
        &WithdrawalRequest {
            amount,
            method,
            details: raw.to_string(),
        },
    )
    .await
    {
        Ok(withdrawal) => Ok(Screen::new(ui::text(
            c,
            "ref.requested",
            &[
                ("id", Text(&withdrawal.id.to_string())),
                ("amount", Text(&text::money(withdrawal.amount))),
            ],
        ))
        .keyboard(back_to_referral(turn, "common.btn.back"))
        .section(Section::Referral)),
        Err(err) => Ok(withdrawal_retry(turn, referral_error(err))),
    }
}

async fn options_start(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let subscriptions: Vec<_> = live(services.subscriptions().list_for_client(client.id).await?)
        .into_iter()
        .filter(|s| s.remna.is_some())
        .collect();
    match subscriptions.as_slice() {
        [] => Ok(Screen::notice(ui::text(c, "opt.no_subscription", &[])).section(Section::Options)),
        [only] => options_menu(turn, client, only.id).await,
        many => {
            let names = subscription_names(&services, many).await;
            let mut keyboard = Keyboard::new();
            for (sub, name) in many.iter().zip(names) {
                keyboard = keyboard.push(ui::button(
                    c,
                    "sub.btn.item_open",
                    &[("tariff", Text(&name))],
                    format!("opt:{}", sub.id),
                ));
            }
            Ok(Screen::new(ui::text(c, "opt.which_sub", &[]))
                .keyboard(keyboard.push(ui::menu(c)))
                .section(Section::Options))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addon {
    Devices(u32),
    Traffic(u32),
}

impl Addon {
    fn parse(kind: &str, arg: &str) -> Option<Addon> {
        let value = arg.parse::<u32>().ok().filter(|v| *v > 0)?;
        match kind {
            "d" => Some(Addon::Devices(value)),
            "t" => Some(Addon::Traffic(value)),
            _ => None,
        }
    }

    fn code(self) -> String {
        match self {
            Addon::Devices(count) => format!("d:{count}"),
            Addon::Traffic(gb) => format!("t:{gb}"),
        }
    }

    fn item(self, subscription_id: SubscriptionId) -> ItemRequest {
        let (kind, payload) = match self {
            Addon::Devices(count) => (
                ITEM_DEVICES,
                DevicesRequest {
                    subscription_id,
                    count,
                }
                .payload(),
            ),
            Addon::Traffic(gb) => (
                ITEM_TRAFFIC,
                TrafficRequest {
                    subscription_id,
                    gb,
                }
                .payload(),
            ),
        };
        ItemRequest {
            module: ModuleId::Options,
            kind: kind.into(),
            payload,
            subscription_id: Some(subscription_id),
        }
    }

    fn what(self, c: &sn_services::bot_content::BotContent) -> String {
        match self {
            Addon::Devices(count) => ui::text(
                c,
                "pay.what_devices",
                &[("count", Text(&count.to_string()))],
            ),
            Addon::Traffic(gb) => ui::text(c, "pay.what_traffic", &[("gb", Text(&gb.to_string()))]),
        }
    }
}

async fn addons(
    turn: &Turn<'_>,
    client: &Client,
    subscription_id: SubscriptionId,
) -> Result<Result<AddonsOffer, String>, BotError> {
    let services = turn.services();
    let mut conn = services.store().conn().await?;
    let result = options::addons_offer(
        &mut conn,
        client.id,
        subscription_id,
        services.settings().currency,
        services.now(),
    )
    .await;
    Ok(match result {
        Ok(view) => Ok(view),
        Err(OptionsError::Rejected(rejection)) => Err(rejection.message()),
        Err(OptionsError::Module(err)) => Err(module_error(err)),
    })
}

async fn options_menu(
    turn: &mut Turn<'_>,
    client: &Client,
    subscription_id: SubscriptionId,
) -> Result<Screen, BotError> {
    let view = match addons(turn, client, subscription_id).await? {
        Ok(view) => view,
        Err(message) => return Ok(Screen::notice(message).section(Section::Options)),
    };
    let content = turn.content();
    let c = &*content;
    match (view.devices.offer, view.traffic.offer) {
        (Some(devices), None) => Ok(devices_screen(c, &devices, false)),
        (None, Some(traffic)) => Ok(traffic_screen(turn, &traffic, false)),
        (Some(devices), Some(_)) => {
            let offset = turn.services().settings().remna_offset();
            let date = text::date(devices.expire_at, offset);
            let mut keyboard = Keyboard::new();
            for (key, code) in [("opt.btn.devices", "d"), ("opt.btn.traffic", "t")] {
                keyboard = keyboard.push(ui::button(
                    c,
                    key,
                    &[],
                    format!("opt:{subscription_id}:{code}"),
                ));
            }
            Ok(
                Screen::new(ui::text(c, "opt.menu", &[("date", Text(&date))]))
                    .keyboard(keyboard.push(ui::menu(c)))
                    .section(Section::Options),
            )
        }
        (None, None) => {
            let message = view
                .devices
                .unavailable
                .or(view.traffic.unavailable)
                .map(|u| u.message)
                .unwrap_or_else(|| ui::text(c, "opt.no_subscription", &[]));
            Ok(Screen::notice(message).section(Section::Options))
        }
    }
}

fn back_or_menu(c: &sn_services::bot_content::BotContent, back: Option<String>) -> Keyboard {
    match back {
        Some(target) => Keyboard::new().pair(ui::back(c, target), ui::menu(c)),
        None => Keyboard::new().push(ui::menu(c)),
    }
}

fn devices_screen(
    c: &sn_services::bot_content::BotContent,
    offer: &DevicesOffer,
    both: bool,
) -> Screen {
    let sub = offer.subscription_id;
    let back = both.then(|| format!("opt:{sub}"));
    if offer.can_add == 0 {
        return Screen::new(ui::text(c, "opt.max", &[]))
            .keyboard(back_or_menu(c, back))
            .section(Section::Options);
    }
    let stars = offer
        .per_device_stars
        .map(|s| ui::text(c, "opt.stars", &[("stars", Text(&text::stars(s)))]))
        .unwrap_or_default();
    let mut message = ui::text(
        c,
        "opt.offer",
        &[
            ("included", Text(&offer.included.to_string())),
            ("extra", Text(&offer.extra_now.to_string())),
            ("can_add", Text(&offer.can_add.to_string())),
            ("days", Text(&text::days(offer.days))),
            ("price", Text(&text::money(offer.per_device))),
            ("stars", Html(&stars)),
        ],
    );
    if !offer.discounts.is_empty() {
        message.push('\n');
        message.push_str(&ui::text(
            c,
            "opt.discounts",
            &[("steps", Text(&crate::buy::discount_steps(&offer.discounts)))],
        ));
    }
    let buttons: Vec<_> = (1..=offer.can_add)
        .filter_map(|n| {
            ui::button(
                c,
                "opt.btn.count",
                &[("count", Text(&n.to_string()))],
                format!("opt:{sub}:d:{n}"),
            )
        })
        .collect();
    let keyboard = Keyboard::new().grid(buttons, 4);
    let keyboard = match back {
        Some(target) => keyboard.pair(ui::back(c, target), ui::menu(c)),
        None => keyboard.push(ui::menu(c)),
    };
    Screen::new(message)
        .keyboard(keyboard)
        .section(Section::Options)
}

fn leftover_key(reset: TrafficReset, leftover: TrafficLeftover) -> &'static str {
    match (leftover, reset.periodic()) {
        (TrafficLeftover::Burn, true) => "opt.traffic_burn_reset",
        (TrafficLeftover::Burn, false) => "opt.traffic_burn_payment",
        (TrafficLeftover::Carry, true) => "opt.traffic_carry_reset",
        (TrafficLeftover::Carry, false) => "opt.traffic_carry_payment",
    }
}

fn traffic_screen(turn: &Turn<'_>, offer: &TrafficOffer, both: bool) -> Screen {
    let content = turn.content();
    let c = &*content;
    let sub = offer.subscription_id;
    let note = ui::text(c, leftover_key(offer.reset, offer.leftover), &[]);
    let message = ui::text(
        c,
        "opt.traffic",
        &[
            ("used", Text(&text::traffic(offer.used_bytes.unwrap_or(0)))),
            (
                "limit",
                Text(&text::traffic(offer.limit_bytes.unwrap_or(0))),
            ),
            ("note", Html(&note)),
        ],
    );
    let mut keyboard = Keyboard::new();
    for pack in &offer.packs {
        keyboard = keyboard.push(ui::button(
            c,
            "opt.btn.pack",
            &[
                ("gb", Text(&pack.gb.to_string())),
                ("price", Text(&text::money(pack.price))),
            ],
            format!("opt:{sub}:t:{}", pack.gb),
        ));
    }
    let keyboard = if both {
        keyboard.pair(ui::back(c, format!("opt:{sub}")), ui::menu(c))
    } else {
        keyboard.push(ui::menu(c))
    };
    Screen::new(message)
        .keyboard(keyboard)
        .section(Section::Options)
}

async fn addon_summary(
    turn: &mut Turn<'_>,
    client: &Client,
    subscription_id: SubscriptionId,
    addon: Addon,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let view = match services
        .checkout()
        .quote_item_in(
            client.id,
            &addon.item(subscription_id),
            Some(ClientChannel::Bot),
        )
        .await
    {
        Ok(view) => view,
        Err(err) => return Ok(Screen::notice(text::user_error(&err)).section(Section::Options)),
    };
    let quote = &view.quote;
    let stars = quote
        .stars
        .map(|s| ui::text(c, "opt.stars", &[("stars", Text(&text::stars(s.total)))]))
        .unwrap_or_default();
    let label = quote
        .lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            *index == 0 || matches!(line.kind, QuoteLineKind::DeviceDiscount { .. })
        })
        .map(|(_, line)| line.shown_label())
        .collect::<Vec<_>>()
        .join(" · ");
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "opt.quote",
        &[
            ("label", Text(&label)),
            ("total", Text(&text::money(quote.total))),
            ("stars", Html(&stars)),
        ],
    ));
    let code = addon.code();
    let mut keyboard = Keyboard::new();
    for method in &view.methods {
        match (&method.unavailable, method.amount) {
            (None, Some(amount)) => {
                keyboard = keyboard.push(ui::button(
                    c,
                    "buy.btn.method",
                    &[
                        ("method", Text(&method.title)),
                        ("amount", Text(&text::amount(amount))),
                    ],
                    format!(
                        "op:{subscription_id}:{code}:{}",
                        crate::buy::pay_target(method.provider, method.method.as_deref())
                    ),
                ));
            }
            (Some(Unavailable::InsufficientBalance { balance, missing }), _) => {
                lines.add(ui::text(
                    c,
                    "buy.balance_short",
                    &[
                        ("missing", Text(&text::money(*missing))),
                        ("balance", Text(&text::money(*balance))),
                    ],
                ));
            }
            _ => {}
        }
    }
    if view.methods.iter().all(|m| m.unavailable.is_some()) {
        lines.add(ui::text(c, "buy.no_methods", &[]));
    }
    let back = match addon {
        Addon::Devices(_) => format!("opt:{subscription_id}:d"),
        Addon::Traffic(_) => format!("opt:{subscription_id}:t"),
    };
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.pair(ui::back(c, back), ui::menu(c)))
        .section(Section::Options))
}

pub async fn options_action(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Options) {
        return main_menu(turn.ctx, client).await;
    }
    let mut parts = rest.splitn(3, ':');
    let Some(sub) = parts.next().and_then(id_arg).map(SubscriptionId) else {
        return options_start(turn, client).await;
    };
    let kind = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("");
    if let Some(addon) = Addon::parse(kind, arg) {
        return addon_summary(turn, client, sub, addon).await;
    }
    if kind.is_empty() {
        return options_menu(turn, client, sub).await;
    }
    let view = match addons(turn, client, sub).await? {
        Ok(view) => view,
        Err(message) => return Ok(Screen::notice(message).section(Section::Options)),
    };
    let both = view.devices.offer.is_some() && view.traffic.offer.is_some();
    let content = turn.content();
    match kind {
        "d" => Ok(match &view.devices.offer {
            Some(offer) => devices_screen(&content, offer, both),
            None => unavailable_screen(view.devices.unavailable.map(|u| u.message)),
        }),
        "t" => Ok(match &view.traffic.offer {
            Some(offer) => traffic_screen(turn, offer, both),
            None => unavailable_screen(view.traffic.unavailable.map(|u| u.message)),
        }),
        _ => options_menu(turn, client, sub).await,
    }
}

fn unavailable_screen(message: Option<String>) -> Screen {
    Screen::notice(message.unwrap_or_default()).section(Section::Options)
}

pub async fn options_pay(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    let mut parts = rest.splitn(4, ':');
    let sub = parts.next().and_then(id_arg).map(SubscriptionId);
    let addon = match (parts.next(), parts.next()) {
        (Some(kind), Some(arg)) => Addon::parse(kind, arg),
        _ => None,
    };
    let target = parts.next().unwrap_or("");
    let (provider, method) = crate::buy::split_target(target);
    let (Some(sub), Some(addon)) = (sub, addon) else {
        return options_start(turn, client).await;
    };
    if provider.is_empty() {
        return addon_summary(turn, client, sub, addon).await;
    }
    let back = crate::buy::back_from(turn, target);
    if let Some(screen) = crate::buy::receipt_gate(turn, client, provider, method, &back).await? {
        return Ok(screen);
    }
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let result = services
        .checkout()
        .start_item(
            client.id,
            &ItemCheckoutRequest {
                item: addon.item(sub),
                provider: provider.to_string(),
                method: method.map(str::to_string),
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await;
    let what = addon.what(&content);
    match result {
        Ok(result) => {
            if let CheckoutResult::Redirect { payment_id, .. } = &result
                && provider == providers::GRAM
                && crate::buy::send_gram_widget(turn, *payment_id, &what).await?
            {
                return Ok(Screen::new(ui::text(
                    &content,
                    "pay.gram_ready",
                    &[("what", Html(&what))],
                ))
                .keyboard(Keyboard::new().push(ui::menu(&content)))
                .section(Section::Payment));
            }
            Ok(crate::account::payment_screen(&content, &result, &what))
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}
