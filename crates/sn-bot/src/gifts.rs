use crate::BotError;
use crate::handler::{Screen, Turn, id_arg, main_menu};
use crate::session::Awaiting;
use crate::telegram::Keyboard;
use crate::text;
use crate::ui::{self, Html, Lines, Text};
use sn_domain::access::ClientChannel;
use sn_domain::bot_content::Section;
use sn_domain::client::Client;
use sn_domain::ids::{PeriodId, SubscriptionId, TariffId};
use sn_domain::lang;
use sn_domain::module::ModuleId;
use sn_domain::payment::providers;
use sn_domain::plan::PlanKind;
use sn_domain::subscription::SubscriptionKind;
use sn_modules::GrantTarget;
use sn_modules::catalog::antibot;
use sn_modules::catalog::gifts::{
    self, GiftError, GiftRejection, GiftRequest, GiftStatus, ITEM_GIFT, RedeemPreview,
};
use sn_modules::catalog::promolinks::{self, Activated, LinkError, LinkRejection};
use sn_payments::Unavailable;
use sn_services::bot_content::BotContent;
use sn_services::checkout::{Channel, CheckoutResult, ItemCheckoutRequest, ItemRequest};
use sn_store::Db;

pub const PROMO_CONFIRM: &str = "plr";

fn enabled(turn: &Turn<'_>) -> bool {
    turn.services().modules().is_enabled(ModuleId::Gifts)
}

fn home_button(c: &BotContent) -> Option<crate::telegram::Button> {
    ui::back(c, format!("x:{}", gifts::BOT_ENTRY))
}

fn item(request: GiftRequest) -> ItemRequest {
    ItemRequest {
        module: ModuleId::Gifts,
        kind: ITEM_GIFT.into(),
        payload: request.payload(),
        subscription_id: None,
    }
}

pub async fn home(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    if !enabled(turn) {
        return main_menu(turn.ctx, client).await;
    }
    let content = turn.content();
    let c = &*content;
    let has_gifts = !gifts::mine(&turn.services().module_context(), client.id)
        .await?
        .is_empty();
    let mut keyboard = Keyboard::new().push(ui::button(c, "gift.btn.buy", &[], "gb"));
    if has_gifts {
        keyboard = keyboard.push(ui::button(c, "gift.btn.mine", &[], "gl"));
    }
    Ok(Screen::new(ui::text(c, "gift.intro", &[]))
        .keyboard(
            keyboard
                .push(ui::button(c, "gift.btn.redeem", &[], "ga"))
                .push(ui::menu(c)),
        )
        .section(Section::Gifts))
}

pub async fn tariffs(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    if !enabled(turn) {
        return main_menu(turn.ctx, client).await;
    }
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let offers = {
        let mut conn = services.store().conn().await?;
        gifts::offers(
            &mut conn,
            services.settings().currency,
            services.now(),
            services.settings().remna_offset(),
        )
        .await?
    };
    if offers.is_empty() {
        return Ok(Screen::new(ui::text(c, "gift.no_tariffs", &[]))
            .keyboard(Keyboard::new().pair(home_button(c), ui::menu(c)))
            .section(Section::Gifts));
    }
    let mut keyboard = Keyboard::new();
    for offer in &offers {
        keyboard = keyboard.push(ui::button(
            c,
            "buy.btn.tariff",
            &[("tariff", Text(&offer.tariff.name))],
            format!("gt:{}", offer.tariff.id),
        ));
    }
    Ok(Screen::new(ui::text(c, "gift.choose_tariff", &[]))
        .keyboard(keyboard.pair(home_button(c), ui::menu(c)))
        .section(Section::Gifts))
}

pub async fn periods(
    turn: &mut Turn<'_>,
    client: &Client,
    tariff_id: TariffId,
) -> Result<Screen, BotError> {
    if !enabled(turn) {
        return main_menu(turn.ctx, client).await;
    }
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let offers = {
        let mut conn = services.store().conn().await?;
        gifts::offers(
            &mut conn,
            services.settings().currency,
            services.now(),
            services.settings().remna_offset(),
        )
        .await?
    };
    let Some(offer) = offers.into_iter().find(|o| o.tariff.id == tariff_id) else {
        return tariffs(turn, client).await;
    };
    let mut keyboard = Keyboard::new();
    for period in &offer.periods {
        let term = gifts::term(period.days);
        let price = text::money(period.price);
        let data = format!("gp:{}:{}", tariff_id, period.period_id);
        keyboard = keyboard.push(match &period.promotion {
            Some(promotion) => ui::button(
                c,
                "gift.btn.period_promo",
                &[
                    ("term", Text(&term)),
                    ("price", Text(&price)),
                    ("percent", Text(&promotion.percent.to_string())),
                ],
                data,
            ),
            None => ui::button(
                c,
                "gift.btn.period",
                &[("term", Text(&term)), ("price", Text(&price))],
                data,
            ),
        });
    }
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "gift.choose_period",
        &[("tariff", Text(&offer.tariff.name))],
    ));
    crate::buy::promotion_lines(c, &mut lines, &offer.periods);
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.pair(ui::back(c, "gb"), ui::menu(c)))
        .section(Section::Gifts))
}

fn parse_pick(rest: &str) -> Option<(GiftRequest, &str)> {
    let mut parts = rest.splitn(3, ':');
    let tariff = parts.next().and_then(id_arg)?;
    let period = parts.next().and_then(id_arg)?;
    Some((
        GiftRequest {
            tariff_id: TariffId(tariff),
            period_id: PeriodId(period),
        },
        parts.next().unwrap_or(""),
    ))
}

pub async fn summary(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    let Some((request, _)) = parse_pick(rest) else {
        return tariffs(turn, client).await;
    };
    if !enabled(turn) {
        return main_menu(turn.ctx, client).await;
    }
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let view = match services
        .checkout()
        .quote_item_in(client.id, &item(request), Some(ClientChannel::Bot))
        .await
    {
        Ok(view) => view,
        Err(err) => return Ok(Screen::notice(text::user_error(&err)).section(Section::Gifts)),
    };
    let quote = &view.quote;
    let stars = quote
        .stars
        .map(|s| ui::text(c, "gift.stars", &[("stars", Text(&text::stars(s.total)))]))
        .unwrap_or_default();
    let label = quote
        .lines
        .first()
        .map(|line| line.shown_label())
        .unwrap_or_default();
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "gift.quote",
        &[
            ("label", Text(&label)),
            ("total", Text(&text::money(quote.total))),
            ("stars", Html(&stars)),
        ],
    ));
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
                        "gy:{}:{}:{}",
                        request.tariff_id,
                        request.period_id,
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
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.pair(
            ui::back(c, format!("gt:{}", request.tariff_id)),
            ui::menu(c),
        ))
        .section(Section::Gifts))
}

pub async fn pay(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    let Some((request, target)) = parse_pick(rest) else {
        return tariffs(turn, client).await;
    };
    let (provider, method) = crate::buy::split_target(target);
    if !provider.is_empty() {
        let back = crate::buy::back_from(turn, target);
        if let Some(screen) =
            crate::buy::receipt_gate(turn, client, provider, method, &back).await?
        {
            return Ok(screen);
        }
    }
    if provider.is_empty() {
        return summary(turn, client, rest).await;
    }
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let result = services
        .checkout()
        .start_item(
            client.id,
            &ItemCheckoutRequest {
                item: item(request),
                provider: provider.to_string(),
                method: method.map(str::to_string),
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await;
    let result = match result {
        Ok(result) => result,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let what = what(&services, request).await;
    if let CheckoutResult::Redirect { payment_id, .. } = &result
        && provider == providers::GRAM
        && crate::buy::send_gram_widget(turn, *payment_id, &what).await?
    {
        return Ok(
            Screen::new(ui::text(c, "pay.gram_ready", &[("what", Html(&what))]))
                .keyboard(Keyboard::new().push(ui::menu(c)))
                .section(Section::Payment),
        );
    }
    Ok(match result {
        CheckoutResult::Completed { .. } => Screen::new(ui::text(c, "gift.paid_balance", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "gift.btn.mine", &[], "gl"))
                    .push(ui::menu(c)),
            )
            .section(Section::Gifts),
        other => crate::account::payment_screen(c, &other, &what),
    })
}

async fn what(services: &sn_services::Services, request: GiftRequest) -> String {
    let content = services.bot_content_in(lang::current());
    let found = match services.store().conn().await {
        Ok(mut conn) => conn.tariffs().get(request.tariff_id).await.ok(),
        Err(_) => None,
    };
    let (tariff, days) = match found {
        Some(tariff) => {
            let days = tariff
                .period(request.period_id)
                .map(|p| p.days)
                .unwrap_or(0);
            (tariff.name, days)
        }
        None => ("Подписка".to_string(), 0),
    };
    ui::text(
        &content,
        "pay.what_gift",
        &[
            ("tariff", Text(&tariff)),
            ("term", Text(&gifts::term(days.max(1)))),
        ],
    )
}

pub async fn mine(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    if !enabled(turn) {
        return main_menu(turn.ctx, client).await;
    }
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let offset = services.settings().remna_offset();
    let list = gifts::mine(&services.module_context(), client.id).await?;
    let mut lines = Lines::new();
    lines.add(ui::text(c, "gift.mine_title", &[]));
    if list.is_empty() {
        lines.add(ui::text(c, "gift.mine_empty", &[]));
    }
    for entry in list.iter().take(10) {
        let gift = &entry.gift;
        let status = match gift.status {
            GiftStatus::Active => match gift.expires_at {
                Some(at) => ui::text(
                    c,
                    "gift.state_active",
                    &[("date", Text(&text::date(at, offset)))],
                ),
                None => ui::text(c, "gift.state_active_open", &[]),
            },
            GiftStatus::Redeemed => ui::text(
                c,
                "gift.state_redeemed",
                &[(
                    "date",
                    Text(
                        &gift
                            .redeemed_at
                            .map(|at| text::date(at, offset))
                            .unwrap_or_default(),
                    ),
                )],
            ),
            GiftStatus::Expired => ui::text(
                c,
                "gift.state_expired",
                &[(
                    "date",
                    Text(
                        &gift
                            .expires_at
                            .map(|at| text::date(at, offset))
                            .unwrap_or_default(),
                    ),
                )],
            ),
            GiftStatus::Canceled => ui::text(c, "gift.state_canceled", &[]),
        };
        lines.blank();
        lines.add(ui::text(
            c,
            "gift.mine_item",
            &[
                ("code", Text(&gift.code)),
                ("tariff", Text(&gift.tariff_name)),
                ("term", Text(&entry.term)),
                ("status", Html(&status)),
            ],
        ));
        if let Some(link) = &entry.links.bot {
            lines.add(ui::text(c, "gift.mine_link", &[("link", Text(link))]));
        }
    }
    Ok(Screen::new(lines.text())
        .keyboard(Keyboard::new().pair(home_button(c), ui::menu(c)))
        .section(Section::Gifts))
}

pub fn ask_code(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::GiftCode);
    let content = turn.content();
    Screen::new(ui::text(&content, "gift.ask_code", &[]))
        .keyboard(Keyboard::new().push(ui::cancel(&content, "m")))
        .section(Section::Gifts)
}

fn rejection_screen(c: &BotContent, rejection: &GiftRejection, retry: bool) -> Screen {
    let mut keyboard = Keyboard::new();
    if retry {
        keyboard = keyboard.push(ui::button(c, "gift.btn.redeem", &[], "ga"));
    }
    Screen::new(rejection.message())
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Gifts)
}

fn current_name(current: &sn_modules::GrantCurrent) -> String {
    current
        .tariff
        .clone()
        .unwrap_or_else(|| "Подписка".to_string())
}

fn preview_screen(c: &BotContent, preview: &RedeemPreview) -> Screen {
    let gift = &preview.gift;
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "gift.preview",
        &[("tariff", Text(&gift.tariff)), ("term", Text(&gift.term))],
    ));
    let trial = preview
        .current
        .as_ref()
        .is_some_and(|cur| cur.kind == SubscriptionKind::Trial);
    let plan = match (preview.plan, &preview.current) {
        (PlanKind::Change, Some(_)) if trial => {
            ui::text(c, "gift.plan_trial", &[("tariff", Text(&gift.tariff))])
        }
        (PlanKind::Change, Some(current)) => ui::text(
            c,
            "gift.plan_change",
            &[
                ("tariff", Text(&gift.tariff)),
                ("current", Text(&current_name(current))),
            ],
        ),
        (PlanKind::Renewal, Some(current)) => ui::text(
            c,
            "gift.plan_renewal",
            &[("current", Text(&current_name(current)))],
        ),
        _ if !preview.new_allowed => ui::text(c, "gift.plan_limit", &[]),
        _ if preview.multi && !preview.choices.is_empty() => ui::text(c, "gift.plan_separate", &[]),
        _ => ui::text(c, "gift.plan_new", &[]),
    };
    lines.add(plan);
    if gift.own {
        lines.add(ui::text(c, "gift.own", &[]));
    }
    let mut keyboard = Keyboard::new();
    if preview.multi && !preview.choices.is_empty() && preview.current.is_none() {
        lines.add(ui::text(
            c,
            if preview.new_allowed {
                "gift.choose_target"
            } else {
                "gift.choose_existing"
            },
            &[],
        ));
        for choice in &preview.choices {
            keyboard = keyboard.push(ui::button(
                c,
                "gift.btn.extend",
                &[("tariff", Text(&current_name(choice)))],
                format!("gr:{}", choice.subscription_id),
            ));
        }
        if preview.new_allowed {
            keyboard = keyboard.push(ui::button(c, "gift.btn.new", &[], "gr:n"));
        }
    } else {
        keyboard = keyboard.push(ui::button(c, "gift.btn.activate", &[], "gr"));
    }
    Screen::new(lines.text())
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Gifts)
}

pub async fn open_code(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    if !enabled(turn) {
        return main_menu(turn.ctx, client).await;
    }
    let content = turn.content();
    let c = &*content;
    let ctx = turn.services().module_context();
    match gifts::preview(&ctx, client.id, raw, GrantTarget::Auto).await {
        Ok(preview) => {
            turn.session.gift_code = Some(preview.gift.code.clone());
            Ok(preview_screen(c, &preview))
        }
        Err(GiftError::Rejected(rejection)) => {
            let retry = matches!(rejection, GiftRejection::NotFound);
            Ok(rejection_screen(c, &rejection, retry))
        }
        Err(GiftError::Module(err)) => Ok(Screen::notice(text::user_error(
            &sn_services::ServiceError::Module(err),
        ))),
    }
}

pub async fn code_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    open_code(turn, client, raw).await
}

pub async fn redeem(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    if !enabled(turn) {
        return main_menu(turn.ctx, client).await;
    }
    let Some(code) = turn.session.gift_code.take() else {
        return Ok(ask_code(turn));
    };
    let target = match rest {
        "n" => GrantTarget::New,
        other => match id_arg(other) {
            Some(id) => GrantTarget::Subscription(SubscriptionId(id)),
            None => GrantTarget::Auto,
        },
    };
    let content = turn.content();
    let c = &*content;
    let ctx = turn.services().module_context();
    match gifts::redeem(&ctx, client.id, &code, target).await {
        Ok(done) => Ok(Screen::new(ui::text(
            c,
            "gift.redeemed",
            &[
                ("tariff", Text(&done.gift.tariff)),
                ("term", Text(&done.gift.term)),
            ],
        ))
        .keyboard(
            Keyboard::new()
                .push(ui::button(c, "common.btn.subscription", &[], "subs"))
                .push(ui::menu(c)),
        )
        .section(Section::Gifts)),
        Err(GiftError::Rejected(rejection)) => Ok(rejection_screen(c, &rejection, false)),
        Err(GiftError::Module(err)) => Ok(Screen::notice(text::user_error(
            &sn_services::ServiceError::Module(err),
        ))),
    }
}

fn link_rejection(c: &BotContent, rejection: &LinkRejection) -> String {
    let key = match rejection {
        LinkRejection::NotFound => "pl.not_found",
        LinkRejection::Disabled => "pl.disabled",
        LinkRejection::Expired => "pl.expired",
        LinkRejection::Exhausted => "pl.exhausted",
        LinkRejection::AlreadyUsed => "pl.already_used",
        LinkRejection::Confirm => "pl.again",
        LinkRejection::Denied => "pl.denied",
        LinkRejection::Grant(refusal) if refusal.code == "grant.has_subscription" => {
            "pl.has_subscription"
        }
        LinkRejection::Grant(refusal) => return refusal.message.clone(),
    };
    ui::text(c, key, &[])
}

pub async fn promo_link(
    turn: &mut Turn<'_>,
    client: &Client,
    code: &str,
) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Promolinks) {
        return main_menu(turn.ctx, client).await;
    }
    let ctx = turn.services().module_context();
    let outcome = promolinks::activate(&ctx, client.id, code).await;
    promo_link_outcome(turn, client, code, outcome).await
}

pub async fn promo_link_confirmed(
    turn: &mut Turn<'_>,
    client: &Client,
    token: &str,
) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Promolinks) {
        return main_menu(turn.ctx, client).await;
    }
    let Some(code) = turn.session.promo_link.take() else {
        let content = turn.content();
        return Ok(Screen::new(ui::text(&content, "pl.again", &[]))
            .keyboard(Keyboard::new().push(ui::menu(&content)))
            .section(Section::PromoLinks));
    };
    let ctx = turn.services().module_context();
    let outcome = promolinks::activate_confirmed(&ctx, client.id, &code, token).await;
    promo_link_outcome(turn, client, &code, outcome).await
}

async fn promo_link_check(
    turn: &mut Turn<'_>,
    client: &Client,
    code: &str,
) -> Result<Screen, BotError> {
    let token = antibot::challenge(
        &turn.services().module_context(),
        client.id,
        antibot::PROMO_LINK,
        antibot::Surface::Bot,
    )
    .await?
    .token;
    turn.session.promo_link = Some(code.to_string());
    let content = turn.content();
    let c = &*content;
    Ok(Screen::new(ui::text(c, "pl.confirm", &[]))
        .keyboard(
            Keyboard::new()
                .push(ui::button(
                    c,
                    "pl.btn.confirm",
                    &[],
                    format!("{PROMO_CONFIRM}:{token}"),
                ))
                .push(ui::menu(c)),
        )
        .section(Section::PromoLinks))
}

async fn promo_link_outcome(
    turn: &mut Turn<'_>,
    client: &Client,
    code: &str,
    outcome: Result<Activated, LinkError>,
) -> Result<Screen, BotError> {
    if matches!(outcome, Err(LinkError::Rejected(LinkRejection::Confirm))) {
        return promo_link_check(turn, client, code).await;
    }
    turn.session.promo_link = None;
    let content = turn.content();
    let c = &*content;
    match outcome {
        Ok(done) => Ok(Screen::new(ui::text(
            c,
            "pl.done",
            &[("days", Text(&text::days(done.days)))],
        ))
        .keyboard(
            Keyboard::new()
                .push(ui::button(c, "common.btn.subscription", &[], "subs"))
                .push(ui::menu(c)),
        )
        .section(Section::PromoLinks)),
        Err(LinkError::Rejected(rejection)) => Ok(Screen::new(link_rejection(c, &rejection))
            .keyboard(Keyboard::new().push(ui::menu(c)))
            .section(Section::PromoLinks)),
        Err(LinkError::Module(err)) => Ok(Screen::notice(text::user_error(
            &sn_services::ServiceError::Module(err),
        ))),
    }
}
