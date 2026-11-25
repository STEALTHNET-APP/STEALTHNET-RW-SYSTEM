use crate::BotError;
use crate::handler::{Screen, Turn, profile, who};
use crate::telegram::Keyboard;
use crate::text;
use crate::ui::{self, Lines, Text};
use sn_domain::bot_content::Section;
use sn_domain::client::IdentityKind;
use sn_services::telegram_link::{START_PREFIX, TelegramLinkPrompt};
use sn_services::{ServiceError, Services};
use sn_store::Db;

const CALLBACK: &str = "tl:";
const CONFIRM: &str = "ok";
const CANCEL: &str = "no";

pub fn opens(param: &str) -> bool {
    param.starts_with(START_PREFIX)
}

pub fn decision(data: &str) -> Option<&str> {
    data.strip_prefix(CALLBACK)
}

fn code(param: &str) -> &str {
    let first = param.split("__").next().unwrap_or(param);
    first.strip_prefix(START_PREFIX).unwrap_or_default()
}

async fn known(services: &Services, user: i64) -> Result<bool, BotError> {
    let mut conn = services.store().conn().await?;
    Ok(conn
        .clients()
        .find_by_identity(IdentityKind::Telegram, &user.to_string())
        .await?
        .is_some())
}

async fn reply(turn: &Turn<'_>, text: String) -> Result<Screen, BotError> {
    let screen = Screen::new(text).section(Section::Login);
    Ok(if known(turn.services(), turn.user.id).await? {
        screen.with_menu()
    } else {
        screen
    })
}

async fn refused(turn: &Turn<'_>, err: &ServiceError) -> Result<Screen, BotError> {
    let text = ui::text(
        &turn.content(),
        "link.error",
        &[("error", Text(&text::user_error(err)))],
    );
    reply(turn, text).await
}

pub async fn opened(turn: &mut Turn<'_>, param: &str) -> Result<Screen, BotError> {
    turn.session.awaiting = None;
    let content = turn.content();
    let c = &*content;
    let code = code(param);
    let prompt = turn
        .services()
        .telegram_links()
        .prompt(code, &profile(turn.user))
        .await;
    let (account, site, closes_empty) = match prompt {
        Ok(TelegramLinkPrompt::Ask {
            account,
            site,
            closes_empty,
        }) => (account, site, closes_empty),
        Ok(TelegramLinkPrompt::Linked) => {
            return reply(turn, ui::text(c, "link.same", &[])).await;
        }
        Err(err) => return refused(turn, &err).await,
    };
    let mut lines = Lines::new();
    lines.add(ui::text(c, "link.title", &[]));
    lines.add(ui::text(
        c,
        "link.account",
        &[("site", Text(&site)), ("account", Text(&account))],
    ));
    lines.add(ui::text(c, "link.who", &[("who", Text(&who(turn.user)))]));
    if closes_empty {
        lines.add(ui::text(c, "link.empty", &[]));
    }
    lines.add(ui::text(c, "link.hint", &[]));
    Ok(Screen::new(lines.text())
        .keyboard(
            Keyboard::new()
                .push(ui::button(
                    c,
                    "link.btn.confirm",
                    &[],
                    format!("{CALLBACK}{CONFIRM}:{code}"),
                ))
                .push(ui::button(
                    c,
                    "link.btn.cancel",
                    &[],
                    format!("{CALLBACK}{CANCEL}:{code}"),
                )),
        )
        .section(Section::Login))
}

pub async fn decided(turn: &mut Turn<'_>, rest: &str) -> Result<Screen, BotError> {
    turn.session.awaiting = None;
    let content = turn.content();
    let (choice, code) = rest.split_once(':').unwrap_or((rest, ""));
    let links = turn.services().telegram_links();
    let profile = profile(turn.user);
    if choice != CONFIRM {
        return match links.reject(code, &profile).await {
            Ok(()) => reply(turn, ui::text(&content, "link.canceled", &[])).await,
            Err(err) => refused(turn, &err).await,
        };
    }
    match links.confirm(code, &profile).await {
        Ok(linked) => {
            let mut conn = turn.services().store().conn().await?;
            if let Err(err) = conn
                .client_photos()
                .request_if_stale(linked.client_id, crate::photos::REFRESH_AFTER)
                .await
            {
                tracing::debug!(error = %err, "запрос фото клиента не сохранён");
            }
            drop(conn);
            let key = if linked.repeated {
                "link.same"
            } else {
                "link.done"
            };
            reply(turn, ui::text(&content, key, &[])).await
        }
        Err(err) => refused(turn, &err).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ссылка_привязки_узнаётся_по_своей_приставке() {
        assert!(opens("tglink_0123456789abcdef0123456789abcdef"));
        assert!(opens("tglink_мусор"));
        assert!(!opens("auth_0123456789abcdef0123456789abcdef"));
        assert!(!opens("ref_tglink_1"));
        assert_eq!(code("tglink_abc__ref_X"), "abc");
        assert_eq!(decision("tl:ok:abc"), Some("ok:abc"));
        assert_eq!(decision("auth:ok"), None);
    }
}
