use crate::BotError;
use crate::handler::{Screen, Turn, id_arg, main_menu};
use crate::telegram::Keyboard;
use crate::text::{escape, lowercase_first};
use crate::ui::{self, Lines, Text};
use chrono::FixedOffset;
use sn_domain::bot_content::Section;
use sn_domain::client::Client;
use sn_domain::lang;
use sn_domain::module::ModuleId;
use sn_modules::catalog::contests::{self, ClientContest, JoinOutcome, PrizeConfig, Status};
use sn_modules::catalog::{antibot, referral};
use sn_services::Services;
use sn_services::bot_content::BotContent;

pub const OPEN: &str = "ct";
pub const JOIN: &str = "cj";
pub const CONFIRM: &str = "cjr";

fn card_text(c: &BotContent, contest: &ClientContest, offset: FixedOffset) -> String {
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "contest.title",
        &[("title", Text(&contest.title))],
    ));
    if let Some(description) = &contest.description {
        lines.add(escape(description));
    }
    match &contest.prize {
        PrizeConfig::Tiered(tiers) => {
            lines.add(ui::text(c, "contest.prizes", &[]));
            for tier in tiers {
                lines.add(format!(
                    "{} — {}",
                    contests::place_title(tier.from_place, tier.to_place),
                    escape(&tier.prize.label())
                ));
            }
            lines.add(ui::text(
                c,
                "contest.winners",
                &[("winners", Text(&contest.winners.to_string()))],
            ));
        }
        PrizeConfig::Single(_) => {
            lines.add(ui::text(
                c,
                "contest.prize",
                &[
                    ("prize", Text(&contest.prize_label)),
                    ("winners", Text(&contest.winners.to_string())),
                ],
            ));
        }
    }
    lines.add(ui::text(
        c,
        "contest.until",
        &[("date", Text(&contests::when(contest.ends_at, offset)))],
    ));
    let running = contest.status == Status::Running;
    if contest.checks.is_empty() {
        if running {
            lines.add(ui::text(c, "contest.open", &[]));
        }
    } else {
        lines.add(ui::text(c, "contest.conditions", &[]));
        for check in &contest.checks {
            let key = match (running, contest.joined, check.ok) {
                (true, false, false) => "contest.condition_no",
                (true, false, true) | (true, true, _) => "contest.condition_ok",
                (false, _, _) => "contest.condition",
            };
            lines.add(ui::text(c, key, &[("condition", Text(&check.text))]));
        }
    }
    lines.add(ui::text(
        c,
        "contest.entries",
        &[("count", Text(&contest.entries.to_string()))],
    ));
    match contest.status {
        Status::Running if contest.joined => lines.add(ui::text(c, "contest.joined", &[])),
        Status::Running if !contest.eligible() => {
            lines.add(ui::text(c, "contest.not_eligible", &[]));
        }
        Status::Drawing => lines.add(ui::text(c, "contest.drawing", &[])),
        Status::Finished => match &contest.result {
            Some(result) if result.won => {
                lines.add(ui::text(
                    c,
                    "contest.won",
                    &[
                        (
                            "prize",
                            Text(
                                result
                                    .prize_label
                                    .as_deref()
                                    .unwrap_or(&contest.prize_label),
                            ),
                        ),
                        (
                            "result",
                            Text(result.prize_note.as_deref().unwrap_or_default()),
                        ),
                    ],
                ));
                if let Some(code) = &result.prize_code {
                    lines.add(ui::text(c, "contest.won_code", &[("code", Text(code))]));
                }
            }
            Some(result) => match &result.excluded {
                Some(reason) => lines.add(ui::text(
                    c,
                    "contest.excluded",
                    &[("comment", Text(&lowercase_first(reason)))],
                )),
                None => lines.add(ui::text(c, "contest.lost", &[])),
            },
            None => {}
        },
        _ => {}
    }
    if matches!(contest.status, Status::Running | Status::Drawing) {
        lines.add(ui::text(
            c,
            "contest.fair",
            &[("hash", Text(&contests::short_hash(&contest.seed_hash)))],
        ));
    }
    lines.text()
}

fn card(turn: &Turn<'_>, contest: &ClientContest, many: bool) -> Screen {
    let services = turn.services();
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    let offset = services.settings().remna_offset();
    let mut keyboard = Keyboard::new();
    if contest.status == Status::Running && !contest.joined {
        keyboard = keyboard.push(ui::button(
            c,
            "contest.btn.join",
            &[],
            format!("{JOIN}:{}", contest.id),
        ));
        let missing = |key: &str| contest.checks.iter().any(|ch| ch.key == key && !ch.ok);
        if missing("subscription") || missing("purchase") {
            keyboard = keyboard.push(ui::button(c, "contest.btn.buy", &[], "buy"));
        }
        if missing("friends") && services.modules().is_enabled(ModuleId::Referral) {
            keyboard = keyboard.push(ui::button(
                c,
                "contest.btn.invite",
                &[],
                format!("x:{}", referral::BOT_ENTRY),
            ));
        }
    }
    if many {
        keyboard = keyboard.push(ui::button(
            c,
            "contest.btn.all",
            &[],
            format!("x:{}", contests::BOT_ENTRY),
        ));
    }
    Screen::new(card_text(c, contest, offset))
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Contests)
}

fn list(turn: &Turn<'_>, all: &[ClientContest]) -> Screen {
    let content = turn.content();
    let c = &*content;
    let mut keyboard = Keyboard::new();
    for contest in all {
        keyboard = keyboard.push(ui::button(
            c,
            "contest.btn.open",
            &[("title", Text(&contest.title))],
            format!("{OPEN}:{}", contest.id),
        ));
    }
    Screen::new(ui::text(c, "contest.list", &[]))
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Contests)
}

pub async fn entry(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let ctx = turn.services().module_context();
    let all = contests::visible_for(&ctx, client.id).await?;
    Ok(match all.as_slice() {
        [] => {
            let content = turn.content();
            Screen::new(ui::text(&content, "contest.none", &[]))
                .with_menu()
                .section(Section::Contests)
        }
        [only] => card(turn, only, false),
        many => list(turn, many),
    })
}

pub async fn open(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Contests) {
        return main_menu(turn.ctx, client).await;
    }
    let Some(id) = id_arg(rest) else {
        return entry(turn, client).await;
    };
    let ctx = turn.services().module_context();
    let all = contests::visible_for(&ctx, client.id).await?;
    match all.iter().find(|c| c.id == id) {
        Some(found) => Ok(card(turn, found, all.len() > 1)),
        None => entry(turn, client).await,
    }
}

pub async fn join(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Contests) {
        return main_menu(turn.ctx, client).await;
    }
    let Some(id) = id_arg(rest) else {
        return entry(turn, client).await;
    };
    let ctx = turn.services().module_context();
    let outcome = contests::join(&ctx, id, client.id).await?;
    joined(turn, client, id, outcome).await
}

pub async fn confirmed(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Contests) {
        return main_menu(turn.ctx, client).await;
    }
    let Some((id, token)) = rest.split_once(':') else {
        return entry(turn, client).await;
    };
    let Some(id) = id_arg(id) else {
        return entry(turn, client).await;
    };
    let ctx = turn.services().module_context();
    let outcome = contests::join_confirmed(&ctx, id, client.id, token).await?;
    joined(turn, client, id, outcome).await
}

async fn human_check(turn: &Turn<'_>, client: &Client, id: i64) -> Result<Screen, BotError> {
    let services = turn.services();
    let token = antibot::challenge(&services.module_context(), client.id).await?;
    let content = services.bot_content_in(lang::current());
    let c = &*content;
    Ok(Screen::new(ui::text(c, "contest.confirm", &[]))
        .keyboard(
            Keyboard::new()
                .push(ui::button(
                    c,
                    "contest.btn.confirm",
                    &[],
                    format!("{CONFIRM}:{id}:{token}"),
                ))
                .push(ui::menu(c)),
        )
        .section(Section::Contests))
}

async fn joined(
    turn: &mut Turn<'_>,
    client: &Client,
    id: i64,
    outcome: JoinOutcome,
) -> Result<Screen, BotError> {
    match outcome {
        JoinOutcome::Confirm => return human_check(turn, client, id).await,
        JoinOutcome::Denied => {
            let content = turn.content();
            return Ok(Screen::notice(ui::text(&content, "contest.denied", &[]))
                .section(Section::Contests));
        }
        _ => {}
    }
    let ctx = turn.services().module_context();
    let all = contests::visible_for(&ctx, client.id).await?;
    let Some(found) = all.iter().find(|c| c.id == id) else {
        let content = turn.content();
        return Ok(
            Screen::notice(ui::text(&content, "contest.closed", &[])).section(Section::Contests)
        );
    };
    let mut screen = card(turn, found, all.len() > 1);
    let content = turn.content();
    let offset = turn.services().settings().remna_offset();
    match outcome {
        JoinOutcome::Joined => {
            let head = ui::text(
                &content,
                "contest.joined_now",
                &[
                    ("title", Text(&found.title)),
                    ("date", Text(&contests::when(found.ends_at, offset))),
                ],
            );
            screen.text = format!("{head}\n\n{}", screen.text);
        }
        JoinOutcome::Closed => {
            let head = ui::text(&content, "contest.closed", &[]);
            screen.text = format!("{head}\n\n{}", screen.text);
        }
        JoinOutcome::Blocked => {
            return Ok(Screen::new(ui::text(&content, "common.blocked", &[])));
        }
        JoinOutcome::Already
        | JoinOutcome::NotEligible
        | JoinOutcome::Confirm
        | JoinOutcome::Denied => {}
    }
    Ok(screen)
}

pub async fn has_open(services: &Services) -> bool {
    if !services.modules().is_enabled(ModuleId::Contests) {
        return false;
    }
    contests::has_running(&services.module_context())
        .await
        .unwrap_or(false)
}
