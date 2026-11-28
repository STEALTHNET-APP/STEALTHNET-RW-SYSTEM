use crate::BotError;
use crate::handler::{Screen, Turn, live, status_text, subscription_names, tariff_name};
use crate::session::{Awaiting, DevicePick};
use crate::telegram::{Button, Keyboard};
use crate::text;
use crate::ui::{self, Html, Lines, Text};
use sn_domain::bot_content::Section;
use sn_domain::bot_layout::{Screen as LayoutScreen, SubscriptionFacts, arrange};
use sn_domain::client::{Client, Locale};
use sn_domain::ids::{SubscriptionId, TicketId};
use sn_domain::lang;
use sn_domain::ledger::Wallet;
use sn_domain::money::{Currency, Money, PayCurrency, parse_units};
use sn_domain::pricing::{BonusTerms, topup_presets, topup_quote};
use sn_domain::settings::{TRIAL_NAME_MAX, TrialVariant};
use sn_domain::traffic::{NextReset, next_reset};
use sn_modules::catalog::antibot;
use sn_payments::{MethodsContext, ProviderKind};
use sn_services::bot_content::BotContent;
use sn_services::checkout::{Channel, CheckoutResult};
use sn_services::renewals::LinkStatus;
use sn_services::support::Delivered;
use sn_services::{Actor, ServiceError, Services};
use sn_store::{Db, StoreError};

pub fn payment_screen(c: &BotContent, result: &CheckoutResult, what: &str) -> Screen {
    match result {
        CheckoutResult::Redirect { pay_url, .. } if pay_url.starts_with("http") => {
            Screen::new(ui::text(c, "pay.ready", &[("what", Html(what))]))
                .keyboard(
                    Keyboard::new()
                        .push(ui::link(c, "pay.btn.pay", &[], pay_url.clone()))
                        .push(ui::menu(c)),
                )
                .section(Section::Payment)
        }
        CheckoutResult::Redirect { pay_url, .. } => Screen::notice(ui::text(
            c,
            "pay.link",
            &[("what", Html(what)), ("url", Text(pay_url))],
        ))
        .section(Section::Payment),
        CheckoutResult::Completed { .. } => Screen::new(ui::text(c, "pay.balance_done", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "common.btn.subscription", &[], "subs"))
                    .push(ui::menu(c)),
            )
            .section(Section::Payment),
    }
}

pub async fn subscriptions(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let list = live(services.subscriptions().list_for_client(client.id).await?);
    match list.as_slice() {
        [] => Ok(Screen::new(ui::text(c, "sub.none", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "sub.btn.buy", &[], "buy"))
                    .push(ui::menu(c)),
            )
            .section(Section::Subscription)),
        [only] => subscription(turn, client, only.id.0).await,
        many => {
            let offset = services.settings().remna_offset();
            let names = subscription_names(&services, many).await;
            let mut keyboard = Keyboard::new();
            for (sub, name) in many.iter().zip(names) {
                let button = match sub.mirror.expire_at {
                    Some(at) => ui::button(
                        c,
                        "sub.btn.item",
                        &[
                            ("tariff", Text(&name)),
                            ("date", Text(&text::day_month(at, offset))),
                        ],
                        format!("sub:{}", sub.id),
                    ),
                    None => ui::button(
                        c,
                        "sub.btn.item_open",
                        &[("tariff", Text(&name))],
                        format!("sub:{}", sub.id),
                    ),
                };
                keyboard = keyboard.push(button);
            }
            Ok(Screen::new(ui::text(c, "sub.choose", &[]))
                .keyboard(keyboard.push(ui::menu(c)))
                .section(Section::Subscription))
        }
    }
}

fn not_found(err: &ServiceError) -> bool {
    matches!(
        err,
        ServiceError::NotFound(_) | ServiceError::Store(StoreError::NotFound(_))
    )
}

pub async fn subscription(
    turn: &mut Turn<'_>,
    client: &Client,
    id: i64,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let actor = Actor::Client(client.id);
    let sid = SubscriptionId(id);
    let (view, link_error) = match services.subscriptions().view(sid, actor, true).await {
        Ok(view) => (view, None),
        Err(err) if not_found(&err) => {
            return Ok(
                Screen::notice(ui::text(c, "sub.not_found", &[])).section(Section::Subscription)
            );
        }
        Err(err) => match services.subscriptions().view(sid, actor, false).await {
            Ok(view) => (view, Some(err)),
            Err(_) => return Ok(Screen::notice(text::user_error(&err))),
        },
    };
    let sub = &view.subscription;
    let now = services.now();
    let settings = services.settings();
    let offset = settings.remna_offset();
    let name = tariff_name(&services, sub.tariff_id).await;
    let status = status_text(c, sub, now);
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "sub.title",
        &[("tariff", Text(&name)), ("status", Html(&status))],
    ));
    match sub.mirror.expire_at {
        Some(at) if at > now => lines.add(ui::text(
            c,
            "sub.until",
            &[
                ("date", Text(&text::date(at, offset))),
                ("days", Text(&text::plural_days(text::days_left(at, now)))),
            ],
        )),
        Some(at) => lines.add(ui::text(
            c,
            "sub.ended",
            &[("date", Text(&text::date(at, offset)))],
        )),
        None => {}
    }
    match sub.mirror.traffic_limit_bytes.filter(|l| *l > 0) {
        Some(limit) => {
            let used = sub.mirror.traffic_used_bytes.unwrap_or(0);
            let reset = match sub.tariff_id {
                Some(tariff_id) => {
                    let mut conn = services.store().conn().await?;
                    conn.tariffs()
                        .get(tariff_id)
                        .await
                        .ok()
                        .map(|t| next_reset(t.traffic.reset, sub.created_at, now, offset))
                }
                None => None,
            };
            let reset = match reset {
                Some(NextReset::At(at)) => ui::text(
                    c,
                    "sub.traffic_reset_date",
                    &[("date", Text(&text::day_month(at, offset)))],
                ),
                Some(NextReset::OnPayment) => ui::text(c, "sub.traffic_reset_payment", &[]),
                _ => String::new(),
            };
            lines.add(ui::text(
                c,
                "sub.traffic",
                &[
                    ("used", Text(&text::traffic(used))),
                    ("limit", Text(&text::traffic(limit))),
                    ("reset", Html(&reset)),
                ],
            ));
        }
        None => lines.add(ui::text(c, "sub.traffic_unlimited", &[])),
    }
    match (sub.mirror.devices_count, sub.mirror.hwid_limit) {
        (Some(count), Some(limit)) => lines.add(ui::text(
            c,
            "sub.devices",
            &[
                ("count", Text(&count.to_string())),
                ("limit", Text(&limit.to_string())),
            ],
        )),
        (None, Some(limit)) => lines.add(ui::text(
            c,
            "sub.devices_limit",
            &[("limit", Text(&limit.to_string()))],
        )),
        _ => {}
    }
    let renewal = view.renewal.as_ref().filter(|r| r.status.live());
    match renewal {
        Some(renewal) if renewal.status == LinkStatus::Active => lines.add(ui::text(
            c,
            "sub.renewal_on",
            &[
                ("provider", Text(&renewal.title)),
                (
                    "amount",
                    Html(&renewal.amount.map(text::money).unwrap_or_default()),
                ),
                (
                    "date",
                    Text(
                        &renewal
                            .next_charge_at
                            .map(|at| text::date(at, offset))
                            .unwrap_or_default(),
                    ),
                ),
            ],
        )),
        Some(renewal) => lines.add(ui::text(
            c,
            "sub.renewal_cancelling",
            &[("provider", Text(&renewal.title))],
        )),
        None => lines.add(ui::text(
            c,
            if sub.autorenew.enabled {
                "sub.autorenew_on"
            } else {
                "sub.autorenew_off"
            },
            &[],
        )),
    }
    let mut connect = None;
    match (&view.subscription_url, &link_error) {
        (Some(url), _) => {
            lines.add(ui::text(c, "sub.link", &[("url", Text(url))]));
            if url.starts_with("http") {
                connect = Some(url.clone());
            }
        }
        (None, Some(err)) => {
            lines.add(ui::text(
                c,
                "sub.link_error",
                &[("error", Text(&text::user_error(err)))],
            ));
        }
        (None, None) => {}
    }
    let facts = SubscriptionFacts {
        link: connect.is_some(),
        remna: sub.remna.is_some(),
        reissue: settings.client_access.self_reissue,
        renewal: renewal.is_some_and(|r| r.status == LinkStatus::Active),
    };
    let present: Vec<(&str, Button)> = facts
        .keys()
        .into_iter()
        .filter_map(|key| {
            let button = match key {
                "sub.btn.connect" => connect
                    .as_ref()
                    .and_then(|url| ui::link(c, key, &[], url.clone())),
                "sub.btn.copy_link" => connect.as_ref().and_then(|url| ui::copy(c, key, url)),
                "sub.btn.devices" => ui::button(c, key, &[], format!("dev:{id}")),
                "sub.btn.reissue" => ui::button(c, key, &[], format!("rei:{id}")),
                "sub.btn.renewal_off" => ui::button(c, key, &[], format!("rno:{id}")),
                "sub.btn.extend" => ui::button(c, key, &[], format!("buy:{id}")),
                "sub.btn.all" => ui::button(c, key, &[], "subs"),
                "common.btn.menu" => ui::menu(c),
                _ => None,
            };
            button.map(|button| (key, button))
        })
        .collect();
    let keyboard = Keyboard::new().rows(arrange(
        &c.content.layout(LayoutScreen::Subscription),
        present,
    ));
    Ok(Screen::new(lines.text())
        .keyboard(keyboard)
        .section(Section::Subscription))
}

pub async fn devices(turn: &mut Turn<'_>, client: &Client, id: i64) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let list = match services
        .subscriptions()
        .devices(SubscriptionId(id), Actor::Client(client.id))
        .await
    {
        Ok(list) => list,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let back = Keyboard::new().pair(ui::back(c, format!("sub:{id}")), ui::menu(c));
    if list.is_empty() {
        turn.session.devices = None;
        return Ok(Screen::new(ui::text(c, "dev.empty", &[]))
            .keyboard(back)
            .section(Section::Devices));
    }
    let offset = services.settings().remna_offset();
    let may_disconnect = services.settings().client_access.self_devices;
    let mut lines = Lines::new();
    lines.add(ui::text(c, "dev.title", &[]));
    let mut keyboard = Keyboard::new();
    for (index, device) in list.iter().enumerate() {
        let details: Vec<String> = [
            match (&device.platform, &device.os_version) {
                (Some(platform), Some(version)) => Some(format!("{platform} {version}")),
                (Some(platform), None) => Some(platform.clone()),
                _ => None,
            },
            device.app.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let number = (index + 1).to_string();
        let date = text::day_month(device.added_at, offset);
        lines.add(if details.is_empty() {
            ui::text(
                c,
                "dev.line_plain",
                &[
                    ("n", Text(&number)),
                    ("title", Text(&device.title)),
                    ("date", Text(&date)),
                ],
            )
        } else {
            ui::text(
                c,
                "dev.line",
                &[
                    ("n", Text(&number)),
                    ("title", Text(&device.title)),
                    ("details", Text(&details.join(" · "))),
                    ("date", Text(&date)),
                ],
            )
        });
        if may_disconnect {
            keyboard = keyboard.push(ui::button(
                c,
                "dev.btn.disconnect",
                &[("title", Text(&device.title))],
                format!("dx:{index}"),
            ));
        }
    }
    turn.session.devices = Some(DevicePick {
        subscription_id: SubscriptionId(id),
        hwids: list.into_iter().map(|d| (d.hwid, d.title)).collect(),
    });
    keyboard.inline_keyboard.extend(back.inline_keyboard);
    Ok(Screen::new(lines.text())
        .keyboard(keyboard)
        .section(Section::Devices))
}

fn picked(turn: &Turn<'_>, rest: &str) -> Option<(SubscriptionId, usize, String, String)> {
    let index: usize = rest.parse().ok()?;
    let pick = turn.session.devices.as_ref()?;
    let (hwid, title) = pick.hwids.get(index)?;
    Some((pick.subscription_id, index, hwid.clone(), title.clone()))
}

pub async fn device_confirm(turn: &mut Turn<'_>, rest: &str) -> Result<Screen, BotError> {
    let content = turn.content();
    let c = &*content;
    let Some((sub, index, _, title)) = picked(turn, rest) else {
        return Ok(Screen::notice(ui::text(c, "dev.stale", &[])).section(Section::Devices));
    };
    Ok(
        Screen::new(ui::text(c, "dev.confirm", &[("title", Text(&title))]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(
                        c,
                        "dev.btn.disconnect_yes",
                        &[],
                        format!("dxy:{index}"),
                    ))
                    .push(ui::cancel(c, format!("dev:{sub}"))),
            )
            .section(Section::Devices),
    )
}

pub async fn device_disconnect(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    let content = turn.content();
    let c = &*content;
    let Some((sub, _, hwid, title)) = picked(turn, rest) else {
        return Ok(Screen::notice(ui::text(c, "dev.stale", &[])).section(Section::Devices));
    };
    match turn
        .services()
        .subscriptions()
        .disconnect_device(sub, &hwid, Actor::Client(client.id))
        .await
    {
        Ok(()) => {
            turn.session.devices = None;
            Ok(
                Screen::new(ui::text(c, "dev.disconnected", &[("title", Text(&title))]))
                    .keyboard(
                        Keyboard::new()
                            .push(ui::button(c, "sub.btn.devices", &[], format!("dev:{sub}")))
                            .push(ui::menu(c)),
                    )
                    .section(Section::Devices),
            )
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

pub fn reissue_confirm(c: &BotContent, id: i64) -> Screen {
    Screen::new(ui::text(c, "sub.reissue_confirm", &[]))
        .keyboard(
            Keyboard::new()
                .push(ui::button(
                    c,
                    "sub.btn.reissue_yes",
                    &[],
                    format!("reiy:{id}"),
                ))
                .push(ui::cancel(c, format!("sub:{id}"))),
        )
        .section(Section::Subscription)
}

pub async fn renewal_confirm(
    turn: &mut Turn<'_>,
    client: &Client,
    id: i64,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let view = match services
        .subscriptions()
        .view(SubscriptionId(id), Actor::Client(client.id), false)
        .await
    {
        Ok(view) => view,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let Some(renewal) = view
        .renewal
        .as_ref()
        .filter(|r| r.status == LinkStatus::Active)
    else {
        return subscription(turn, client, id).await;
    };
    let date = view
        .subscription
        .mirror
        .expire_at
        .map(|at| text::date(at, services.settings().remna_offset()))
        .unwrap_or_default();
    Ok(Screen::new(ui::text(
        c,
        "sub.renewal_confirm",
        &[("provider", Text(&renewal.title)), ("date", Text(&date))],
    ))
    .keyboard(
        Keyboard::new()
            .push(ui::button(
                c,
                "sub.btn.renewal_off_yes",
                &[],
                format!("rnoy:{id}"),
            ))
            .push(ui::cancel(c, format!("sub:{id}"))),
    )
    .section(Section::Subscription))
}

pub async fn renewal_off(
    turn: &mut Turn<'_>,
    client: &Client,
    id: i64,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let actor = Actor::Client(client.id);
    let sid = SubscriptionId(id);
    if let Err(err) = services
        .subscriptions()
        .set_autorenew(sid, false, None, actor)
        .await
    {
        return Ok(Screen::notice(text::user_error(&err)));
    }
    let view = match services.subscriptions().view(sid, actor, false).await {
        Ok(view) => view,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let title = view
        .renewal
        .as_ref()
        .map(|r| r.title.clone())
        .unwrap_or_default();
    let date = view
        .subscription
        .mirror
        .expire_at
        .map(|at| text::date(at, services.settings().remna_offset()))
        .unwrap_or_default();
    let line = match view.renewal.as_ref().map(|r| r.status) {
        Some(LinkStatus::Cancelling) => {
            ui::text(c, "sub.renewal_pending", &[("provider", Text(&title))])
        }
        _ => ui::text(
            c,
            "sub.renewal_done",
            &[("provider", Text(&title)), ("date", Text(&date))],
        ),
    };
    Ok(Screen::new(line)
        .keyboard(
            Keyboard::new()
                .push(ui::back(c, format!("sub:{id}")))
                .push(ui::menu(c)),
        )
        .section(Section::Subscription))
}

pub async fn reissue(turn: &mut Turn<'_>, client: &Client, id: i64) -> Result<Screen, BotError> {
    let content = turn.content();
    let c = &*content;
    match turn
        .services()
        .subscriptions()
        .reissue_link(SubscriptionId(id), Actor::Client(client.id))
        .await
    {
        Ok(url) => {
            let mut keyboard = Keyboard::new();
            if url.starts_with("http") {
                keyboard = keyboard
                    .push(ui::link(c, "sub.btn.connect", &[], url.clone()))
                    .push(ui::copy(c, "sub.btn.copy_link", &url));
            }
            Ok(
                Screen::new(ui::text(c, "sub.reissued", &[("url", Text(&url))]))
                    .keyboard(keyboard.push(ui::menu(c)))
                    .section(Section::Subscription),
            )
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

pub async fn balance(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let settings = services.settings();
    let currency = settings.currency;
    let (main, referral) = {
        let mut conn = services.store().conn().await?;
        (
            conn.ledger()
                .balance(client.id, Wallet::Main, currency)
                .await?,
            conn.ledger()
                .balance(client.id, Wallet::Referral, currency)
                .await?,
        )
    };
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "bal.amount",
        &[("amount", Text(&text::money(main)))],
    ));
    if referral.minor > 0 {
        lines.add(ui::text(
            c,
            "bal.referral",
            &[("amount", Text(&text::money(referral)))],
        ));
    }
    if !services.payments().balance_enabled() {
        lines.add(ui::text(c, "bal.off_note", &[]));
        return Ok(Screen::new(lines.text())
            .keyboard(Keyboard::new().push(ui::menu(c)))
            .section(Section::Balance));
    }
    lines.add(ui::text(c, "bal.note", &[]));
    let buttons: Vec<_> = topup_presets(settings.topup_limits())
        .into_iter()
        .filter_map(|amount| {
            ui::button(
                c,
                "bal.btn.preset",
                &[("amount", Text(&text::money(amount)))],
                format!("top:{}", amount.minor),
            )
        })
        .collect();
    Ok(Screen::new(lines.text())
        .keyboard(
            Keyboard::new()
                .grid(buttons, 2)
                .push(ui::button(c, "bal.btn.custom", &[], "topx"))
                .push(ui::menu(c)),
        )
        .section(Section::Balance))
}

fn bonus_title(title: &str, terms: Option<BonusTerms>, amount: Money) -> String {
    match terms {
        Some(terms) if terms.reached(amount) => format!("{title} +{}%", terms.percent),
        Some(BonusTerms {
            percent,
            min_minor: Some(min),
            ..
        }) => {
            let from = text::money(Money::new(min, amount.currency));
            match lang::current() {
                Locale::Ru => format!("{title} +{percent}% от {from}"),
                Locale::En => format!("{title} +{percent}% from {from}"),
            }
        }
        _ => title.to_string(),
    }
}

pub async fn topup_amount(turn: &mut Turn<'_>, minor: i64) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let settings = services.settings();
    let amount = Money::new(minor, settings.currency);
    if let Err(err) = topup_quote(amount, settings.topup_limits()) {
        turn.session.wait_for(Awaiting::TopUpAmount);
        return Ok(Screen::notice(ui::text(
            c,
            "bal.bad_amount",
            &[("error", Text(&text::capitalize(&err.to_string())))],
        ))
        .section(Section::Balance));
    }
    let methods: Vec<_> = services
        .payments()
        .checkout_methods(&MethodsContext {
            currency: settings.currency,
            total: amount,
            stars_total: None,
            balance: None,
            channel: Some(sn_domain::access::ClientChannel::Bot),
        })
        .into_iter()
        .filter(|m| m.kind == ProviderKind::External && m.unavailable.is_none())
        .filter(|m| m.method.as_deref() != Some(sn_domain::payment::RECURRING_METHOD))
        .collect();
    if methods.is_empty() {
        return Ok(Screen::notice(ui::text(c, "bal.no_methods", &[])).section(Section::Balance));
    }
    turn.session.topup_minor = Some(minor);
    let mut keyboard = Keyboard::new();
    for method in methods {
        let data = format!(
            "tp:{}",
            crate::buy::pay_target(method.provider, method.method.as_deref())
        );
        let charged = method
            .amount
            .and_then(sn_domain::money::Amount::as_money)
            .filter(|charged| *charged != amount);
        let terms = services.payments().topup_bonus(
            method.provider,
            method.method.as_deref(),
            settings.currency,
        );
        let title = bonus_title(&method.title, terms, charged.unwrap_or(amount));
        keyboard = keyboard.push(if let Some(charged) = charged {
            ui::button(
                c,
                "bal.btn.method_hint",
                &[
                    ("method", Text(&title)),
                    ("hint", Text(&text::money(charged))),
                ],
                data,
            )
        } else if method.hint.is_empty() {
            ui::button(c, "bal.btn.method", &[("method", Text(&title))], data)
        } else {
            ui::button(
                c,
                "bal.btn.method_hint",
                &[("method", Text(&title)), ("hint", Text(&method.hint))],
                data,
            )
        });
    }
    Ok(Screen::new(ui::text(
        c,
        "bal.choose_method",
        &[("amount", Text(&text::money(amount)))],
    ))
    .keyboard(keyboard.push(ui::back(c, "bal")))
    .section(Section::Balance))
}

pub fn topup_custom(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::TopUpAmount);
    let content = turn.content();
    let example = match turn.services().settings().currency {
        Currency::Usd => lang::pick("например 10 или 12.50", "for example 10 or 12.50"),
        Currency::Rub => lang::pick("например 500", "for example 500"),
    };
    Screen::new(ui::text(
        &content,
        "bal.enter_amount",
        &[("example", Text(example))],
    ))
    .keyboard(Keyboard::new().push(ui::cancel(&content, "bal")))
    .section(Section::Balance)
}

pub async fn topup_entered(turn: &mut Turn<'_>, raw: &str) -> Result<Screen, BotError> {
    let currency = turn.services().settings().currency;
    let cleaned: String = raw
        .trim()
        .trim_end_matches(['$', '₽'])
        .trim_start_matches('$')
        .replace(',', ".")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}')
        .collect();
    match parse_units(&cleaned, PayCurrency::from(currency)) {
        Some(minor) if minor > 0 => topup_amount(turn, minor).await,
        _ => {
            turn.session.wait_for(Awaiting::TopUpAmount);
            let content = turn.content();
            Ok(Screen::new(ui::text(&content, "bal.not_understood", &[]))
                .keyboard(Keyboard::new().push(ui::cancel(&content, "bal")))
                .section(Section::Balance))
        }
    }
}

pub async fn topup_start(
    turn: &mut Turn<'_>,
    client: &Client,
    target: &str,
) -> Result<Screen, BotError> {
    let (provider, method) = crate::buy::split_target(target);
    let Some(minor) = turn.session.topup_minor else {
        return balance(turn, client).await;
    };
    if let Some(screen) = crate::buy::receipt_gate(turn, client, provider, method, "bal").await? {
        return Ok(screen);
    }
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let currency = services.settings().currency;
    let asked = Money::new(minor, currency);
    let registry = services.payments();
    let amount = registry
        .get(provider)
        .and_then(|found| {
            sn_payments::charge(
                found.as_ref(),
                &registry.config(found.id()),
                &MethodsContext {
                    currency,
                    total: asked,
                    stars_total: None,
                    balance: None,
                    channel: Some(sn_domain::access::ClientChannel::Bot),
                },
            )
        })
        .unwrap_or(asked);
    match services
        .checkout()
        .topup(client.id, amount, provider, method, Channel::Bot)
        .await
    {
        Ok(result) => {
            turn.session.topup_minor = None;
            let bonus = registry
                .topup_bonus(provider, method, currency)
                .map_or_else(|| Money::zero(currency), |terms| terms.bonus(amount));
            let shown = text::money(amount);
            let what = match amount.checked_add(bonus) {
                Ok(total) if !bonus.is_zero() => ui::text(
                    &content,
                    "pay.what_topup_bonus",
                    &[
                        ("amount", Text(&shown)),
                        ("total", Text(&text::money_exact(total))),
                        ("bonus", Text(&text::money_exact(bonus))),
                    ],
                ),
                _ => ui::text(&content, "pay.what_topup", &[("amount", Text(&shown))]),
            };
            Ok(payment_screen(&content, &result, &what))
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

pub async fn support_reply_prompt(
    turn: &mut Turn<'_>,
    client: &Client,
    ticket: i64,
) -> Result<Screen, BotError> {
    let open = match turn
        .services()
        .support()
        .telegram_ticket(client.id, Some(TicketId(ticket)))
        .await
    {
        Ok(open) => open,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let Some(open) = open else {
        return Ok(support_prompt(turn));
    };
    turn.session.support_ticket = Some(open.0);
    turn.session.wait_for(Awaiting::SupportMessage);
    let content = turn.content();
    let c = &*content;
    Ok(Screen::new(ui::text(
        c,
        "sup.reply_prompt",
        &[("id", Text(&open.to_string()))],
    ))
    .keyboard(Keyboard::new().push(ui::cancel(c, "m")))
    .section(Section::Support))
}

pub fn support_prompt(turn: &mut Turn<'_>) -> Screen {
    turn.session.support_ticket = None;
    turn.session.wait_for(Awaiting::SupportMessage);
    let content = turn.content();
    let c = &*content;
    let settings = turn.services().settings();
    let link = crate::handler::support_link(&settings);
    if let Some(url) = link
        && !settings.support_tickets()
    {
        turn.session.awaiting = None;
        return Screen::new(ui::text(c, "sup.link", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::link(c, "sup.btn.write", &[], url))
                    .push(ui::menu(c)),
            )
            .section(Section::Support);
    }
    let mut keyboard = Keyboard::new();
    if let Some(url) = link {
        keyboard = keyboard.push(ui::link(c, "sup.btn.other", &[], url));
    }
    Screen::new(ui::text(c, "sup.prompt", &[]))
        .keyboard(keyboard.push(ui::cancel(c, "m")))
        .section(Section::Support)
}

pub async fn support_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    body: &str,
) -> Result<Screen, BotError> {
    let preferred = turn.session.support_ticket.take();
    if body.trim().is_empty() {
        return match preferred {
            Some(ticket) => support_reply_prompt(turn, client, ticket).await,
            None => Ok(support_prompt(turn)),
        };
    }
    deliver(turn, client, body, preferred.map(TicketId)).await
}

pub async fn support_followup(
    turn: &mut Turn<'_>,
    client: &Client,
    body: &str,
) -> Result<Option<Screen>, BotError> {
    if body.trim().is_empty() {
        return Ok(None);
    }
    match turn
        .services()
        .support()
        .telegram_ticket(client.id, None)
        .await
    {
        Ok(Some(ticket)) => Ok(Some(deliver(turn, client, body, Some(ticket)).await?)),
        Ok(None) => Ok(None),
        Err(err) => Ok(Some(Screen::notice(text::user_error(&err)))),
    }
}

async fn deliver(
    turn: &mut Turn<'_>,
    client: &Client,
    body: &str,
    preferred: Option<TicketId>,
) -> Result<Screen, BotError> {
    let content = turn.content();
    let (key, id) = match turn
        .services()
        .support()
        .from_telegram(client.id, body, preferred)
        .await
    {
        Ok(Delivered::Opened(id)) => ("sup.sent", id),
        Ok(Delivered::Added(id)) => ("sup.added", id),
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    Ok(
        Screen::notice(ui::text(&content, key, &[("id", Text(&id.to_string()))]))
            .section(Section::Support),
    )
}

fn trial_started(c: &BotContent) -> Screen {
    Screen::new(ui::text(c, "sub.trial_started", &[]))
        .keyboard(
            Keyboard::new()
                .push(ui::button(c, "common.btn.subscription", &[], "subs"))
                .push(ui::menu(c)),
        )
        .section(Section::Subscription)
}

fn variant_title(variant: &TrialVariant, index: usize) -> String {
    let name: String = variant.name.trim().chars().take(TRIAL_NAME_MAX).collect();
    if name.is_empty() {
        format!("№{}", index + 1)
    } else {
        name
    }
}

fn offered_variants(turn: &Turn<'_>) -> Vec<TrialVariant> {
    turn.services()
        .settings()
        .trial
        .offered()
        .into_iter()
        .cloned()
        .collect()
}

fn trial_choice(c: &BotContent, variants: &[TrialVariant], note: Option<String>) -> Screen {
    let mut lines = Lines::new();
    if let Some(note) = note {
        lines.add(text::escape(&note));
    }
    lines.add(ui::text(c, "sub.trial_choose", &[]));
    let mut keyboard = Keyboard::new();
    for (index, variant) in variants.iter().enumerate() {
        let title = variant_title(variant, index);
        let days = text::days(variant.days);
        let count = variant.devices.to_string();
        lines.add(match variant.traffic.limit_bytes.filter(|b| *b > 0) {
            Some(bytes) => ui::text(
                c,
                "sub.trial_variant_line",
                &[
                    ("variant", Text(&title)),
                    ("days", Text(&days)),
                    ("count", Text(&count)),
                    ("limit", Text(&text::traffic(bytes))),
                ],
            ),
            None => ui::text(
                c,
                "sub.trial_variant_unlimited",
                &[
                    ("variant", Text(&title)),
                    ("days", Text(&days)),
                    ("count", Text(&count)),
                ],
            ),
        });
        keyboard = keyboard.push(ui::button(
            c,
            "sub.btn.trial_variant",
            &[("variant", Text(&title)), ("days", Text(&days))],
            format!("trial:{}", variant.id),
        ));
    }
    Screen::new(lines.text())
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Subscription)
}

fn verify_button(c: &BotContent, services: &Services, token: &str) -> Option<Button> {
    let site = sn_services::bot_layout::site_url(services)?;
    let url = format!("{site}/verify/{token}");
    if site.starts_with("https://") {
        ui::web_app(c, "sub.btn.trial_verify", url)
    } else {
        ui::link(c, "sub.btn.trial_verify", &[], url)
    }
}

async fn trial_check(
    turn: &mut Turn<'_>,
    client: &Client,
    variant: Option<u32>,
) -> Result<Screen, BotError> {
    let content = turn.content();
    let c = &*content;
    let services = turn.services().clone();
    let challenge = match services
        .checkout()
        .trial_challenge(client.id, antibot::Surface::Bot, variant)
        .await
    {
        Ok(challenge) => challenge,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    if challenge.captcha() {
        return Ok(match verify_button(c, &services, &challenge.token) {
            Some(button) => Screen::new(ui::text(c, "sub.trial_verify", &[]))
                .keyboard(Keyboard::new().push(Some(button)).push(ui::menu(c)))
                .section(Section::Subscription),
            None => Screen::notice(ui::text(c, "sub.trial_verify_off", &[])),
        });
    }
    Ok(Screen::new(ui::text(c, "sub.trial_confirm", &[]))
        .keyboard(
            Keyboard::new()
                .push(ui::button(
                    c,
                    "sub.btn.trial_confirm",
                    &[],
                    match variant {
                        Some(id) => format!("ab:{}:{id}", challenge.token),
                        None => format!("ab:{}", challenge.token),
                    },
                ))
                .push(ui::menu(c)),
        )
        .section(Section::Subscription))
}

async fn trial_start(
    turn: &mut Turn<'_>,
    client: &Client,
    variant: Option<u32>,
) -> Result<Screen, BotError> {
    let content = turn.content();
    match turn
        .services()
        .checkout()
        .start_trial(client.id, variant)
        .await
    {
        Ok(_) => Ok(trial_started(&content)),
        Err(err) if err.code() == antibot::CONFIRM_CODE => trial_check(turn, client, variant).await,
        Err(err) => Ok(trial_refused(turn, &err)),
    }
}

fn trial_refused(turn: &Turn<'_>, err: &ServiceError) -> Screen {
    let offered = offered_variants(turn);
    match err.code() {
        "trial.variant_required" if offered.len() > 1 => {
            trial_choice(&turn.content(), &offered, None)
        }
        "trial.variant_unavailable" if offered.len() > 1 => {
            trial_choice(&turn.content(), &offered, Some(text::user_error(err)))
        }
        _ => Screen::notice(text::user_error(err)),
    }
}

pub async fn trial(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let offered = offered_variants(turn);
    if offered.len() > 1 {
        return Ok(trial_choice(&turn.content(), &offered, None));
    }
    trial_start(turn, client, None).await
}

pub async fn trial_variant(
    turn: &mut Turn<'_>,
    client: &Client,
    variant: u32,
) -> Result<Screen, BotError> {
    trial_start(turn, client, Some(variant)).await
}

pub async fn trial_confirmed(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    let (token, variant) = match rest.split_once(':') {
        Some((token, id)) => (token, id.parse::<u32>().ok().filter(|id| *id > 0)),
        None => (rest, None),
    };
    let content = turn.content();
    match turn
        .services()
        .checkout()
        .confirm_trial(client.id, token, antibot::Answer::button(), variant)
        .await
    {
        Ok(_) => Ok(trial_started(&content)),
        Err(err) if matches!(err.code(), antibot::TOO_FAST | antibot::CONFIRM_CODE) => {
            trial_check(turn, client, variant).await
        }
        Err(err) => Ok(trial_refused(turn, &err)),
    }
}
