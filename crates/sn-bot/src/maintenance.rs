use crate::BotError;
use crate::handler::{BotCtx, user_locale};
use crate::telegram::BotUser;
use crate::text::escape;

pub async fn closed_for(ctx: &BotCtx, user: &BotUser) -> Result<bool, BotError> {
    let ops = ctx.services.maintenance();
    if !ops.active() {
        return Ok(false);
    }
    Ok(!ops.lets_through(user.id).await?)
}

pub async fn text_for(ctx: &BotCtx, user: &BotUser) -> Result<String, BotError> {
    let locale = user_locale(&ctx.services, user).await?;
    Ok(ctx.services.maintenance().text(locale))
}

pub async fn notify(ctx: &BotCtx, user: &BotUser, chat_id: i64) -> Result<(), BotError> {
    let ops = ctx.services.maintenance();
    if !ops.claim_notice(chat_id).await? {
        return Ok(());
    }
    let text = escape(&text_for(ctx, user).await?);
    if let Err(err) = ctx.api.send_message(chat_id, &text, None).await {
        if let Err(release) = ops.release_notice(chat_id).await {
            tracing::warn!(error = %release, "отметка о сообщении про работы не снята");
        }
        return Err(err.into());
    }
    Ok(())
}
