use crate::BotError;
use crate::account::payment_screen;
use crate::gram_wallet;
use crate::handler::{Screen, Turn, id_arg, live, subscription_names, tariff_name};
use crate::session::{Awaiting, Cart};
use crate::telegram::Keyboard;
use crate::text;
use crate::ui::{self, Html, Lines, Text};
use sn_domain::bot_content::Section;
use sn_domain::client::Client;
use sn_domain::client::mask_email;
use sn_domain::ids::{PeriodId, SubscriptionId, TariffId};
use sn_domain::lang;
use sn_domain::module::ModuleId;
use sn_domain::money::Money;
use sn_domain::payment::providers;
use sn_domain::plan::PlanKind;
use sn_domain::tariff::{DeviceDiscount, ExtraDevices, PeriodOffer, Tariff};
use sn_modules::catalog::promo::{self, PromoError, Purpose};
use sn_payments::Unavailable;
use sn_services::bot_content::BotContent;
use sn_services::checkout::{
    CartRequest, CategoryFull, Channel, CheckoutRequest, CheckoutResult, TariffOffer,
};
use sn_services::clients::RECEIPT_EMAIL_INVALID;
use sn_services::{Actor, ServiceError, Services};
use sn_store::{Db, StoreError};
use uuid::Uuid;

fn back_target(cart: &Cart) -> String {
    match cart.subscription_id {
        Some(id) => format!("buy:{id}"),
        None => "buy:new".into(),
    }
}

pub async fn start(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let multi = services.settings().multi_subscriptions;
    let subscriptions = live(services.subscriptions().list_for_client(client.id).await?);
    let target = if rest == "new" && (multi || subscriptions.is_empty()) {
        None
    } else if let Some(id) = id_arg(rest) {
        subscriptions.iter().find(|s| s.id.0 == id).map(|s| s.id)
    } else {
        match subscriptions.as_slice() {
            [] => None,
            [only] if !multi => Some(only.id),
            many => {
                let content = services.bot_content_in(lang::current());
                let c = &*content;
                let offset = services.settings().remna_offset();
                let names = subscription_names(&services, many).await;
                let mut keyboard = Keyboard::new();
                for (sub, name) in many.iter().zip(names) {
                    let data = format!("buy:{}", sub.id);
                    keyboard = keyboard.push(match sub.mirror.expire_at {
                        Some(at) => ui::button(
                            c,
                            "buy.btn.extend_item",
                            &[
                                ("tariff", Text(&name)),
                                ("date", Text(&text::day_month(at, offset))),
                            ],
                            data,
                        ),
                        None => ui::button(
                            c,
                            "buy.btn.extend_item_open",
                            &[("tariff", Text(&name))],
                            data,
                        ),
                    });
                }
                if multi {
                    keyboard = keyboard.push(ui::button(c, "buy.btn.new", &[], "buy:new"));
                }
                return Ok(Screen::new(ui::text(c, "buy.which", &[]))
                    .keyboard(keyboard.push(ui::menu(c)))
                    .section(Section::Buy));
            }
        }
    };
    turn.session.cart = Some(Cart {
        subscription_id: target,
        ..Cart::default()
    });
    catalog(turn).await
}

async fn offers(turn: &Turn<'_>) -> Result<Vec<TariffOffer>, BotError> {
    Ok(turn
        .services()
        .checkout()
        .catalog_in(Some(sn_domain::access::ClientChannel::Bot))
        .await?)
}

async fn catalog(turn: &mut Turn<'_>) -> Result<Screen, BotError> {
    let content = turn.content();
    let c = &*content;
    let offers = offers(turn).await?;
    if offers.is_empty() {
        return Ok(Screen::notice(ui::text(c, "buy.empty", &[])).section(Section::Buy));
    }
    let mut lines = Lines::new();
    lines.add(ui::text(c, "buy.title", &[]));
    let mut keyboard = Keyboard::new();
    let mut category = None;
    for offer in &offers {
        if category != Some(offer.category.id) {
            category = Some(offer.category.id);
            lines.add(ui::text(
                c,
                "buy.category",
                &[("category", Text(&offer.category.name))],
            ));
        }
        let cheapest = offer.periods.iter().min_by_key(|p| p.per_month.minor);
        lines.add(match cheapest {
            Some(period) => match &period.promotion {
                Some(promotion) => ui::text(
                    c,
                    "buy.tariff_line_promo",
                    &[
                        ("tariff", Text(&offer.tariff.name)),
                        ("per_month", Text(&text::money(period.per_month))),
                        (
                            "full_per_month",
                            Text(&text::money(
                                sn_domain::tariff::month_price(
                                    promotion.full_price,
                                    period.days,
                                    period.months,
                                )
                                .unwrap_or(promotion.full_price),
                            )),
                        ),
                        ("percent", Text(&promotion.percent.to_string())),
                        ("promotion", Text(&promotion.name)),
                        ("ends", Text(&promotion.until)),
                    ],
                ),
                None => ui::text(
                    c,
                    "buy.tariff_line",
                    &[
                        ("tariff", Text(&offer.tariff.name)),
                        ("per_month", Text(&text::money(period.per_month))),
                    ],
                ),
            },
            None => ui::text(
                c,
                "buy.tariff_line_plain",
                &[("tariff", Text(&offer.tariff.name))],
            ),
        });
        keyboard = keyboard.push(ui::button(
            c,
            "buy.btn.tariff",
            &[("tariff", Text(&offer.tariff.name))],
            format!("t:{}", offer.tariff.id),
        ));
    }
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Buy))
}

pub fn discount_steps(discounts: &[DeviceDiscount]) -> String {
    discounts
        .iter()
        .enumerate()
        .map(|(index, step)| match (lang::current(), index) {
            (sn_domain::client::Locale::Ru, 0) => {
                format!("от {} — {}%", text::devices_from(step.from), step.percent)
            }
            (sn_domain::client::Locale::Ru, _) => format!("от {} — {}%", step.from, step.percent),
            (sn_domain::client::Locale::En, 0) => {
                format!("{}+ extra devices — {}%", step.from, step.percent)
            }
            (sn_domain::client::Locale::En, _) => format!("{}+ — {}%", step.from, step.percent),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

async fn category_choice(
    services: &Services,
    c: &BotContent,
    full: &CategoryFull,
    tariff_id: TariffId,
) -> Screen {
    let offset = services.settings().remna_offset();
    let names = subscription_names(services, &full.held).await;
    let mut keyboard = Keyboard::new();
    for (sub, name) in full.held.iter().zip(names) {
        let data = format!("tb:{tariff_id}:{}", sub.id);
        keyboard = keyboard.push(match sub.mirror.expire_at {
            Some(at) => ui::button(
                c,
                "buy.btn.extend_item",
                &[
                    ("tariff", Text(&name)),
                    ("date", Text(&text::day_month(at, offset))),
                ],
                data,
            ),
            None => ui::button(
                c,
                "buy.btn.extend_item_open",
                &[("tariff", Text(&name))],
                data,
            ),
        });
    }
    Screen::new(ui::text(
        c,
        "buy.category_choose",
        &[("category", Text(&full.category.name))],
    ))
    .keyboard(keyboard.pair(ui::back(c, "buy:new"), ui::menu(c)))
    .section(Section::Buy)
}

pub async fn tariff_for(
    turn: &mut Turn<'_>,
    client: &Client,
    tariff_id: TariffId,
    subscription_id: SubscriptionId,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let owned = live(services.subscriptions().list_for_client(client.id).await?)
        .iter()
        .any(|s| s.id == subscription_id);
    if !owned {
        return start(turn, client, "").await;
    }
    let cart = turn.session.cart.get_or_insert_with(Cart::default);
    cart.subscription_id = Some(subscription_id);
    tariff(turn, client, tariff_id).await
}

pub async fn tariff(
    turn: &mut Turn<'_>,
    client: &Client,
    tariff_id: TariffId,
) -> Result<Screen, BotError> {
    let content = turn.content();
    let c = &*content;
    let offers = offers(turn).await?;
    let Some(offer) = offers.into_iter().find(|o| o.tariff.id == tariff_id) else {
        return start(turn, client, "").await;
    };
    let services = turn.services().clone();
    let mut limit_note = None;
    if let Some(full) = services
        .checkout()
        .category_room(client.id, tariff_id)
        .await?
    {
        let target = turn.session.cart.as_ref().and_then(|c| c.subscription_id);
        if !target.is_some_and(|id| full.holds(id)) {
            let [only] = full.held.as_slice() else {
                return Ok(category_choice(&services, c, &full, tariff_id).await);
            };
            let cart = turn.session.cart.get_or_insert_with(Cart::default);
            cart.subscription_id = Some(only.id);
            limit_note = Some(ui::text(
                c,
                "buy.category_renew",
                &[("category", Text(&full.category.name))],
            ));
        }
    }
    let cart = turn.session.cart.get_or_insert_with(Cart::default);
    cart.tariff_id = Some(tariff_id);
    cart.period_id = None;
    cart.extra_devices = 0;
    let back = back_target(cart);
    let tariff = &offer.tariff;
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "buy.tariff_title",
        &[("tariff", Text(&tariff.name))],
    ));
    if let Some(description) = tariff
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        lines.add(text::escape(description));
    }
    if let Some(note) = limit_note {
        lines.add(note);
    }
    lines.add(ui::text(
        c,
        "buy.tariff_devices",
        &[("count", Text(&tariff.devices.included.to_string()))],
    ));
    match tariff.traffic.limit_bytes {
        Some(limit) if limit > 0 => lines.add(ui::text(
            c,
            "buy.tariff_traffic",
            &[
                ("limit", Text(&text::traffic(limit))),
                ("reset", Text(tariff.traffic.reset.short())),
            ],
        )),
        _ => lines.add(ui::text(c, "buy.tariff_unlimited", &[])),
    }
    promotion_lines(c, &mut lines, &offer.periods);
    lines.add(ui::text(c, "buy.choose_period", &[]));
    let mut keyboard = Keyboard::new();
    for period in &offer.periods {
        let days = text::days(period.days);
        let price = text::money(period.price);
        let per_month = text::money(period.per_month);
        let data = format!("p:{}", period.period_id);
        if let Some(promotion) = &period.promotion {
            keyboard = keyboard.push(ui::button(
                c,
                "buy.btn.period_promo",
                &[
                    ("days", Text(&days)),
                    ("price", Text(&price)),
                    ("percent", Text(&promotion.percent.to_string())),
                ],
                data,
            ));
            continue;
        }
        keyboard = keyboard.push(match period.savings_percent {
            Some(savings) => ui::button(
                c,
                "buy.btn.period_savings",
                &[
                    ("days", Text(&days)),
                    ("price", Text(&price)),
                    ("per_month", Text(&per_month)),
                    ("savings", Text(&savings.to_string())),
                ],
                data,
            ),
            None if period.months > 1 => ui::button(
                c,
                "buy.btn.period_monthly",
                &[
                    ("days", Text(&days)),
                    ("price", Text(&price)),
                    ("per_month", Text(&per_month)),
                ],
                data,
            ),
            None => ui::button(
                c,
                "buy.btn.period",
                &[("days", Text(&days)), ("price", Text(&price))],
                data,
            ),
        });
    }
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.pair(ui::back(c, back), ui::menu(c)))
        .section(Section::Buy))
}

pub(crate) fn promotion_lines(c: &BotContent, lines: &mut Lines, periods: &[PeriodOffer]) {
    let Some(promotion) = periods.iter().find_map(|p| p.promotion.as_ref()) else {
        return;
    };
    lines.add(ui::text(
        c,
        "buy.promotion",
        &[
            ("promotion", Text(&promotion.name)),
            ("ends", Text(&promotion.until)),
            ("percent", Text(&promotion.percent.to_string())),
        ],
    ));
    for period in periods {
        let Some(promotion) = &period.promotion else {
            continue;
        };
        lines.add(ui::text(
            c,
            "buy.promotion_period",
            &[
                ("days", Text(&text::days(period.days))),
                ("price", Text(&text::money(period.price))),
                ("full_price", Text(&text::money(promotion.full_price))),
            ],
        ));
    }
}

fn sellable(services: &Services, tariff: &Tariff) -> Option<(ExtraDevices, Money)> {
    let extra = tariff
        .devices
        .extra
        .clone()
        .filter(|e| e.max > 0 && services.modules().is_enabled(ModuleId::Options))?;
    let monthly = extra.monthly.get(services.settings().currency)?;
    Some((extra, monthly))
}

async fn kept_devices(
    services: &Services,
    client: &Client,
    cart: &Cart,
    tariff: &Tariff,
    extra: &ExtraDevices,
) -> Result<u32, BotError> {
    let Some(subscription_id) = cart.subscription_id else {
        return Ok(0);
    };
    let subscription = {
        let mut conn = services.store().conn().await?;
        match conn.subscriptions().get(subscription_id).await {
            Ok(subscription) => subscription,
            Err(StoreError::NotFound(_)) => return Ok(0),
            Err(err) => return Err(err.into()),
        }
    };
    if subscription.client_id != client.id || subscription.tariff_id != Some(tariff.id) {
        return Ok(0);
    }
    Ok(subscription.extra_devices.min(extra.max))
}

fn devices_screen(
    c: &BotContent,
    tariff: &Tariff,
    extra: &ExtraDevices,
    monthly: Money,
    back: String,
) -> Screen {
    let buttons: Vec<_> = (1..=extra.max)
        .filter_map(|n| {
            ui::button(
                c,
                "buy.btn.extra",
                &[("count", Text(&n.to_string()))],
                format!("d:{n}"),
            )
        })
        .collect();
    let included_text = text::devices(tariff.devices.included);
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "buy.extra_devices",
        &[
            ("included", Text(&included_text)),
            ("price", Text(&text::money(monthly))),
        ],
    ));
    if !tariff.device_discounts.is_empty() {
        lines.add(ui::text(
            c,
            "buy.device_discounts",
            &[("steps", Text(&discount_steps(&tariff.device_discounts)))],
        ));
    }
    Screen::new(lines.text())
        .keyboard(
            Keyboard::new()
                .push(ui::button(c, "buy.btn.no_extra", &[], "d:0"))
                .grid(buttons, 4)
                .push(ui::back(c, back)),
        )
        .section(Section::Buy)
}

pub async fn period(
    turn: &mut Turn<'_>,
    client: &Client,
    period_id: i64,
) -> Result<Screen, BotError> {
    let Some(tariff_id) = turn.session.cart.as_ref().and_then(|c| c.tariff_id) else {
        return start(turn, client, "").await;
    };
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let tariff = {
        let mut conn = services.store().conn().await?;
        conn.tariffs().get(tariff_id).await?
    };
    if tariff.period(PeriodId(period_id)).is_none() {
        return self::tariff(turn, client, tariff_id).await;
    }
    if let Some(cart) = turn.session.cart.as_mut() {
        cart.period_id = Some(PeriodId(period_id));
        cart.extra_devices = 0;
    }
    let Some((extra, monthly)) = sellable(&services, &tariff) else {
        return summary(turn, client).await;
    };
    let kept = match turn.session.cart.as_ref() {
        Some(cart) => kept_devices(&services, client, cart, &tariff, &extra).await?,
        None => 0,
    };
    if kept > 0 {
        if let Some(cart) = turn.session.cart.as_mut() {
            cart.extra_devices = kept;
        }
        return summary(turn, client).await;
    }
    Ok(devices_screen(
        c,
        &tariff,
        &extra,
        monthly,
        format!("t:{tariff_id}"),
    ))
}

pub async fn change_devices(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let Some(tariff_id) = turn.session.cart.as_ref().and_then(|c| c.tariff_id) else {
        return start(turn, client, "").await;
    };
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let tariff = {
        let mut conn = services.store().conn().await?;
        conn.tariffs().get(tariff_id).await?
    };
    match sellable(&services, &tariff) {
        Some((extra, monthly)) => Ok(devices_screen(
            &content,
            &tariff,
            &extra,
            monthly,
            "sum".into(),
        )),
        None => summary(turn, client).await,
    }
}

pub async fn devices(turn: &mut Turn<'_>, client: &Client, count: u32) -> Result<Screen, BotError> {
    if let Some(cart) = turn.session.cart.as_mut() {
        cart.extra_devices = count;
    }
    summary(turn, client).await
}

fn request(cart: &Cart, promo_code: Option<String>) -> Option<CartRequest> {
    Some(CartRequest {
        tariff_id: cart.tariff_id?,
        period_id: cart.period_id?,
        extra_devices: cart.extra_devices,
        subscription_id: cart.subscription_id,
        promo_code,
    })
}

pub async fn summary(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let Some(cart) = turn.session.cart.clone() else {
        return start(turn, client, "").await;
    };
    let Some(cart_request) = request(&cart, turn.session.promo.clone()) else {
        return catalog(turn).await;
    };
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let mut notes = Vec::new();
    let view = match services
        .checkout()
        .quote_in(client.id, &cart_request, Channel::Bot)
        .await
    {
        Ok(view) => view,
        Err(err) if cart_request.promo_code.is_some() => {
            notes.push(ui::text(
                c,
                "buy.promo_removed",
                &[("error", Text(&text::user_error(&err)))],
            ));
            turn.session.promo = None;
            let plain = CartRequest {
                promo_code: None,
                ..cart_request.clone()
            };
            match services
                .checkout()
                .quote_in(client.id, &plain, Channel::Bot)
                .await
            {
                Ok(view) => view,
                Err(err) => return Ok(Screen::notice(text::user_error(&err))),
            }
        }
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let offset = services.settings().remna_offset();
    let quote = &view.quote;
    let until = text::date(view.expire_preview, offset);
    let tariff = tariff_name(&services, cart.tariff_id).await;
    let current = view
        .current
        .as_ref()
        .and_then(|current| current.tariff.clone())
        .unwrap_or_else(|| lang::pick("без тарифа", "no plan").to_string());
    let plan_text = ui::text(
        c,
        match (view.kind, view.current.as_ref().is_some_and(|s| s.trial)) {
            (PlanKind::Renewal, _) => "buy.summary_renewal",
            (PlanKind::Change, true) => "buy.summary_from_trial",
            (PlanKind::Change, false) => "buy.summary_change",
            (PlanKind::New, _) => "buy.summary_new",
        },
        &[
            ("date", Text(&until)),
            ("tariff", Text(&tariff)),
            ("current", Text(&current)),
        ],
    );
    let (before, after) = match view.kind {
        PlanKind::Change => (plan_text, String::new()),
        PlanKind::New | PlanKind::Renewal => (String::new(), plan_text),
    };
    let mut lines = Lines::new();
    lines.add(ui::text(c, "buy.summary_title", &[]));
    lines.add(before);
    for line in &quote.lines {
        lines.add(ui::text(
            c,
            "buy.summary_line",
            &[
                ("label", Text(&line.shown_label())),
                ("amount", Text(&text::money_signed(line.amount))),
            ],
        ));
    }
    let total = text::money(quote.total);
    lines.add(match quote.stars {
        Some(stars) => ui::text(
            c,
            "buy.summary_total_stars",
            &[
                ("total", Text(&total)),
                ("stars", Text(&text::stars(stars.total))),
            ],
        ),
        None => ui::text(c, "buy.summary_total", &[("total", Text(&total))]),
    });
    lines.add(after);
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
                        "pay:{}",
                        pay_target(method.provider, method.method.as_deref())
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
    if view
        .methods
        .iter()
        .any(|m| m.needs_email && m.unavailable.is_none())
    {
        let billing = services.checkout().billing_email(client.id).await?;
        if let (true, Some(email)) = (billing.editable(), billing.receipt.as_deref()) {
            lines.add(ui::text(
                c,
                "buy.email_line",
                &[("email", Text(&mask_email(email)))],
            ));
            keyboard = keyboard.push(ui::button(c, "buy.btn.email_change", &[], "rcpt:sum"));
        }
    }
    if let Some(promotion) = &view.promotion {
        if promotion.code_skipped {
            notes.push(ui::text(
                c,
                "buy.promo_beaten",
                &[("promotion", Text(&promotion.name))],
            ));
        } else if !promotion.applied {
            notes.push(ui::text(
                c,
                "buy.promotion_beaten",
                &[("promotion", Text(&promotion.name))],
            ));
        }
    }
    for note in notes {
        lines.add(note);
    }
    if sellable_for(&services, cart.tariff_id).await {
        keyboard = keyboard.push(ui::button(c, "buy.btn.change_devices", &[], "dv"));
    }
    if services.modules().is_enabled(ModuleId::Promo) {
        keyboard = keyboard.push(match turn.session.promo {
            Some(_) => ui::button(c, "buy.btn.promo_remove", &[], "nopromo"),
            None => ui::button(c, "buy.btn.promo", &[], "promo"),
        });
    }
    let back = match cart.tariff_id {
        Some(tariff) => format!("t:{tariff}"),
        None => back_target(&cart),
    };
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.pair(ui::back(c, back), ui::menu(c)))
        .section(Section::Buy))
}

async fn sellable_for(services: &Services, tariff_id: Option<TariffId>) -> bool {
    let Some(tariff_id) = tariff_id else {
        return false;
    };
    let tariff = match services.store().conn().await {
        Ok(mut conn) => conn.tariffs().get(tariff_id).await.ok(),
        Err(_) => None,
    };
    tariff.is_some_and(|tariff| sellable(services, &tariff).is_some())
}

pub fn ask_promo(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::PromoForCart);
    let content = turn.content();
    Screen::new(ui::text(&content, "buy.ask_promo", &[]))
        .keyboard(Keyboard::new().push(ui::cancel(&content, "sum")))
        .section(Section::Buy)
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
    let tariff = turn.session.cart.as_ref().and_then(|c| c.tariff_id);
    match promo::check(
        &ctx,
        client.id,
        raw,
        Purpose::Check {
            tariff,
            currency: services.settings().currency,
        },
    )
    .await
    {
        Ok(code) if code.is_discount() => {
            turn.session.promo = Some(code.code);
            summary(turn, client).await
        }
        Ok(_) => Ok(Screen::new(promo::PromoRejection::DaysCode.message())
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "buy.btn.promo_activate", &[], "x:promo"))
                    .push(ui::button(c, "buy.btn.back_to_payment", &[], "sum")),
            )
            .section(Section::Buy)),
        Err(PromoError::Rejected(rejection)) => Ok(Screen::new(rejection.message())
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "buy.btn.promo_other", &[], "promo"))
                    .push(ui::button(c, "buy.btn.no_promo", &[], "sum")),
            )
            .section(Section::Buy)),
        Err(PromoError::Module(err)) => Ok(Screen::notice(text::user_error(&err.into()))),
    }
}

pub(crate) async fn send_gram_widget(
    turn: &Turn<'_>,
    payment_id: Uuid,
    what: &str,
) -> Result<bool, BotError> {
    let services = turn.services();
    let payment = {
        let mut conn = services.store().conn().await?;
        conn.payments().by_public_id(payment_id).await?
    };
    let Some(widget) = gram_wallet::compose(services, &payment, what) else {
        return Ok(false);
    };
    match turn.ctx.api.send_widget(turn.target.chat_id, &widget).await {
        Ok(_) => Ok(true),
        Err(err) => {
            tracing::warn!(error = %err, "виджет оплаты GRAM не отправлен, показываем ссылку");
            Ok(false)
        }
    }
}

pub(crate) fn pay_target(provider: &str, method: Option<&str>) -> String {
    match method {
        Some(code) => format!("{provider}:{code}"),
        None => provider.to_string(),
    }
}

pub(crate) fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once(':') {
        Some((provider, code)) if !code.is_empty() => (provider, Some(code)),
        Some((provider, _)) => (provider, None),
        None => (target, None),
    }
}

async fn period_term(services: &Services, cart: &Cart) -> String {
    let (Some(tariff_id), Some(period_id)) = (cart.tariff_id, cart.period_id) else {
        return String::new();
    };
    let tariff = match services.store().conn().await {
        Ok(mut conn) => conn.tariffs().get(tariff_id).await.ok(),
        Err(_) => None,
    };
    tariff
        .and_then(|t| t.periods.into_iter().find(|p| p.id == period_id))
        .map(|p| text::term(p.days))
        .unwrap_or_default()
}

pub(crate) fn back_from(turn: &Turn<'_>, target: &str) -> String {
    turn.data
        .as_deref()
        .and_then(|data| data.strip_suffix(target))
        .map(|data| data.trim_end_matches(':').to_string())
        .filter(|data| !data.is_empty())
        .unwrap_or_else(|| "m".to_string())
}

pub(crate) async fn receipt_gate(
    turn: &mut Turn<'_>,
    client: &Client,
    provider: &str,
    method: Option<&str>,
    back: &str,
) -> Result<Option<Screen>, BotError> {
    let services = turn.services().clone();
    if !services
        .checkout()
        .email_needed(client.id, provider, method)
        .await?
    {
        return Ok(None);
    }
    turn.session.receipt_resume = turn.data.clone();
    turn.session.receipt_back = Some(back.to_string());
    turn.session.wait_for(Awaiting::ReceiptEmail);
    let content = turn.content();
    let title = services
        .payments()
        .get(provider)
        .map_or_else(|| provider.to_string(), |p| p.title().to_string());
    Ok(Some(
        Screen::new(ui::text(
            &content,
            "buy.email_ask",
            &[("provider", Text(&title))],
        ))
        .keyboard(Keyboard::new().push(ui::cancel(&content, back.to_string())))
        .section(Section::Buy),
    ))
}

pub async fn receipt_email_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    match services
        .clients()
        .set_receipt_email(client.id, raw, Actor::Client(client.id))
        .await
    {
        Ok(_) => {
            turn.session.receipt_back = None;
            match turn.session.receipt_resume.take() {
                Some(data) => crate::handler::callback(turn, client, &data).await,
                None => crate::handler::main_menu(turn.ctx, client).await,
            }
        }
        Err(ServiceError::Rejected { code, .. }) if code == RECEIPT_EMAIL_INVALID => {
            turn.session.wait_for(Awaiting::ReceiptEmail);
            let content = turn.content();
            let back = turn
                .session
                .receipt_back
                .clone()
                .unwrap_or_else(|| "m".to_string());
            Ok(Screen::new(ui::text(&content, "buy.email_invalid", &[]))
                .keyboard(Keyboard::new().push(ui::cancel(&content, back)))
                .section(Section::Buy))
        }
        Err(err) => Err(err.into()),
    }
}

pub async fn change_receipt_email(
    turn: &mut Turn<'_>,
    _client: &Client,
    back: &str,
) -> Result<Screen, BotError> {
    let back = if back.is_empty() { "sum" } else { back };
    turn.session.receipt_resume = Some(back.to_string());
    turn.session.receipt_back = Some(back.to_string());
    turn.session.wait_for(Awaiting::ReceiptEmail);
    let content = turn.content();
    Ok(Screen::new(ui::text(&content, "buy.email_change_ask", &[]))
        .keyboard(Keyboard::new().push(ui::cancel(&content, back.to_string())))
        .section(Section::Buy))
}

pub async fn pay(turn: &mut Turn<'_>, client: &Client, target: &str) -> Result<Screen, BotError> {
    let (provider, method) = split_target(target);
    let Some(cart) = turn.session.cart.clone() else {
        return start(turn, client, "").await;
    };
    let Some(cart_request) = request(&cart, turn.session.promo.clone()) else {
        return catalog(turn).await;
    };
    if let Some(screen) = receipt_gate(turn, client, provider, method, "sum").await? {
        return Ok(screen);
    }
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let title = tariff_name(&services, cart.tariff_id).await;
    let term = period_term(&services, &cart).await;
    let result = services
        .checkout()
        .start(
            client.id,
            &CheckoutRequest {
                cart: cart_request,
                provider: provider.to_string(),
                method: method.map(str::to_string),
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await;
    match result {
        Ok(result) => {
            turn.session.cart = None;
            turn.session.promo = None;
            let what = ui::text(
                &content,
                "pay.what_tariff",
                &[("tariff", Text(&title)), ("term", Text(&term))],
            );
            if let CheckoutResult::Redirect { payment_id, .. } = &result
                && provider == providers::GRAM
                && send_gram_widget(turn, *payment_id, &what).await?
            {
                return Ok(Screen::new(ui::text(
                    &content,
                    "pay.gram_ready",
                    &[("what", Html(&what))],
                ))
                .keyboard(Keyboard::new().push(ui::menu(&content)))
                .section(Section::Payment));
            }
            Ok(payment_screen(&content, &result, &what))
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}
