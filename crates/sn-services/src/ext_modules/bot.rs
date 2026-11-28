use super::{ExtModulesOps, address, runtime, secret_key, signing, store};
use crate::error::ServiceResult;
use crate::settings::read_secret;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sn_domain::bot_content::{normalize_markup, visible_len};
use sn_domain::client::Locale;
use sn_domain::ids::ClientId;
use std::time::Duration;

pub const SCREEN_KIND: &str = "bot_screen";
pub const CALLBACK_PREFIX: &str = "xm";
pub const TEXT_MAX: usize = 3500;
pub const ROWS_MAX: usize = 6;
pub const ROW_BUTTONS_MAX: usize = 3;
pub const BUTTON_TEXT_MAX: usize = 64;
const RESPONSE_MAX: usize = 64 * 1024;
const SCREEN_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleButton {
    pub module: String,
    pub button: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenAction {
    WebApp(String),
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenButton {
    pub text: String,
    pub action: ScreenAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleScreen {
    pub html: String,
    pub rows: Vec<Vec<ScreenButton>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenRequest<'a> {
    pub client_id: ClientId,
    pub telegram_id: i64,
    pub locale: Locale,
    pub first_name: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenFailure {
    Missing,
    Unreachable(String),
    Invalid(String),
}

#[derive(Deserialize)]
struct RawButton {
    text: String,
    #[serde(default)]
    web_app: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
struct RawScreen {
    text: String,
    #[serde(default)]
    buttons: Vec<Vec<RawButton>>,
}

fn invalid(message: impl Into<String>) -> ScreenFailure {
    ScreenFailure::Invalid(message.into())
}

fn https(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw.trim()).ok()?;
    (url.scheme() == "https"
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}

fn link(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.starts_with("tg://") && raw.len() <= 512 && !raw.contains(char::is_whitespace) {
        return Some(raw.to_string());
    }
    https(raw)
}

pub fn parse_screen(body: &[u8]) -> Result<ModuleScreen, ScreenFailure> {
    let raw: RawScreen = serde_json::from_slice(body)
        .map_err(|_| invalid("модуль ответил не тем: ждали JSON с полями text и buttons"))?;
    let html = normalize_markup(raw.text.trim())
        .map_err(|problem| invalid(format!("разметка текста: {}", problem.message())))?;
    if html.trim().is_empty() {
        return Err(invalid("текст экрана пустой"));
    }
    if visible_len(&html) > TEXT_MAX {
        return Err(invalid(format!("текст экрана длиннее {TEXT_MAX} символов")));
    }
    if raw.buttons.len() > ROWS_MAX {
        return Err(invalid(format!("кнопок — не больше {ROWS_MAX} рядов")));
    }
    let mut rows = Vec::new();
    for row in raw.buttons {
        if row.len() > ROW_BUTTONS_MAX {
            return Err(invalid(format!(
                "в ряду не больше {ROW_BUTTONS_MAX} кнопок"
            )));
        }
        let mut line = Vec::new();
        for button in row {
            let text = button.text.trim().to_string();
            if text.is_empty() || text.chars().count() > BUTTON_TEXT_MAX || text.contains('\n') {
                return Err(invalid(format!(
                    "надпись на кнопке — одна строка до {BUTTON_TEXT_MAX} символов"
                )));
            }
            let action = match (button.web_app.as_deref(), button.url.as_deref()) {
                (Some(url), None) => ScreenAction::WebApp(
                    https(url)
                        .ok_or_else(|| invalid("кнопка web_app открывает только https://"))?,
                ),
                (None, Some(url)) => ScreenAction::Url(
                    link(url)
                        .ok_or_else(|| invalid("кнопка url ведёт только на https:// или tg://"))?,
                ),
                _ => return Err(invalid("у кнопки должен быть ровно один из web_app и url")),
            };
            line.push(ScreenButton { text, action });
        }
        if !line.is_empty() {
            rows.push(line);
        }
    }
    Ok(ModuleScreen { html, rows })
}

impl ExtModulesOps<'_> {
    pub async fn bot_buttons(&self) -> ServiceResult<Vec<ModuleButton>> {
        let mut conn = self.services.store().conn().await?;
        let mut buttons: Vec<ModuleButton> = store::list(&mut conn)
            .await?
            .into_iter()
            .filter(|row| row.enabled && row.managed && row.manifest.run.is_some())
            .filter_map(|row| {
                let bot = row.manifest.bot.as_ref()?;
                Some(ModuleButton {
                    module: row.manifest.id.clone(),
                    button: bot.button.clone(),
                })
            })
            .collect();
        buttons.sort_by(|a, b| {
            a.button
                .cmp(&b.button)
                .then_with(|| a.module.cmp(&b.module))
        });
        Ok(buttons)
    }

    pub async fn bot_screen(
        &self,
        module: &str,
        request: ScreenRequest<'_>,
    ) -> Result<ModuleScreen, ScreenFailure> {
        if !runtime::valid_module(module) {
            return Err(ScreenFailure::Missing);
        }
        let unreachable =
            |err: crate::error::ServiceError| ScreenFailure::Unreachable(err.to_string());
        let mut conn = self
            .services
            .store()
            .conn()
            .await
            .map_err(|e| unreachable(e.into()))?;
        let row = store::get(&mut conn, module)
            .await
            .map_err(unreachable)?
            .filter(|row| row.enabled && row.managed)
            .ok_or(ScreenFailure::Missing)?;
        let (Some(bot), Some(run)) = (row.manifest.bot.as_ref(), row.manifest.run.as_ref()) else {
            return Err(ScreenFailure::Missing);
        };
        let key = read_secret(&mut conn, self.services.secrets(), &secret_key(module))
            .await
            .map_err(unreachable)?
            .as_deref()
            .and_then(signing::key)
            .ok_or_else(|| {
                ScreenFailure::Unreachable("секрет подписи модуля не прочитан".into())
            })?;
        drop(conn);
        let url = runtime::url(
            self.services.update_config().as_deref(),
            module,
            run.port,
            &bot.path,
        );
        let now = self.services.now();
        let id = format!("scr_{}", signing::nonce());
        let body = serde_json::to_vec(&json!({
            "id": id,
            "type": SCREEN_KIND,
            "module": module,
            "occurred_at": now,
            "data": {
                "client_id": request.client_id,
                "telegram_id": request.telegram_id,
                "locale": request.locale,
                "first_name": request.first_name,
            },
        }))
        .unwrap_or_default();
        let signature = signing::signature(&key, &id, now.timestamp(), &body)
            .ok_or_else(|| ScreenFailure::Unreachable("подпись не собрана".into()))?;
        let response = address::internal_client()
            .post(&url)
            .timeout(SCREEN_TIMEOUT)
            .header(CONTENT_TYPE, "application/json")
            .header("webhook-id", &id)
            .header("webhook-timestamp", now.timestamp().to_string())
            .header("webhook-signature", signature)
            .body(body)
            .send()
            .await
            .map_err(|err| {
                ScreenFailure::Unreachable(if err.is_timeout() {
                    "модуль не ответил за 4 секунды".into()
                } else {
                    "не удалось соединиться с модулем".into()
                })
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(ScreenFailure::Unreachable(format!(
                "модуль ответил кодом {}",
                status.as_u16()
            )));
        }
        let mut response = response;
        let mut buffer: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ScreenFailure::Unreachable("ответ модуля оборвался".into()))?
        {
            if buffer.len() + chunk.len() > RESPONSE_MAX {
                return Err(invalid("ответ модуля больше 64 КБ"));
            }
            buffer.extend_from_slice(&chunk);
        }
        parse_screen(&buffer)
    }
}

pub fn callback(module: &str) -> String {
    format!("{CALLBACK_PREFIX}:{module}")
}

pub fn describe(failure: &ScreenFailure) -> Value {
    match failure {
        ScreenFailure::Missing => json!("модуль не найден или выключен"),
        ScreenFailure::Unreachable(why) | ScreenFailure::Invalid(why) => json!(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(value: Value) -> Result<ModuleScreen, ScreenFailure> {
        parse_screen(value.to_string().as_bytes())
    }

    #[test]
    fn экран_модуля_разбирается_и_разметка_чинится() {
        let parsed = screen(json!({
            "text": "🎮 <b>Игра</b> & <i>бонусы</i>\n👉 Ваши попытки: 5",
            "buttons": [
                [{ "text": "🕹️ Играть", "web_app": "https://game.example/?v=1" }],
                [{ "text": "Правила", "url": "https://example.com/rules" }, { "text": "Канал", "url": "tg://resolve?domain=wolf" }]
            ]
        }))
        .unwrap();
        assert_eq!(
            parsed.html,
            "🎮 <b>Игра</b> &amp; <i>бонусы</i>\n👉 Ваши попытки: 5"
        );
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(
            parsed.rows[0][0].action,
            ScreenAction::WebApp("https://game.example/?v=1".into())
        );
        assert_eq!(
            parsed.rows[1][1].action,
            ScreenAction::Url("tg://resolve?domain=wolf".into())
        );
        let plain = screen(json!({ "text": "Только текст" })).unwrap();
        assert!(plain.rows.is_empty());
        let escaped = screen(json!({ "text": "<script>x</script>" })).unwrap();
        assert_eq!(escaped.html, "&lt;script&gt;x&lt;/script&gt;");
        assert!(matches!(
            screen(json!({ "text": "<tg-emoji emoji-id=\"abc\">🔥</tg-emoji>" })),
            Err(ScreenFailure::Invalid(why)) if why.contains("разметка текста")
        ));
    }

    #[test]
    fn плохой_экран_модуля_отклоняется() {
        let row = |button: Value| json!({ "text": "Игра", "buttons": [[button]] });
        let cases = [
            (json!({ "text": "" }), "текст экрана пустой"),
            (json!({ "text": "x".repeat(TEXT_MAX + 1) }), "длиннее"),
            (json!({ "buttons": [] }), "ждали JSON"),
            (
                row(json!({ "text": "Играть", "web_app": "http://game.example/" })),
                "web_app открывает только https",
            ),
            (
                row(json!({ "text": "Сайт", "url": "javascript:alert(1)" })),
                "url ведёт только на https",
            ),
            (
                row(
                    json!({ "text": "Два", "url": "https://a.example", "web_app": "https://b.example" }),
                ),
                "ровно один",
            ),
            (row(json!({ "text": "Пусто" })), "ровно один"),
            (
                row(json!({ "text": "x".repeat(BUTTON_TEXT_MAX + 1), "url": "https://a.example" })),
                "надпись на кнопке",
            ),
            (
                json!({ "text": "Игра", "buttons": [[{"text":"1","url":"https://a.example"},{"text":"2","url":"https://a.example"},{"text":"3","url":"https://a.example"},{"text":"4","url":"https://a.example"}]] }),
                "в ряду не больше",
            ),
            (
                json!({ "text": "Игра", "buttons": vec![vec![json!({"text":"1","url":"https://a.example"})]; ROWS_MAX + 1] }),
                "рядов",
            ),
        ];
        for (value, part) in cases {
            match screen(value.clone()) {
                Err(ScreenFailure::Invalid(why)) => assert!(why.contains(part), "{value}: {why}"),
                other => panic!("{value}: {other:?}"),
            }
        }
        assert!(matches!(
            parse_screen(b"not json"),
            Err(ScreenFailure::Invalid(_))
        ));
    }
}
