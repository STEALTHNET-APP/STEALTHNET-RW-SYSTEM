use crate::session::{Awaiting, Session};
use crate::stars::{self, Settled};
use crate::telegram::{
    BotUser, Button, CallbackQuery, Keyboard, Message, PreCheckoutQuery, TelegramApi, Update,
};
use crate::text::{self, escape};
use crate::ui::{self, Html, Lines, Text};
use crate::{
    BotError, account, buy, consent, contests, email, extras, gifts, gram_wallet, link, login,
    media,
};
use sn_domain::bot_content::Section;
use sn_domain::bot_layout::{Screen as LayoutScreen, arrange, pack};
use sn_domain::client::{
    Client, IdentityKind, Locale, RegistrationSource, TelegramProfile, proven_identities,
};
use sn_domain::ids::{SubscriptionId, TariffId};
use sn_domain::lang;
use sn_domain::ledger::Wallet;
use sn_domain::money::Money;
use sn_domain::settings::CoreSettings;
use sn_domain::subscription::Subscription;
use sn_modules::catalog::antibot::signups::Refusal;
use sn_modules::catalog::utm::clicks::ClickChannel;
use sn_modules::catalog::{
    antibot, broadcast, gifts as gift_module, options, promo, promolinks, referral,
};
use sn_payments::providers::{StarsPayment, StarsPaymentEvent, StarsPreCheckout};
use sn_services::Services;
use sn_services::bot_content::{BotContent, Document};
use sn_services::bot_flood::Signup;
use sn_services::bot_layout::{Install, link_button};
use sn_services::clients::Entered;
use sn_services::subscriptions::SubscriptionView;
use sn_store::Db;
use std::sync::Arc;

#[derive(Clone)]
pub struct BotCtx {
    pub services: Services,
    pub api: Arc<TelegramApi>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Screen {
    pub text: String,
    pub keyboard: Keyboard,
    pub section: Option<Section>,
    pub menu: bool,
    pub spare: bool,
}

impl Screen {
    pub fn new(text: impl Into<String>) -> Self {
        Screen {
            text: text.into(),
            keyboard: Keyboard::new(),
            section: None,
            menu: false,
            spare: false,
        }
    }

    pub fn keyboard(mut self, keyboard: Keyboard) -> Self {
        self.keyboard = keyboard;
        self
    }

    pub fn spare(text: impl Into<String>) -> Self {
        Screen {
            spare: true,
            ..Screen::new(text)
        }
    }

    pub fn notice(text: impl Into<String>) -> Self {
        Screen::new(text).with_menu()
    }

    pub fn with_menu(mut self) -> Self {
        self.menu = true;
        self
    }

    pub fn section(mut self, section: Section) -> Self {
        self.section = Some(section);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub chat_id: i64,
    pub message_id: Option<i64>,
    pub photo: bool,
}

impl BotCtx {
    pub async fn show(&self, target: Target, screen: &Screen) -> Result<(), BotError> {
        media::show(self, target, screen).await
    }
}

pub fn profile(user: &BotUser) -> TelegramProfile {
    TelegramProfile {
        id: user.id,
        username: user.username.clone(),
        first_name: Some(user.first_name.clone()).filter(|n| !n.trim().is_empty()),
        last_name: user.last_name.clone(),
        language_code: user.language_code.clone(),
        is_premium: user.is_premium.unwrap_or(false),
    }
}

pub struct Turn<'a> {
    pub ctx: &'a BotCtx,
    pub user: &'a BotUser,
    pub target: Target,
    pub session: Session,
    pub locale: Locale,
    pub secret_left: bool,
    pub repeat: bool,
}

impl Turn<'_> {
    pub fn services(&self) -> &Services {
        &self.ctx.services
    }

    pub fn content(&self) -> Arc<BotContent> {
        self.ctx.services.bot_content_in(self.locale)
    }
}

pub async fn user_locale(services: &Services, user: &BotUser) -> Result<Locale, BotError> {
    let languages = services.settings().languages;
    if !languages.several() {
        return Ok(Locale::Ru);
    }
    let mut conn = services.store().conn().await?;
    let known = conn
        .clients()
        .find_by_identity(IdentityKind::Telegram, &user.id.to_string())
        .await?;
    Ok(match known {
        Some(client) => languages.effective(client.locale),
        None => languages.detect(user.language_code.as_deref()),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Start(Option<String>),
    Menu,
    Language,
    PaySupport,
    Input(Awaiting, String),
    Text(String),
    Callback(String),
    File(crate::support_files::IncomingFile),
}

enum Gate {
    Ready(Client),
    Stop(Screen),
}

pub fn parse_command(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix('/')?;
    let (head, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let command = head.split('@').next().unwrap_or(head).to_ascii_lowercase();
    if command.is_empty() {
        return None;
    }
    Some((command, args.trim().to_string()))
}

pub async fn handle_update(ctx: &BotCtx, update: Update) {
    let id = update.update_id;
    if let Err(err) = Box::pin(dispatch(ctx, update)).await {
        tracing::warn!(update_id = id, error = %err, "апдейт бота не обработан");
    }
}

async fn dispatch(ctx: &BotCtx, update: Update) -> Result<(), BotError> {
    let repeat = update.repeat;
    if let Some(query) = update.widget_query {
        return gram_wallet::on_query(ctx, query).await;
    }
    if let Some(query) = update.pre_checkout_query {
        return pre_checkout(ctx, &query).await;
    }
    if let Some(callback) = update.callback_query {
        return on_callback(ctx, callback).await;
    }
    let Some(message) = update.message else {
        return Ok(());
    };
    if message.chat.is_group() {
        if let Some((command, args)) = message.text.as_deref().and_then(parse_command)
            && command == "start"
        {
            return crate::alerts::bind_from_group(&ctx.services, &ctx.api, &message, &args).await;
        }
        return Ok(());
    }
    if !message.chat.is_private() {
        return Ok(());
    }
    let Some(user) = message.from.clone().filter(|u| !u.is_bot) else {
        return Ok(());
    };
    let target = Target {
        chat_id: message.chat.id,
        message_id: None,
        photo: false,
    };
    if let Some(event) = stars::from_message(&message) {
        return match event.event {
            StarsPaymentEvent::SuccessfulPayment => stars_paid(ctx, target, &event).await,
            StarsPaymentEvent::RefundedPayment => {
                stars_refunded(ctx, &event).await;
                Ok(())
            }
        };
    }
    if crate::maintenance::closed_for(ctx, &user).await? {
        return crate::maintenance::notify(ctx, &user, target.chat_id).await;
    }
    if let Some(file) = crate::support_files::incoming(&message) {
        let session = Session::load(&ctx.services, target.chat_id).await?;
        return run(ctx, &user, target, session, Action::File(file)).await;
    }
    let Some(text) = message.text.as_deref() else {
        return Ok(());
    };
    if parse_command(text).is_some_and(|(command, _)| command == "emoji") {
        return ctx.show(target, &emoji_codes(&message)).await;
    }
    let session = Session::load(&ctx.services, target.chat_id).await?;
    let command = parse_command(text);
    let secret = command.is_none() && session.awaiting.is_some_and(Awaiting::secret);
    let secret_left = secret && !forget(ctx, target.chat_id, message.message_id).await;
    let action = match command {
        Some((command, args)) => match command.as_str() {
            "start" => Action::Start(Some(args).filter(|a| !a.is_empty())),
            "paysupport" => Action::PaySupport,
            "language" | "lang" => Action::Language,
            _ => Action::Menu,
        },
        None => match session.awaiting {
            Some(awaiting) if awaiting.secret() => Action::Input(awaiting, text.to_string()),
            Some(awaiting) => Action::Input(awaiting, text.trim().to_string()),
            None => Action::Text(text.trim().to_string()),
        },
    };
    run_with(ctx, &user, target, session, action, (secret_left, repeat)).await
}

async fn forget(ctx: &BotCtx, chat_id: i64, message_id: i64) -> bool {
    match ctx.api.delete_message(chat_id, message_id).await {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!(error = %err, "сообщение с паролем или кодом не удалилось из чата");
            false
        }
    }
}

async fn on_callback(ctx: &BotCtx, callback: CallbackQuery) -> Result<(), BotError> {
    let private = callback
        .message
        .as_ref()
        .is_some_and(|m| m.chat.is_private());
    if private && crate::maintenance::closed_for(ctx, &callback.from).await? {
        let text = crate::maintenance::text_for(ctx, &callback.from).await?;
        if let Err(err) = ctx.api.answer_alert(&callback.id, &text).await {
            tracing::debug!(error = %err, "окно о технических работах не показано");
        }
        return Ok(());
    }
    if let Err(err) = ctx.api.answer_callback(&callback.id, None).await {
        tracing::debug!(error = %err, "ответ на нажатие кнопки не отправлен");
    }
    let Some(message) = callback.message.as_ref().filter(|m| m.chat.is_private()) else {
        return Ok(());
    };
    let data = callback.data.clone().unwrap_or_default();
    let (target, data) = match data.strip_prefix(broadcast::CALLBACK_PREFIX) {
        Some(rest) => (
            Target {
                chat_id: message.chat.id,
                message_id: None,
                photo: false,
            },
            rest.to_string(),
        ),
        None => (
            Target {
                chat_id: message.chat.id,
                message_id: Some(message.message_id),
                photo: message.has_photo(),
            },
            data,
        ),
    };
    let session = Session::load(&ctx.services, target.chat_id).await?;
    run(ctx, &callback.from, target, session, Action::Callback(data)).await
}

async fn run(
    ctx: &BotCtx,
    user: &BotUser,
    target: Target,
    session: Session,
    action: Action,
) -> Result<(), BotError> {
    run_with(ctx, user, target, session, action, (false, false)).await
}

async fn run_with(
    ctx: &BotCtx,
    user: &BotUser,
    target: Target,
    session: Session,
    action: Action,
    (secret_left, repeat): (bool, bool),
) -> Result<(), BotError> {
    let locale = user_locale(&ctx.services, user).await?;
    let mut turn = Turn {
        ctx,
        user,
        target,
        session,
        locale,
        secret_left,
        repeat,
    };
    let screen = lang::scope(locale, process(&mut turn, action)).await?;
    if screen.spare {
        if let Err(err) = ctx.api.send_spare(target.chat_id, &screen.text).await {
            tracing::debug!(error = %err, "ответ при наплыве не отправлен");
        }
        return Ok(());
    }
    if !screen.text.is_empty() {
        lang::scope(locale, ctx.show(target, &screen)).await?;
    }
    turn.session.save(&ctx.services, target.chat_id).await?;
    Ok(())
}

async fn process(turn: &mut Turn<'_>, action: Action) -> Result<Screen, BotError> {
    let (action, replayed) = match action {
        Action::PaySupport => return Ok(paysupport(turn.ctx)),
        Action::Callback(data) if data == "terms:ok" => {
            accept_documents(turn).await?;
            (Action::Start(turn.session.pending_start.clone()), true)
        }
        other => (other, false),
    };
    match &action {
        Action::Start(Some(param)) if link::opens(param) => {
            return link::opened(turn, param).await;
        }
        Action::Callback(data) => {
            if let Some(rest) = link::decision(data) {
                return link::decided(turn, rest).await;
            }
        }
        _ => {}
    }
    let start_param = match &action {
        Action::Start(param) => param.clone(),
        _ => None,
    };
    if !replayed && !turn.repeat {
        turn.services()
            .count_utm_click(ClickChannel::Bot, start_param.as_deref())
            .await;
    }
    let client = match enter(turn, start_param.as_deref()).await? {
        Gate::Ready(client) => client,
        Gate::Stop(screen) => return Ok(screen),
    };
    match action {
        Action::Start(param) => after_start(turn, &client, param.as_deref()).await,
        Action::Menu | Action::PaySupport => {
            turn.session.awaiting = None;
            main_menu(turn.ctx, &client).await
        }
        Action::Language => {
            turn.session.awaiting = None;
            language_screen(turn, &client).await
        }
        Action::Input(awaiting, text) => input(turn, &client, awaiting, &text).await,
        Action::Text(text) => match account::support_followup(turn, &client, &text).await? {
            Some(screen) => Ok(screen),
            None => main_menu(turn.ctx, &client).await,
        },
        Action::Callback(data) => callback(turn, &client, &data).await,
        Action::File(file) => match crate::support_files::receive(turn, &client, file).await? {
            Some(screen) => Ok(screen),
            None => main_menu(turn.ctx, &client).await,
        },
    }
}

pub fn emoji_codes(message: &Message) -> Screen {
    let mut found: Vec<(String, String)> = Vec::new();
    for (fallback, id) in message.custom_emoji() {
        if !found.iter().any(|(_, known)| *known == id) {
            found.push((fallback, id));
        }
    }
    if found.is_empty() {
        return Screen::notice(
            "Пришлите /emoji вместе с премиум-эмодзи в одном сообщении, можно целой фразой. Бот ответит строкой для текста и кодами для кнопок.",
        );
    }
    let mut out = vec![
        "<b>Для текста</b> — скопируйте строку целиком и вставьте в текст:".to_string(),
        format!("<code>{}</code>", escape(&emoji_line(message))),
        String::new(),
        "<b>Для значка кнопки</b> — код в поле «Премиум-эмодзи» у кнопки:".to_string(),
    ];
    for (fallback, id) in &found {
        out.push(format!(
            "{} — <code>{}</code>",
            escape(fallback),
            escape(id)
        ));
    }
    Screen::notice(out.join("\n"))
}

pub fn emoji_line(message: &Message) -> String {
    let text = message.text.as_deref().unwrap_or_default();
    let units: Vec<u16> = text.encode_utf16().collect();
    let skip = units
        .iter()
        .position(|u| *u == u16::from(b' ') || *u == u16::from(b'\n'))
        .unwrap_or(units.len());
    let mut marks: Vec<(usize, usize, &str)> = message
        .entities
        .iter()
        .flatten()
        .filter(|e| e.kind == "custom_emoji")
        .filter_map(|e| {
            let id = e.custom_emoji_id.as_deref()?;
            let start = usize::try_from(e.offset).ok()?;
            let end = start.checked_add(usize::try_from(e.length).ok()?)?;
            (start >= skip && end <= units.len()).then_some((start, end, id))
        })
        .collect();
    marks.sort_by_key(|mark| mark.0);
    let piece = |from: usize, to: usize| String::from_utf16_lossy(&units[from..to]);
    let mut out = String::new();
    let mut at = skip;
    for (start, end, id) in marks {
        if start < at {
            continue;
        }
        out.push_str(&escape(&piece(at, start)));
        out.push_str(&format!(
            "<tg-emoji emoji-id=\"{}\">{}</tg-emoji>",
            escape(id),
            escape(&piece(start, end))
        ));
        at = end;
    }
    out.push_str(&escape(&piece(at, units.len())));
    out.trim().to_string()
}

async fn enter(turn: &mut Turn<'_>, start_param: Option<&str>) -> Result<Gate, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content_in(lang::current());
    {
        let documents = content.consent_documents();
        let mut conn = services.store().conn().await?;
        if let Some(version) = consent::version(documents)
            && !consent::accepted(&mut conn, turn.user.id, &version).await?
        {
            if let Some(param) = start_param {
                turn.session.pending_start = Some(param.to_string());
            }
            return Ok(Gate::Stop(consent_screen(
                &content,
                services.settings().public_url.as_deref(),
            )));
        }
    }
    let param = start_param
        .map(str::to_string)
        .or_else(|| turn.session.pending_start.take());
    turn.session.pending_start = None;
    let flood = services.settings().bot_flood;
    let mut signup = None;
    let visit = match param.as_deref().and_then(crate::login::token_from_param) {
        Some(code) => services.auth().telegram_login_visit(code).await?,
        None => None,
    };
    let entered = services
        .clients()
        .ensure_telegram_from(
            &profile(turn.user),
            RegistrationSource::Bot,
            param.as_deref(),
            visit.as_ref(),
            || {
                let verdict = services
                    .bot_flood()
                    .signup(&flood, std::time::Instant::now());
                signup = Some(verdict);
                verdict != Signup::Refused
            },
        )
        .await?;
    let (client, created) = match entered {
        Entered::Ready(client, created) => (client, created),
        Entered::Busy => {
            return Ok(Gate::Stop(Screen::spare(ui::text(
                &content,
                "common.busy",
                &[],
            ))));
        }
        Entered::Refused(refusal) => {
            let key = match refusal {
                Refusal::Minute => "login.busy",
                Refusal::Address | Refusal::Network => "login.limit",
            };
            return Ok(Gate::Stop(Screen::spare(ui::text(&content, key, &[]))));
        }
    };
    if created
        && let Some(Signup::Allowed {
            surge: true,
            per_minute,
        }) = signup
    {
        match antibot::note_surge(&services.module_context(), client.id, per_minute).await {
            Ok(true) => tracing::info!(
                client = client.id.0,
                per_minute,
                "новый клиент пришёл во время наплыва и отмечен антиботом"
            ),
            Ok(false) => {}
            Err(err) => tracing::warn!(error = %err, "отметка наплыва не сохранена"),
        }
    }
    if client.is_blocked() {
        turn.session = Session::default();
        return Ok(Gate::Stop(Screen::new(ui::text(
            &content,
            "common.blocked",
            &[],
        ))));
    }
    let mut conn = services.store().conn().await?;
    if client.bot_unreachable {
        conn.clients().set_bot_unreachable(client.id, false).await?;
    }
    if let Err(err) = conn
        .client_photos()
        .request_if_stale(client.id, crate::photos::REFRESH_AFTER)
        .await
    {
        tracing::debug!(error = %err, "запрос фото клиента не сохранён");
    }
    Ok(Gate::Ready(client))
}

fn document_lines(
    content: &BotContent,
    documents: &[Document],
    public_url: Option<&str>,
    lines: &mut Lines,
) {
    let locale = content.locale();
    for document in documents {
        let Some(url) = document.absolute(public_url) else {
            continue;
        };
        lines.add(ui::text(
            content,
            "docs.item",
            &[
                ("title", Text(document.title_in(locale))),
                ("url", Text(&url)),
            ],
        ));
    }
}

fn consent_screen(content: &BotContent, public_url: Option<&str>) -> Screen {
    let mut lines = Lines::new();
    lines.add(ui::text(content, "docs.intro", &[]));
    document_lines(content, content.consent_documents(), public_url, &mut lines);
    lines.add(ui::text(content, "docs.agree", &[]));
    Screen::new(lines.text())
        .keyboard(Keyboard::new().push(ui::button(content, "docs.btn.accept", &[], "terms:ok")))
        .section(Section::Documents)
}

pub fn documents_screen(content: &BotContent, public_url: Option<&str>) -> Screen {
    let mut lines = Lines::new();
    lines.add(ui::text(content, "docs.list_title", &[]));
    document_lines(content, &content.documents, public_url, &mut lines);
    Screen::new(lines.text())
        .with_menu()
        .section(Section::Documents)
}

async fn accept_documents(turn: &mut Turn<'_>) -> Result<(), BotError> {
    let content = turn.content();
    let documents = content.consent_documents();
    if let Some(version) = consent::version(documents) {
        let mut conn = turn.services().store().conn().await?;
        consent::accept(&mut conn, turn.user.id, &version, documents).await?;
    }
    Ok(())
}

async fn after_start(
    turn: &mut Turn<'_>,
    client: &Client,
    param: Option<&str>,
) -> Result<Screen, BotError> {
    turn.session.awaiting = None;
    match param {
        Some(param) if param.starts_with(login::PREFIX) => login_screen(turn, client, param).await,
        Some(SUPPORT_START) => Ok(account::support_prompt(turn)),
        Some(param) => {
            if let Some(code) = gift_module::start_code(param) {
                return gifts::open_code(turn, client, &code).await;
            }
            if let Some(code) = promolinks::start_code(param) {
                return gifts::promo_link(turn, client, &code).await;
            }
            main_menu(turn.ctx, client).await
        }
        None => main_menu(turn.ctx, client).await,
    }
}

async fn input(
    turn: &mut Turn<'_>,
    client: &Client,
    awaiting: Awaiting,
    text: &str,
) -> Result<Screen, BotError> {
    turn.session.awaiting = None;
    match awaiting {
        Awaiting::PromoCode => extras::promo_entered(turn, client, text).await,
        Awaiting::PromoForCart => buy::promo_entered(turn, client, text).await,
        Awaiting::TopUpAmount if !turn.services().payments().balance_enabled() => {
            account::balance(turn, client).await
        }
        Awaiting::TopUpAmount => account::topup_entered(turn, text).await,
        Awaiting::SupportMessage => account::support_entered(turn, client, text).await,
        Awaiting::WithdrawalAmount => extras::withdrawal_amount(turn, client, text).await,
        Awaiting::WithdrawalDetails => extras::withdrawal_details(turn, client, text).await,
        Awaiting::GiftCode => gifts::code_entered(turn, client, text).await,
        Awaiting::EmailAddress => email::address_entered(turn, client, text).await,
        Awaiting::EmailProof => email::proof_entered(turn, client, text).await,
        Awaiting::EmailCode => email::code_entered(turn, client, text).await,
        Awaiting::SitePassword => email::password_entered(turn, client, text).await,
    }
}

pub fn id_arg(rest: &str) -> Option<i64> {
    rest.parse::<i64>().ok().filter(|id| *id > 0)
}

async fn callback(turn: &mut Turn<'_>, client: &Client, data: &str) -> Result<Screen, BotError> {
    let (head, rest) = data.split_once(':').unwrap_or((data, ""));
    turn.session.awaiting = None;
    let screen = match head {
        "subs" => account::subscriptions(turn, client).await?,
        "sub" => match id_arg(rest) {
            Some(id) => account::subscription(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "dev" => match id_arg(rest) {
            Some(id) => account::devices(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "dx" => account::device_confirm(turn, rest).await?,
        "dxy" => account::device_disconnect(turn, client, rest).await?,
        "rei" => match id_arg(rest) {
            Some(id) => account::reissue_confirm(&turn.content(), id),
            None => main_menu(turn.ctx, client).await?,
        },
        "reiy" => match id_arg(rest) {
            Some(id) => account::reissue(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "rno" => match id_arg(rest) {
            Some(id) => account::renewal_confirm(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "rnoy" => match id_arg(rest) {
            Some(id) => account::renewal_off(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "bal" => account::balance(turn, client).await?,
        "top" | "topx" | "tp" if !turn.services().payments().balance_enabled() => {
            account::balance(turn, client).await?
        }
        "top" => match rest.parse::<i64>() {
            Ok(minor) => account::topup_amount(turn, minor).await?,
            Err(_) => account::balance(turn, client).await?,
        },
        "topx" => account::topup_custom(turn),
        "tp" => account::topup_start(turn, client, rest).await?,
        "sup" => match id_arg(rest) {
            Some(id) => account::support_reply_prompt(turn, client, id).await?,
            None => account::support_prompt(turn),
        },
        "trial" => match id_arg(rest).and_then(|id| u32::try_from(id).ok()) {
            Some(id) => account::trial_variant(turn, client, id).await?,
            None => account::trial(turn, client).await?,
        },
        "ab" => account::trial_confirmed(turn, client, rest).await?,
        "buy" => buy::start(turn, client, rest).await?,
        "t" => match id_arg(rest) {
            Some(id) => buy::tariff(turn, client, TariffId(id)).await?,
            None => buy::start(turn, client, "").await?,
        },
        "tb" => match rest
            .split_once(':')
            .and_then(|(tariff, sub)| Some((id_arg(tariff)?, id_arg(sub)?)))
        {
            Some((tariff, sub)) => {
                buy::tariff_for(turn, client, TariffId(tariff), SubscriptionId(sub)).await?
            }
            None => buy::start(turn, client, "").await?,
        },
        "p" => match id_arg(rest) {
            Some(id) => buy::period(turn, client, id).await?,
            None => buy::start(turn, client, "").await?,
        },
        "d" => match rest.parse::<u32>() {
            Ok(count) => buy::devices(turn, client, count).await?,
            Err(_) => buy::summary(turn, client).await?,
        },
        "dv" => buy::change_devices(turn, client).await?,
        "sum" => buy::summary(turn, client).await?,
        "promo" => buy::ask_promo(turn),
        "nopromo" => {
            turn.session.promo = None;
            buy::summary(turn, client).await?
        }
        "pay" => buy::pay(turn, client, rest).await?,
        "x" => extras::entry(turn, client, rest).await?,
        "pc" => match id_arg(rest) {
            Some(id) => extras::activate_days(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "ref" => extras::referral_action(turn, client, rest).await?,
        contests::OPEN => contests::open(turn, client, rest).await?,
        contests::JOIN => contests::join(turn, client, rest).await?,
        contests::CONFIRM => contests::confirmed(turn, client, rest).await?,
        "opt" => extras::options_action(turn, client, rest).await?,
        "op" => extras::options_pay(turn, client, rest).await?,
        "gb" => gifts::tariffs(turn, client).await?,
        "gt" => match id_arg(rest) {
            Some(id) => gifts::periods(turn, client, TariffId(id)).await?,
            None => gifts::tariffs(turn, client).await?,
        },
        "gp" => gifts::summary(turn, client, rest).await?,
        "gy" => gifts::pay(turn, client, rest).await?,
        "gl" => gifts::mine(turn, client).await?,
        "ga" => gifts::ask_code(turn),
        "gr" => gifts::redeem(turn, client, rest).await?,
        gifts::PROMO_CONFIRM => gifts::promo_link_confirmed(turn, client, rest).await?,
        "auth" => login_decision(turn, rest).await?,
        email::CALLBACK => email::action(turn, client, rest).await?,
        "docs" => documents_screen(
            &turn.content(),
            turn.services().settings().public_url.as_deref(),
        ),
        "lang" if rest.is_empty() => language_screen(turn, client).await?,
        "lang" => set_language(turn, client, rest).await?,
        _ => main_menu(turn.ctx, client).await?,
    };
    Ok(screen)
}

async fn language_screen(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let languages = turn.services().settings().languages;
    if !languages.several() {
        return main_menu(turn.ctx, client).await;
    }
    let content = turn.content();
    let buttons = languages
        .enabled()
        .into_iter()
        .map(|locale| {
            let name = locale.native_name();
            let label = if locale == turn.locale {
                format!("✓ {name}")
            } else {
                name.to_string()
            };
            Button::callback(label, format!("lang:{}", locale.as_str()))
        })
        .collect();
    Ok(Screen::new(ui::text(&content, "lang.choose", &[]))
        .keyboard(Keyboard::new().grid(buttons, 2).push(ui::menu(&content))))
}

async fn set_language(turn: &mut Turn<'_>, client: &Client, raw: &str) -> Result<Screen, BotError> {
    let Ok(wanted) = Locale::parse(raw) else {
        return main_menu(turn.ctx, client).await;
    };
    let services = turn.services().clone();
    let locale = match services.clients().set_locale(client.id, wanted).await {
        Ok(locale) => locale,
        Err(_) => return language_screen(turn, client).await,
    };
    turn.locale = locale;
    let mut updated = client.clone();
    updated.locale = locale;
    lang::scope(locale, main_menu(turn.ctx, &updated)).await
}

pub async fn tariff_name(services: &Services, tariff_id: Option<TariffId>) -> String {
    let fallback = || lang::pick("Подписка", "Subscription").to_string();
    let Some(id) = tariff_id else {
        return fallback();
    };
    let found = match services.store().conn().await {
        Ok(mut conn) => conn.tariffs().get(id).await.ok(),
        Err(_) => None,
    };
    found.map_or_else(fallback, |t| t.name)
}

pub async fn subscription_names(services: &Services, subs: &[Subscription]) -> Vec<String> {
    let mut bases = Vec::with_capacity(subs.len());
    for sub in subs {
        bases.push(tariff_name(services, sub.tariff_id).await);
    }
    distinct_names(&bases, subs)
}

pub fn distinct_names(bases: &[String], subs: &[Subscription]) -> Vec<String> {
    bases
        .iter()
        .zip(subs)
        .enumerate()
        .map(|(index, (base, sub))| {
            if bases.iter().filter(|other| *other == base).count() < 2 {
                return base.clone();
            }
            match sub
                .remna
                .as_ref()
                .map(|remna| remna.username.as_str())
                .filter(|nick| !nick.is_empty() && !legacy_nick(nick))
            {
                Some(nick) => format!("{base} · {nick}"),
                None => format!("{} {}", lang::pick("Подписка", "Subscription"), index + 1),
            }
        })
        .collect()
}

fn legacy_nick(nick: &str) -> bool {
    nick.strip_prefix("sn_").is_some_and(|rest| {
        let bytes = rest.as_bytes();
        bytes.len() > 8 && bytes[8] == b'-' && bytes[..8].iter().all(u8::is_ascii_hexdigit)
    })
}

pub fn live(views: Vec<SubscriptionView>) -> Vec<Subscription> {
    views
        .into_iter()
        .map(|v| v.subscription)
        .filter(|s| s.ended_at.is_none())
        .collect()
}

async fn trial_available(services: &Services, client: &Client) -> Result<bool, BotError> {
    if !services.settings().trial.is_offered() {
        return Ok(false);
    }
    let mut conn = services.store().conn().await?;
    let identities = proven_identities(conn.clients().identities(client.id).await?);
    Ok(!conn.trials().was_claimed(client.id, &identities).await?)
}

fn menu_button(c: &BotContent, install: &Install, key: &'static str) -> Option<Button> {
    if let Some(url) = install.app_url(key) {
        return ui::web_app(c, key, url);
    }
    let entry = |name: &str| format!("x:{name}");
    match key {
        "menu.btn.app" | "menu.btn.roulette" => None,
        "menu.btn.subscription" => ui::button(c, key, &[], "subs"),
        "menu.btn.trial" => ui::button(c, key, &[], "trial"),
        "menu.btn.buy" | "menu.btn.extend" => ui::button(c, key, &[], "buy"),
        "menu.btn.balance" => ui::button(c, key, &[], "bal"),
        "menu.btn.promo" => ui::button(c, key, &[], entry(promo::BOT_ENTRY)),
        "menu.btn.options" => ui::button(c, key, &[], entry(options::BOT_ENTRY)),
        "menu.btn.gifts" => ui::button(c, key, &[], entry(gift_module::BOT_ENTRY)),
        "menu.btn.referral" => ui::button(c, key, &[], entry(referral::BOT_ENTRY)),
        "menu.btn.contests" => {
            ui::button(c, key, &[], entry(sn_modules::catalog::contests::BOT_ENTRY))
        }
        "menu.btn.documents" => ui::button(c, key, &[], "docs"),
        "menu.btn.support" => match &install.support {
            Some(url) => ui::link(c, key, &[], url.clone()),
            None => ui::button(c, key, &[], "sup"),
        },
        "menu.btn.email" => ui::button(c, key, &[], email::CALLBACK),
        "menu.btn.language" => ui::button(c, key, &[], "lang"),
        _ => None,
    }
}

async fn first_name(services: &Services, client: &Client) -> String {
    let Ok(mut conn) = services.store().conn().await else {
        return String::new();
    };
    conn.clients()
        .identities(client.id)
        .await
        .ok()
        .and_then(|list| {
            list.into_iter()
                .find(|i| i.kind == IdentityKind::Telegram)
                .and_then(|i| {
                    i.meta
                        .get("first_name")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
        })
        .unwrap_or_default()
}

pub fn status_text(
    content: &BotContent,
    subscription: &Subscription,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    ui::text(content, text::status_key(subscription, now), &[])
}

async fn balance_of(services: &Services, client: &Client) -> Result<Money, BotError> {
    let mut conn = services.store().conn().await?;
    Ok(conn
        .ledger()
        .balance(client.id, Wallet::Main, services.settings().currency)
        .await?)
}

async fn has_balance(services: &Services, client: &Client) -> Result<bool, BotError> {
    Ok(balance_of(services, client).await?.minor > 0)
}

fn single_lines(
    c: &BotContent,
    subscription: &Subscription,
    tariff: &str,
    now: chrono::DateTime<chrono::Utc>,
    offset: chrono::FixedOffset,
) -> Vec<String> {
    let status = status_text(c, subscription, now);
    let mirror = &subscription.mirror;
    let mut lines = vec![ui::text(
        c,
        "menu.single",
        &[("tariff", Text(tariff)), ("status", Html(&status))],
    )];
    match mirror.expire_at {
        Some(at) if at > now => lines.push(ui::text(
            c,
            "menu.single_until",
            &[
                ("date", Text(&text::date(at, offset))),
                ("days", Text(&text::plural_days(text::days_left(at, now)))),
            ],
        )),
        Some(at) => lines.push(ui::text(
            c,
            "menu.single_ended",
            &[("date", Text(&text::date(at, offset)))],
        )),
        None => {}
    }
    lines.push(
        match mirror.traffic_limit_bytes.filter(|limit| *limit > 0) {
            Some(limit) => ui::text(
                c,
                "menu.single_traffic",
                &[
                    (
                        "used",
                        Text(&text::traffic(mirror.traffic_used_bytes.unwrap_or(0))),
                    ),
                    ("limit", Text(&text::traffic(limit))),
                ],
            ),
            None => ui::text(c, "menu.single_traffic_unlimited", &[]),
        },
    );
    match (mirror.devices_count, mirror.hwid_limit) {
        (Some(count), Some(limit)) => lines.push(ui::text(
            c,
            "menu.single_devices",
            &[
                ("count", Text(&count.to_string())),
                ("limit", Text(&limit.to_string())),
            ],
        )),
        (None, Some(limit)) => lines.push(ui::text(
            c,
            "menu.single_devices_limit",
            &[("limit", Text(&limit.to_string()))],
        )),
        _ => {}
    }
    lines
}

pub async fn main_menu(ctx: &BotCtx, client: &Client) -> Result<Screen, BotError> {
    let services = &ctx.services;
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let settings = services.settings();
    let now = services.now();
    let offset = settings.remna_offset();
    let subscriptions = live(services.subscriptions().list_for_client(client.id).await?);
    let wants_name = ["menu.title", "menu.greeting"]
        .iter()
        .any(|key| c.content.template(key).contains("{name}"));
    let name = if wants_name {
        first_name(services, client).await
    } else {
        String::new()
    };
    let head = [
        ("brand", Text(settings.brand_name.as_str())),
        ("name", Text(name.as_str())),
    ];
    let mut lines = Lines::new();
    lines.add(ui::text(c, "menu.title", &head));
    lines.add(ui::text(c, "menu.greeting", &head));
    match subscriptions.as_slice() {
        [] => lines.add(ui::text(c, "menu.no_subscription", &[])),
        [only] => {
            let tariff = tariff_name(services, only.tariff_id).await;
            for line in single_lines(c, only, &tariff, now, offset) {
                lines.add(line);
            }
            if services.payments().balance_enabled() {
                let balance = balance_of(services, client).await?;
                lines.add(ui::text(
                    c,
                    "menu.balance",
                    &[("balance", Text(&text::money(balance)))],
                ));
            }
        }
        many => {
            for subscription in many {
                let tariff = tariff_name(services, subscription.tariff_id).await;
                let status = status_text(c, subscription, now);
                lines.add(match subscription.mirror.expire_at {
                    Some(at) => ui::text(
                        c,
                        "menu.subscription",
                        &[
                            ("tariff", Text(&tariff)),
                            ("status", Html(&status)),
                            ("date", Text(&text::date(at, offset))),
                        ],
                    ),
                    None => ui::text(
                        c,
                        "menu.subscription_open",
                        &[("tariff", Text(&tariff)), ("status", Html(&status))],
                    ),
                });
            }
        }
    }
    let install = Install::load(services).await;
    let subscribed = !subscriptions.is_empty();
    let trial_left = !subscribed && install.trial && trial_available(services, client).await?;
    let money = !install.balance && has_balance(services, client).await?;
    let facts = install.menu(subscribed, trial_left, money);
    let present: Vec<(&str, Button)> = facts
        .keys()
        .into_iter()
        .filter_map(|key| menu_button(c, &install, key).map(|button| (key, button)))
        .collect();
    let rows = if c.content.has_own_layout(LayoutScreen::Menu) {
        arrange(&c.content.layout(LayoutScreen::Menu), present)
    } else {
        pack(present, c.content.menu_columns())
    };
    Ok(Screen::new(lines.text())
        .keyboard(Keyboard::new().rows(rows))
        .section(Section::Menu))
}

pub const SUPPORT_START: &str = "support";

pub fn support_link(settings: &CoreSettings) -> Option<&str> {
    settings.support_link().filter(|u| link_button(u))
}

fn paysupport(ctx: &BotCtx) -> Screen {
    let content = ctx.services.bot_content_in(lang::current());
    let settings = ctx.services.settings();
    let mut keyboard = Keyboard::new();
    let link = support_link(&settings);
    if let Some(url) = link {
        keyboard = keyboard.push(ui::link(&content, "pay.btn.write_support", &[], url));
    }
    if settings.support_tickets() || link.is_none() {
        keyboard = keyboard.push(ui::button(&content, "pay.btn.support_bot", &[], "sup"));
    }
    Screen::new(ui::text(&content, "pay.paysupport", &[]))
        .keyboard(keyboard)
        .section(Section::Support)
}

pub fn who(user: &BotUser) -> String {
    match &user.username {
        Some(username) => format!("{} (@{}, Telegram {})", user.first_name, username, user.id),
        None => format!("{} (Telegram {})", user.first_name, user.id),
    }
}

async fn login_screen(
    turn: &mut Turn<'_>,
    client: &Client,
    param: &str,
) -> Result<Screen, BotError> {
    let Some(token) = login::token_from_param(param) else {
        return main_menu(turn.ctx, client).await;
    };
    let content = turn.content();
    let c = &*content;
    let request = match login::describe(turn.services(), token, &profile(turn.user)).await {
        Ok(request) => request,
        Err(err) => {
            return Ok(Screen::notice(ui::text(
                c,
                "login.error",
                &[("error", Text(&text::user_error(&err)))],
            ))
            .section(Section::Login));
        }
    };
    turn.session.login_token = Some(token.to_string());
    let who = who(turn.user);
    let mut lines = Lines::new();
    lines.add(ui::text(c, "login.title", &[]));
    lines.add(ui::text(c, "login.account", &[("who", Text(&who))]));
    if let Some(ip) = &request.ip {
        lines.add(ui::text(c, "login.ip", &[("ip", Text(ip))]));
    }
    if let Some(agent) = &request.user_agent {
        lines.add(ui::text(c, "login.browser", &[("agent", Text(agent))]));
    }
    lines.add(ui::text(c, "login.hint", &[]));
    Ok(Screen::new(lines.text())
        .keyboard(
            Keyboard::new()
                .push(ui::button(c, "login.btn.confirm", &[], "auth:ok"))
                .push(ui::button(c, "login.btn.reject", &[], "auth:no")),
        )
        .section(Section::Login))
}

async fn login_decision(turn: &mut Turn<'_>, rest: &str) -> Result<Screen, BotError> {
    let content = turn.content();
    let Some(token) = turn.session.login_token.take() else {
        return Ok(Screen::notice(ui::text(&content, "login.expired", &[])).section(Section::Login));
    };
    let result = match rest {
        "ok" => login::confirm(turn.services(), &token, &profile(turn.user))
            .await
            .map(|_| "login.confirmed"),
        _ => login::reject(turn.services(), &token)
            .await
            .map(|_| "login.rejected"),
    };
    Ok(match result {
        Ok(key) => Screen::notice(ui::text(&content, key, &[])).section(Section::Login),
        Err(err) => Screen::notice(text::user_error(&err)),
    })
}

async fn pre_checkout(ctx: &BotCtx, query: &PreCheckoutQuery) -> Result<(), BotError> {
    if crate::maintenance::closed_for(ctx, &query.from).await? {
        let text = crate::maintenance::text_for(ctx, &query.from).await?;
        ctx.api.answer_pre_checkout(&query.id, Some(&text)).await?;
        return Ok(());
    }
    let request = StarsPreCheckout {
        query_id: query.id.clone(),
        invoice_payload: query.invoice_payload.clone(),
        total_amount: query.total_amount,
        currency: query.currency.clone(),
        telegram_user_id: query.from.id,
    };
    let error = match ctx.services.inbox().stars_pre_checkout(&request).await {
        Ok(()) => None,
        Err(err) => Some(text::user_error(&err)),
    };
    ctx.api
        .answer_pre_checkout(&query.id, error.as_deref())
        .await?;
    Ok(())
}

async fn stars_paid(ctx: &BotCtx, target: Target, event: &StarsPayment) -> Result<(), BotError> {
    let content = ctx.services.bot_content_in(lang::current());
    let c = &*content;
    let deferred = Screen::notice(ui::text(c, "pay.stars_deferred", &[])).section(Section::Payment);
    let screen = match stars::settle(&ctx.services, event).await {
        Settled::Done => Screen::new(ui::text(c, "pay.stars_done", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "common.btn.subscription", &[], "subs"))
                    .push(ui::menu(c)),
            )
            .section(Section::Payment),
        Settled::Repeat => {
            Screen::notice(ui::text(c, "pay.stars_repeat", &[])).section(Section::Payment)
        }
        Settled::Waiting(reason) => {
            tracing::warn!(
                charge = %event.telegram_payment_charge_id,
                reason = %reason,
                "оплата звёздами сохранена и будет проведена повторно"
            );
            deferred
        }
        Settled::Failed(reason) => {
            tracing::error!(
                charge = %event.telegram_payment_charge_id,
                reason = %reason,
                "оплата звёздами не проведена"
            );
            deferred
        }
    };
    ctx.show(target, &screen).await
}

async fn stars_refunded(ctx: &BotCtx, event: &StarsPayment) {
    match stars::settle(&ctx.services, event).await {
        Settled::Done | Settled::Repeat => {}
        Settled::Waiting(reason) => tracing::info!(
            charge = %event.telegram_payment_charge_id,
            reason = %reason,
            "возврат звёзд сохранён и будет записан позже"
        ),
        Settled::Failed(reason) => tracing::warn!(
            charge = %event.telegram_payment_charge_id,
            reason = %reason,
            "возврат звёзд не записан"
        ),
    }
}
