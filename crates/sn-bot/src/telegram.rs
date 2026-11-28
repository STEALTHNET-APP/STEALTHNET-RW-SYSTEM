use crate::pacer::{Class, Pacer, SendLimits, sends};
use chrono::DateTime;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sn_domain::bot_content::clip_markup;
use sn_services::bot_buttons::MenuButton;
use sn_services::bot_webhook::ALLOWED_UPDATES;
use sn_services::server_clock::ServerClock;
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_API: &str = "https://api.telegram.org";
pub const CALLBACK_DATA_MAX: usize = 64;
pub const MESSAGE_MAX: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TelegramError {
    #[error("Telegram недоступен: {0}")]
    Transport(String),
    #[error("Telegram отказал ({code}): {description}")]
    Api {
        code: i64,
        description: String,
        retry_after: Option<u64>,
        #[source]
        migrated: Option<Migrated>,
    },
    #[error("ответ Telegram не разобран: {0}")]
    Decode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("группа стала супергруппой {0}")]
pub struct Migrated(pub i64);

impl TelegramError {
    pub fn migrated_to(&self) -> Option<i64> {
        match self {
            TelegramError::Api {
                migrated: Some(Migrated(id)),
                ..
            } => Some(*id),
            _ => None,
        }
    }

    pub fn retry_after(&self) -> Option<u64> {
        match self {
            TelegramError::Api {
                code: 429,
                retry_after,
                ..
            } => Some(retry_after.unwrap_or(5)),
            _ => None,
        }
    }

    pub fn is_forbidden(&self) -> bool {
        matches!(self, TelegramError::Api { code: 403, .. })
    }

    pub fn is_thread_missing(&self) -> bool {
        matches!(self, TelegramError::Api { code: 400, description, .. }
            if description.contains("thread not found") || description.contains("TOPIC_DELETED") || description.contains("TOPIC_CLOSED"))
    }

    pub fn is_chat_missing(&self) -> bool {
        matches!(self, TelegramError::Api { code: 400, description, .. }
            if description.contains("chat not found") || description.contains("upgraded to a supergroup"))
    }

    pub fn is_not_modified(&self) -> bool {
        matches!(self, TelegramError::Api { code: 400, description, .. } if description.contains("message is not modified"))
    }

    pub fn is_parse_error(&self) -> bool {
        matches!(self, TelegramError::Api { code: 400, description, .. } if description.contains("can't parse entities"))
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            TelegramError::Transport(_) | TelegramError::Decode(_) => true,
            TelegramError::Api { code, .. } => *code == 429 || *code >= 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotUser {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub language_code: Option<String>,
    #[serde(default)]
    pub is_premium: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_forum: Option<bool>,
}

impl Chat {
    pub fn is_private(&self) -> bool {
        self.kind == "private"
    }

    pub fn is_group(&self) -> bool {
        self.kind == "group" || self.kind == "supergroup"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ForumTopic {
    pub message_thread_id: i64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessfulPayment {
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
    pub telegram_payment_charge_id: String,
    #[serde(default)]
    pub provider_payment_charge_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundedPayment {
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
    pub telegram_payment_charge_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub message_id: i64,
    #[serde(default)]
    pub from: Option<BotUser>,
    pub chat: Chat,
    #[serde(default)]
    pub date: i64,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub successful_payment: Option<SuccessfulPayment>,
    #[serde(default)]
    pub refunded_payment: Option<RefundedPayment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<Vec<PhotoSize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<MessageEntity>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<Document>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<Video>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_group_id: Option<String>,
}

impl Message {
    pub fn has_photo(&self) -> bool {
        self.photo.as_ref().is_some_and(|p| !p.is_empty())
    }

    pub fn largest_photo(&self) -> Option<&PhotoSize> {
        self.photo
            .as_ref()?
            .iter()
            .max_by_key(|p| p.width.saturating_mul(p.height))
    }

    pub fn custom_emoji(&self) -> Vec<(String, String)> {
        let Some(text) = self.text.as_deref() else {
            return Vec::new();
        };
        let units: Vec<u16> = text.encode_utf16().collect();
        self.entities
            .iter()
            .flatten()
            .filter(|e| e.kind == "custom_emoji")
            .filter_map(|e| {
                let id = e.custom_emoji_id.clone()?;
                let start = usize::try_from(e.offset).ok()?;
                let end = start.checked_add(usize::try_from(e.length).ok()?)?;
                let fallback = String::from_utf16(units.get(start..end)?).ok()?;
                Some((fallback, id))
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    #[serde(default)]
    pub file_unique_id: String,
    #[serde(default)]
    pub width: i64,
    #[serde(default)]
    pub height: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Video {
    pub file_id: String,
    #[serde(default)]
    pub file_unique_id: String,
    #[serde(default)]
    pub width: i64,
    #[serde(default)]
    pub height: i64,
    #[serde(default)]
    pub duration: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub file_id: String,
    #[serde(default)]
    pub file_unique_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileInfo {
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Bytes(Vec<u8>),
    TooBig,
}

fn safe_file_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() < 256
        && !path.starts_with('/')
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.' | b'-'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub kind: String,
    pub offset: i64,
    pub length: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_emoji_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: BotUser,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub data: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreCheckoutQuery {
    pub id: String,
    pub from: BotUser,
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetQueryWallet {
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetQueryKind {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet: Option<WidgetQueryWallet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetQuery {
    pub id: String,
    pub from: BotUser,
    #[serde(default)]
    pub chat: Option<Chat>,
    #[serde(default)]
    pub message_id: Option<i64>,
    #[serde(rename = "type")]
    pub kind: WidgetQueryKind,
    #[serde(default)]
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetMessage {
    pub html: String,
    pub chat_data: String,
    pub caption: String,
    pub keyboard: Keyboard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
    #[serde(default)]
    pub pre_checkout_query: Option<PreCheckoutQuery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget_query: Option<WidgetQuery>,
    #[serde(skip)]
    pub repeat: bool,
}

impl Update {
    pub fn sender(&self) -> Option<&BotUser> {
        if let Some(message) = &self.message {
            return message.from.as_ref();
        }
        if let Some(callback) = &self.callback_query {
            return Some(&callback.from);
        }
        if let Some(query) = &self.widget_query {
            return Some(&query.from);
        }
        self.pre_checkout_query.as_ref().map(|q| &q.from)
    }

    pub fn chat_key(&self) -> i64 {
        if let Some(message) = &self.message {
            return message.chat.id;
        }
        if let Some(message) = self
            .callback_query
            .as_ref()
            .and_then(|c| c.message.as_ref())
        {
            return message.chat.id;
        }
        self.sender().map_or(0, |u| u.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Button {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_custom_emoji_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_app: Option<WebApp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_text: Option<CopyText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebApp {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CopyText {
    pub text: String,
}

pub const COPY_TEXT_MAX: usize = 256;

impl Button {
    pub fn callback(text: impl Into<String>, data: impl Into<String>) -> Self {
        Button {
            text: text.into(),
            icon_custom_emoji_id: None,
            style: None,
            callback_data: Some(data.into()),
            url: None,
            web_app: None,
            copy_text: None,
        }
    }

    pub fn url(text: impl Into<String>, url: impl Into<String>) -> Self {
        Button {
            text: text.into(),
            icon_custom_emoji_id: None,
            style: None,
            callback_data: None,
            url: Some(url.into()),
            web_app: None,
            copy_text: None,
        }
    }

    pub fn web_app(text: impl Into<String>, url: impl Into<String>) -> Self {
        Button {
            text: text.into(),
            icon_custom_emoji_id: None,
            style: None,
            callback_data: None,
            url: None,
            web_app: Some(WebApp { url: url.into() }),
            copy_text: None,
        }
    }

    pub fn copy(text: impl Into<String>, value: impl Into<String>) -> Self {
        Button {
            text: text.into(),
            icon_custom_emoji_id: None,
            style: None,
            callback_data: None,
            url: None,
            web_app: None,
            copy_text: Some(CopyText { text: value.into() }),
        }
    }

    pub fn look(mut self, icon: Option<String>, style: Option<&str>) -> Self {
        self.icon_custom_emoji_id = icon;
        self.style = style.map(str::to_string);
        self
    }

    pub fn plain(mut self) -> Self {
        self.icon_custom_emoji_id = None;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Keyboard {
    pub inline_keyboard: Vec<Vec<Button>>,
}

impl Keyboard {
    pub fn new() -> Self {
        Keyboard::default()
    }

    pub fn row(mut self, buttons: Vec<Button>) -> Self {
        let buttons: Vec<Button> = buttons
            .into_iter()
            .filter(|b| {
                b.callback_data
                    .as_ref()
                    .is_none_or(|d| !d.is_empty() && d.len() <= CALLBACK_DATA_MAX)
            })
            .collect();
        if !buttons.is_empty() {
            self.inline_keyboard.push(buttons);
        }
        self
    }

    pub fn button(self, text: impl Into<String>, data: impl Into<String>) -> Self {
        self.row(vec![Button::callback(text, data)])
    }

    pub fn link(self, text: impl Into<String>, url: impl Into<String>) -> Self {
        self.row(vec![Button::url(text, url)])
    }

    pub fn is_empty(&self) -> bool {
        self.inline_keyboard.is_empty()
    }

    pub fn push(self, button: Option<Button>) -> Self {
        match button {
            Some(button) => self.row(vec![button]),
            None => self,
        }
    }

    pub fn pair(self, left: Option<Button>, right: Option<Button>) -> Self {
        self.row(left.into_iter().chain(right).collect())
    }

    pub fn rows(mut self, rows: Vec<Vec<Button>>) -> Self {
        for row in rows {
            self = self.row(row);
        }
        self
    }

    pub fn grid(mut self, buttons: Vec<Button>, columns: usize) -> Self {
        for chunk in buttons.chunks(columns.max(1)) {
            self = self.row(chunk.to_vec());
        }
        self
    }

    pub fn has_custom_emoji(&self) -> bool {
        self.inline_keyboard
            .iter()
            .flatten()
            .any(|b| b.icon_custom_emoji_id.is_some())
    }

    pub fn plain(&self) -> Keyboard {
        Keyboard {
            inline_keyboard: self
                .inline_keyboard
                .iter()
                .map(|row| row.iter().cloned().map(Button::plain).collect())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileUpload<'a> {
    pub bytes: &'a [u8],
    pub name: &'a str,
    pub mime: &'a str,
    pub photo: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum PhotoInput<'a> {
    FileId(&'a str),
    Upload {
        bytes: &'a [u8],
        content_type: &'a str,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum VideoInput<'a> {
    FileId(&'a str),
    Upload {
        bytes: &'a [u8],
        content_type: &'a str,
        info: sn_domain::video::VideoInfo,
    },
}

#[derive(Deserialize)]
struct ResponseParameters {
    #[serde(default)]
    retry_after: Option<u64>,
    #[serde(default)]
    migrate_to_chat_id: Option<i64>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    #[serde(default)]
    error_code: i64,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parameters: Option<ResponseParameters>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct WebhookInfo {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub pending_update_count: i64,
    #[serde(default)]
    pub last_error_date: Option<i64>,
    #[serde(default)]
    pub last_error_message: Option<String>,
    #[serde(default)]
    pub max_connections: Option<u32>,
    #[serde(default)]
    pub allowed_updates: Option<Vec<String>>,
}

pub fn parse_update(value: serde_json::Value) -> Option<Update> {
    let id = value.get("update_id").and_then(|v| v.as_i64());
    match serde_json::from_value::<Update>(value) {
        Ok(update) => Some(update),
        Err(err) => {
            tracing::warn!(update_id = ?id, error = %err, "апдейт Telegram не разобран");
            id.map(|update_id| Update {
                update_id,
                message: None,
                callback_query: None,
                pre_checkout_query: None,
                widget_query: None,
                repeat: false,
            })
        }
    }
}

pub struct TelegramApi {
    http: reqwest::Client,
    base: String,
    files: String,
    pacer: Option<Arc<Pacer>>,
    class: Class,
    clock: Option<Arc<ServerClock>>,
}

const PACED_RETRIES: u32 = 2;
const POLL_TIMEOUT: Duration = Duration::from_secs(65);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const UPLOAD_BYTES_PER_SEC: u64 = 256 * 1024;

fn upload_timeout(bytes: usize) -> Duration {
    UPLOAD_TIMEOUT
        + Duration::from_secs(u64::try_from(bytes).unwrap_or(u64::MAX) / UPLOAD_BYTES_PER_SEC)
}

fn http_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(65))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
}

pub type Proxy = Option<Arc<sn_egress::Proxy>>;

impl TelegramApi {
    pub fn new(api_base: &str, token: &str, proxy: Proxy) -> Self {
        TelegramApi {
            http: sn_egress::build(http_builder(), proxy.as_deref()),
            base: format!("{}/bot{}", api_base.trim_end_matches('/'), token),
            files: format!("{}/file/bot{}", api_base.trim_end_matches('/'), token),
            pacer: None,
            class: Class::Interactive,
            clock: None,
        }
    }

    pub fn paced(api_base: &str, token: &str, limits: SendLimits, proxy: Proxy) -> Self {
        TelegramApi {
            pacer: Some(crate::pacer::shared(api_base, token, limits)),
            ..TelegramApi::new(api_base, token, proxy)
        }
    }

    pub fn bulk(mut self) -> Self {
        self.class = Class::Bulk;
        self
    }

    pub fn timed(mut self, clock: Arc<ServerClock>) -> Self {
        self.clock = Some(clock);
        self
    }

    fn compare_clock(&self, response: &reqwest::Response) {
        let Some(clock) = &self.clock else { return };
        if let Some(date) = response
            .headers()
            .get(reqwest::header::DATE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| DateTime::parse_from_rfc2822(value.trim()).ok())
        {
            clock.answered(date.to_utc());
        }
    }

    pub fn pacer(&self) -> Option<&Arc<Pacer>> {
        self.pacer.as_ref()
    }

    pub async fn send_spare(&self, chat_id: i64, text: &str) -> Result<bool, TelegramError> {
        if let Some(pacer) = &self.pacer
            && !pacer.try_acquire(Some(chat_id), self.class)
        {
            return Ok(false);
        }
        self.request::<Message>(
            "sendMessage",
            &Self::message_body(chat_id, &strip_tags(text), None, false),
        )
        .await
        .map(|_| true)
    }

    pub async fn get_file(&self, file_id: &str) -> Result<FileInfo, TelegramError> {
        self.call("getFile", &serde_json::json!({ "file_id": file_id }))
            .await
    }

    pub async fn download(&self, path: &str, max_bytes: usize) -> Result<Fetched, TelegramError> {
        if !safe_file_path(path) {
            return Err(TelegramError::Decode("путь к файлу не подходит".into()));
        }
        let mut response = self
            .http
            .get(format!("{}/{path}", self.files))
            .send()
            .await
            .map_err(|e| TelegramError::Transport(e.without_url().to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(TelegramError::Api {
                code: i64::from(status.as_u16()),
                description: status.to_string(),
                retry_after: None,
                migrated: None,
            });
        }
        if response
            .content_length()
            .is_some_and(|bytes| bytes > max_bytes as u64)
        {
            return Ok(Fetched::TooBig);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| TelegramError::Transport(e.without_url().to_string()))?
        {
            if body.len() + chunk.len() > max_bytes {
                return Ok(Fetched::TooBig);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Fetched::Bytes(body))
    }

    pub async fn send_document(
        &self,
        chat_id: i64,
        upload: FileUpload<'_>,
        caption: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<Message, TelegramError> {
        let part = reqwest::multipart::Part::bytes(upload.bytes.to_vec())
            .file_name(upload.name.to_string())
            .mime_str(upload.mime)
            .map_err(|e| TelegramError::Transport(e.without_url().to_string()))?;
        let field = if upload.photo { "photo" } else { "document" };
        let mut form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part(field, part);
        if !caption.is_empty() {
            form = form
                .text("caption", caption.to_string())
                .text("parse_mode", "HTML");
        }
        if let Some(markup) = keyboard.filter(|k| !k.is_empty()) {
            form = form.text("reply_markup", serde_json::json!(markup).to_string());
        }
        let method = if upload.photo {
            "sendPhoto"
        } else {
            "sendDocument"
        };
        self.call_form(method, form, Some(chat_id)).await
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &serde_json::Value,
    ) -> Result<T, TelegramError> {
        let Some(pacer) = self.pacer.as_ref().filter(|_| sends(method)) else {
            return self.request(method, body).await;
        };
        let chat = body.get("chat_id").and_then(serde_json::Value::as_i64);
        let mut attempt = 0;
        loop {
            pacer.acquire(chat, self.class).await;
            let result = self.request(method, body).await;
            let wait = match &result {
                Err(err) if attempt < PACED_RETRIES => err.retry_after(),
                _ => None,
            };
            let Some(wait) = wait.map(Duration::from_secs) else {
                return result;
            };
            if wait > pacer.max_pause() {
                pacer.pause(wait);
                return result;
            }
            tracing::warn!(
                method,
                wait_secs = wait.as_secs(),
                "Telegram попросил подождать, повторим"
            );
            pacer.pause(wait);
            attempt += 1;
        }
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &serde_json::Value,
    ) -> Result<T, TelegramError> {
        let timeout = if method == "getUpdates" {
            POLL_TIMEOUT
        } else {
            CALL_TIMEOUT
        };
        let response = self
            .http
            .post(format!("{}/{method}", self.base))
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(|e| TelegramError::Transport(e.without_url().to_string()))?;
        self.compare_clock(&response);
        Self::parse(response).await
    }

    async fn call_form<T: DeserializeOwned>(
        &self,
        method: &str,
        form: reqwest::multipart::Form,
        chat: Option<i64>,
    ) -> Result<T, TelegramError> {
        self.call_form_within(method, form, chat, UPLOAD_TIMEOUT)
            .await
    }

    async fn call_form_within<T: DeserializeOwned>(
        &self,
        method: &str,
        form: reqwest::multipart::Form,
        chat: Option<i64>,
        timeout: Duration,
    ) -> Result<T, TelegramError> {
        let pacer = self.pacer.as_ref().filter(|_| sends(method));
        if let Some(pacer) = pacer {
            pacer.acquire(chat, self.class).await;
        }
        let response = self
            .http
            .post(format!("{}/{method}", self.base))
            .timeout(timeout)
            .multipart(form)
            .send()
            .await
            .map_err(|e| TelegramError::Transport(e.without_url().to_string()))?;
        self.compare_clock(&response);
        let result = Self::parse(response).await;
        if let (Some(pacer), Err(err)) = (pacer, &result)
            && let Some(wait) = err.retry_after()
        {
            tracing::warn!(
                method,
                wait_secs = wait,
                "Telegram попросил подождать с отправкой файла"
            );
            pacer.pause(Duration::from_secs(wait));
        }
        result
    }

    fn photo_part(
        bytes: &[u8],
        content_type: &str,
    ) -> Result<reqwest::multipart::Part, TelegramError> {
        let name = if content_type == "image/png" {
            "image.png"
        } else {
            "image.jpg"
        };
        reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(name)
            .mime_str(content_type)
            .map_err(|e| TelegramError::Transport(e.without_url().to_string()))
    }

    pub async fn send_photo(
        &self,
        chat_id: i64,
        photo: PhotoInput<'_>,
        caption: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<Message, TelegramError> {
        let markup = keyboard
            .filter(|k| !k.is_empty())
            .map(|k| serde_json::json!(k));
        match photo {
            PhotoInput::FileId(file_id) => {
                let mut body = serde_json::json!({
                    "chat_id": chat_id,
                    "photo": file_id,
                    "caption": caption,
                    "parse_mode": "HTML",
                });
                if let Some(markup) = markup {
                    body["reply_markup"] = markup;
                }
                self.call("sendPhoto", &body).await
            }
            PhotoInput::Upload {
                bytes,
                content_type,
            } => {
                let mut form = reqwest::multipart::Form::new()
                    .text("chat_id", chat_id.to_string())
                    .text("caption", caption.to_string())
                    .text("parse_mode", "HTML")
                    .part("photo", Self::photo_part(bytes, content_type)?);
                if let Some(markup) = markup {
                    form = form.text("reply_markup", markup.to_string());
                }
                self.call_form("sendPhoto", form, Some(chat_id)).await
            }
        }
    }

    pub async fn send_video(
        &self,
        chat_id: i64,
        video: VideoInput<'_>,
        caption: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<Message, TelegramError> {
        let markup = keyboard
            .filter(|k| !k.is_empty())
            .map(|k| serde_json::json!(k));
        match video {
            VideoInput::FileId(file_id) => {
                let mut body = serde_json::json!({
                    "chat_id": chat_id,
                    "video": file_id,
                    "caption": caption,
                    "parse_mode": "HTML",
                });
                if let Some(markup) = markup {
                    body["reply_markup"] = markup;
                }
                self.call("sendVideo", &body).await
            }
            VideoInput::Upload {
                bytes,
                content_type,
                info,
            } => {
                let part = reqwest::multipart::Part::bytes(bytes.to_vec())
                    .file_name("video.mp4")
                    .mime_str(content_type)
                    .map_err(|e| TelegramError::Transport(e.without_url().to_string()))?;
                let mut form = reqwest::multipart::Form::new()
                    .text("chat_id", chat_id.to_string())
                    .text("caption", caption.to_string())
                    .text("parse_mode", "HTML")
                    .part("video", part);
                if info.width > 0 && info.height > 0 {
                    form = form
                        .text("width", info.width.to_string())
                        .text("height", info.height.to_string());
                }
                if info.duration > 0 {
                    form = form.text("duration", info.duration.to_string());
                }
                if info.streaming {
                    form = form.text("supports_streaming", "true");
                }
                if let Some(markup) = markup {
                    form = form.text("reply_markup", markup.to_string());
                }
                self.call_form_within(
                    "sendVideo",
                    form,
                    Some(chat_id),
                    upload_timeout(bytes.len()),
                )
                .await
            }
        }
    }

    pub async fn edit_photo(
        &self,
        chat_id: i64,
        message_id: i64,
        photo: PhotoInput<'_>,
        caption: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<Option<Message>, TelegramError> {
        let media_ref = match photo {
            PhotoInput::FileId(file_id) => file_id.to_string(),
            PhotoInput::Upload { .. } => "attach://photo".to_string(),
        };
        let media = serde_json::json!({
            "type": "photo",
            "media": media_ref,
            "caption": caption,
            "parse_mode": "HTML",
        });
        let markup = keyboard
            .filter(|k| !k.is_empty())
            .map(|k| serde_json::json!(k));
        let result: Result<serde_json::Value, TelegramError> = match photo {
            PhotoInput::FileId(_) => {
                let mut body = serde_json::json!({
                    "chat_id": chat_id,
                    "message_id": message_id,
                    "media": media,
                });
                if let Some(markup) = markup {
                    body["reply_markup"] = markup;
                }
                self.call("editMessageMedia", &body).await
            }
            PhotoInput::Upload {
                bytes,
                content_type,
            } => {
                let mut form = reqwest::multipart::Form::new()
                    .text("chat_id", chat_id.to_string())
                    .text("message_id", message_id.to_string())
                    .text("media", media.to_string())
                    .part("photo", Self::photo_part(bytes, content_type)?);
                if let Some(markup) = markup {
                    form = form.text("reply_markup", markup.to_string());
                }
                self.call_form("editMessageMedia", form, Some(chat_id))
                    .await
            }
        };
        match result {
            Ok(value) => Ok(serde_json::from_value::<Message>(value).ok()),
            Err(err) if err.is_not_modified() => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub async fn delete_message(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<bool, TelegramError> {
        self.call(
            "deleteMessage",
            &serde_json::json!({ "chat_id": chat_id, "message_id": message_id }),
        )
        .await
    }

    async fn parse<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, TelegramError> {
        let status = response.status();
        let envelope: Envelope<T> = response.json().await.map_err(|e| {
            if status.is_server_error() {
                TelegramError::Api {
                    code: i64::from(status.as_u16()),
                    description: status.to_string(),
                    retry_after: None,
                    migrated: None,
                }
            } else {
                TelegramError::Decode(e.without_url().to_string())
            }
        })?;
        match (envelope.ok, envelope.result) {
            (true, Some(result)) => Ok(result),
            _ => {
                let parameters = envelope.parameters;
                Err(TelegramError::Api {
                    code: if envelope.error_code == 0 {
                        i64::from(status.as_u16())
                    } else {
                        envelope.error_code
                    },
                    description: envelope.description,
                    retry_after: parameters.as_ref().and_then(|p| p.retry_after),
                    migrated: parameters
                        .as_ref()
                        .and_then(|p| p.migrate_to_chat_id)
                        .map(Migrated),
                })
            }
        }
    }

    pub async fn get_me(&self) -> Result<BotUser, TelegramError> {
        self.call("getMe", &serde_json::json!({})).await
    }

    pub async fn get_webhook_info(&self) -> Result<WebhookInfo, TelegramError> {
        self.call("getWebhookInfo", &serde_json::json!({})).await
    }

    pub async fn set_webhook(
        &self,
        url: &str,
        secret_token: &str,
        max_connections: u32,
    ) -> Result<bool, TelegramError> {
        self.call(
            "setWebhook",
            &serde_json::json!({
                "url": url,
                "secret_token": secret_token,
                "max_connections": max_connections,
                "allowed_updates": ALLOWED_UPDATES,
                "drop_pending_updates": false,
            }),
        )
        .await
    }

    pub async fn delete_webhook(&self) -> Result<bool, TelegramError> {
        self.call(
            "deleteWebhook",
            &serde_json::json!({ "drop_pending_updates": false }),
        )
        .await
    }

    pub async fn set_menu_button(&self, button: &MenuButton) -> Result<bool, TelegramError> {
        self.call(
            "setChatMenuButton",
            &serde_json::json!({ "menu_button": button.json() }),
        )
        .await
    }

    pub async fn set_commands(&self, commands: &[(&str, &str)]) -> Result<bool, TelegramError> {
        self.set_commands_for(commands, None).await
    }

    pub async fn set_commands_for(
        &self,
        commands: &[(&str, &str)],
        language: Option<&str>,
    ) -> Result<bool, TelegramError> {
        let list: Vec<serde_json::Value> = commands
            .iter()
            .map(|(command, description)| {
                serde_json::json!({ "command": command, "description": description })
            })
            .collect();
        let mut body = serde_json::json!({ "commands": list });
        if let Some(language) = language {
            body["language_code"] = serde_json::json!(language);
        }
        self.call("setMyCommands", &body).await
    }

    pub async fn delete_commands_for(&self, language: &str) -> Result<bool, TelegramError> {
        self.call(
            "deleteMyCommands",
            &serde_json::json!({ "language_code": language }),
        )
        .await
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout: Duration,
    ) -> Result<Vec<Update>, TelegramError> {
        let raw: Vec<serde_json::Value> = self
            .call(
                "getUpdates",
                &serde_json::json!({
                    "offset": offset,
                    "timeout": timeout.as_secs(),
                    "allowed_updates": ALLOWED_UPDATES,
                }),
            )
            .await?;
        Ok(raw.into_iter().filter_map(parse_update).collect())
    }

    fn message_body(
        chat_id: i64,
        text: &str,
        keyboard: Option<&Keyboard>,
        html: bool,
    ) -> serde_json::Value {
        let text = if html {
            clip_markup(text, MESSAGE_MAX).into_owned()
        } else {
            truncate(text, MESSAGE_MAX)
        };
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "link_preview_options": { "is_disabled": true },
        });
        if html {
            body["parse_mode"] = serde_json::json!("HTML");
        }
        if let Some(keyboard) = keyboard.filter(|k| !k.is_empty()) {
            body["reply_markup"] = serde_json::json!(keyboard);
        }
        body
    }

    pub async fn send_to_thread(
        &self,
        chat_id: i64,
        thread: Option<i64>,
        text: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<Message, TelegramError> {
        let with_thread = |mut body: serde_json::Value| {
            if let Some(thread) = thread {
                body["message_thread_id"] = serde_json::json!(thread);
            }
            body
        };
        match self
            .call(
                "sendMessage",
                &with_thread(Self::message_body(chat_id, text, keyboard, true)),
            )
            .await
        {
            Err(err) if err.is_parse_error() => {
                self.call(
                    "sendMessage",
                    &with_thread(Self::message_body(
                        chat_id,
                        &strip_tags(text),
                        keyboard,
                        false,
                    )),
                )
                .await
            }
            other => other,
        }
    }

    pub async fn create_forum_topic(
        &self,
        chat_id: i64,
        name: &str,
    ) -> Result<ForumTopic, TelegramError> {
        self.call(
            "createForumTopic",
            &serde_json::json!({ "chat_id": chat_id, "name": name }),
        )
        .await
    }

    pub async fn get_chat(&self, chat_id: i64) -> Result<Chat, TelegramError> {
        self.call("getChat", &serde_json::json!({ "chat_id": chat_id }))
            .await
    }

    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<Message, TelegramError> {
        match self
            .call(
                "sendMessage",
                &Self::message_body(chat_id, text, keyboard, true),
            )
            .await
        {
            Err(err) if err.is_parse_error() => {
                self.call(
                    "sendMessage",
                    &Self::message_body(chat_id, &strip_tags(text), keyboard, false),
                )
                .await
            }
            other => other,
        }
    }

    pub async fn edit_message(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<(), TelegramError> {
        let mut body = Self::message_body(chat_id, text, keyboard, true);
        body["message_id"] = serde_json::json!(message_id);
        match self
            .call::<serde_json::Value>("editMessageText", &body)
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.is_not_modified() => Ok(()),
            Err(err) => Err(err),
        }
    }

    pub async fn answer_callback(
        &self,
        callback_id: &str,
        text: Option<&str>,
    ) -> Result<(), TelegramError> {
        let mut body = serde_json::json!({ "callback_query_id": callback_id });
        if let Some(text) = text {
            body["text"] = serde_json::json!(truncate(text, 200));
        }
        self.call::<bool>("answerCallbackQuery", &body)
            .await
            .map(|_| ())
    }

    pub async fn answer_alert(&self, callback_id: &str, text: &str) -> Result<(), TelegramError> {
        let body = serde_json::json!({
            "callback_query_id": callback_id,
            "text": truncate(text, 200),
            "show_alert": true,
        });
        self.call::<bool>("answerCallbackQuery", &body)
            .await
            .map(|_| ())
    }

    fn widget_body(chat_id: i64, widget: &WidgetMessage, html: bool) -> serde_json::Value {
        let caption = if html {
            truncate(&widget.caption, 1_024)
        } else {
            truncate(&strip_tags(&widget.caption), 1_024)
        };
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "html": widget.html,
            "chat_data": widget.chat_data,
            "caption": caption,
            "allow_queries": true,
        });
        if html {
            body["parse_mode"] = serde_json::json!("HTML");
        }
        if !widget.keyboard.is_empty() {
            body["reply_markup"] = serde_json::json!(widget.keyboard);
        }
        body
    }

    pub async fn send_widget(
        &self,
        chat_id: i64,
        widget: &WidgetMessage,
    ) -> Result<Message, TelegramError> {
        match self
            .call("sendWidget", &Self::widget_body(chat_id, widget, true))
            .await
        {
            Err(err) if err.is_parse_error() => {
                self.call("sendWidget", &Self::widget_body(chat_id, widget, false))
                    .await
            }
            other => other,
        }
    }

    pub async fn answer_widget_query(
        &self,
        query_id: &str,
        data: &str,
    ) -> Result<(), TelegramError> {
        self.call::<bool>(
            "answerWidgetQuery",
            &serde_json::json!({ "widget_query_id": query_id, "data": data }),
        )
        .await
        .map(|_| ())
    }

    pub async fn edit_widget_data(
        &self,
        chat_id: i64,
        message_id: i64,
        chat_data: &str,
    ) -> Result<(), TelegramError> {
        self.call::<bool>(
            "editMessageWidgetData",
            &serde_json::json!({ "chat_id": chat_id, "message_id": message_id, "chat_data": chat_data }),
        )
        .await
        .map(|_| ())
    }

    pub async fn answer_pre_checkout(
        &self,
        query_id: &str,
        error: Option<&str>,
    ) -> Result<(), TelegramError> {
        let body = match error {
            None => serde_json::json!({ "pre_checkout_query_id": query_id, "ok": true }),
            Some(message) => serde_json::json!({
                "pre_checkout_query_id": query_id,
                "ok": false,
                "error_message": truncate(message, 200),
            }),
        };
        self.call::<bool>("answerPreCheckoutQuery", &body)
            .await
            .map(|_| ())
    }
}

pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

pub fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_services::server_clock::Evidence;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "123456:SECRET-token";

    #[tokio::test]
    async fn ошибка_сети_не_раскрывает_токен() {
        let api = TelegramApi::new("http://127.0.0.1:9", TOKEN, None);
        let err = api.get_me().await.unwrap_err();
        assert!(matches!(err, TelegramError::Transport(_)));
        assert!(!err.to_string().contains("SECRET"));
        assert!(err.is_retryable());
    }

    fn hours_ahead(hours: i64) -> Arc<ServerClock> {
        let now = chrono::Utc::now() + chrono::Duration::hours(hours);
        Arc::new(ServerClock::new(Arc::new(sn_services::FixedClock::new(
            now,
        ))))
    }

    #[tokio::test]
    async fn часы_сервера_сверяются_с_ответами_телеграма() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/getMe")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"id": 1, "is_bot": true, "first_name": "Бот"}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendMessage")))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "ok": false, "error_code": 403, "description": "Forbidden: bot was blocked by the user"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendDocument")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"message_id": 1, "chat": {"id": 1, "type": "private"}}
            })))
            .mount(&server)
            .await;
        let exact = hours_ahead(0);
        let api = TelegramApi::new(&server.uri(), TOKEN, None).timed(exact.clone());
        api.get_me().await.unwrap();
        api.get_me().await.unwrap();
        assert_eq!(exact.drift(), None);

        let fast = hours_ahead(2);
        let api = TelegramApi::new(&server.uri(), TOKEN, None).timed(fast.clone());
        api.get_me().await.unwrap();
        assert_eq!(fast.drift(), None, "одного ответа мало");
        assert!(api.send_message(1, "текст", None).await.is_err());
        let drift = fast.drift().expect("отказ Telegram тоже несёт время");
        assert!((drift.seconds - 7_200).abs() <= 5, "{drift:?}");
        assert_eq!(drift.evidence, Evidence::Telegram);

        let slow = hours_ahead(-2);
        let api = TelegramApi::new(&server.uri(), TOKEN, None).timed(slow.clone());
        let upload = FileUpload {
            bytes: b"report",
            name: "report.txt",
            mime: "text/plain",
            photo: false,
        };
        api.send_document(1, upload, "", None).await.unwrap();
        api.send_document(1, upload, "", None).await.unwrap();
        let drift = slow.drift().expect("отправка файла тоже сверяет время");
        assert!((drift.seconds + 7_200).abs() <= 5, "{drift:?}");
    }

    #[tokio::test]
    async fn ответ_без_даты_или_с_мусором_о_часах_ничего_не_говорит() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/getMe")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("date", "вчера")
                    .set_body_json(serde_json::json!({
                        "ok": true, "result": {"id": 1, "is_bot": true, "first_name": "Бот"}
                    })),
            )
            .mount(&server)
            .await;
        let fast = hours_ahead(2);
        let api = TelegramApi::new(&server.uri(), TOKEN, None).timed(fast.clone());
        api.get_me().await.unwrap();
        api.get_me().await.unwrap();
        assert_eq!(fast.drift(), None);
    }

    #[tokio::test]
    async fn отказ_телеграма_разбирается_с_паузой() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendMessage")))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "ok": false, "error_code": 429, "description": "Too Many Requests: retry after 7",
                "parameters": {"retry_after": 7}
            })))
            .mount(&server)
            .await;
        let api = TelegramApi::new(&server.uri(), TOKEN, None);
        let err = api.send_message(1, "текст", None).await.unwrap_err();
        assert_eq!(err.retry_after(), Some(7));
        assert!(err.is_retryable());
        assert!(!err.is_forbidden());
    }

    #[tokio::test]
    async fn отказ_429_на_отправке_файла_ставит_общую_паузу() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendDocument")))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "ok": false, "error_code": 429, "description": "Too Many Requests: retry after 7",
                "parameters": {"retry_after": 7}
            })))
            .mount(&server)
            .await;
        let upload = FileUpload {
            bytes: b"report",
            name: "report.txt",
            mime: "text/plain",
            photo: false,
        };
        let plain = TelegramApi::new(&server.uri(), TOKEN, None);
        let err = plain.send_document(1, upload, "", None).await.unwrap_err();
        assert_eq!(err.retry_after(), Some(7));
        let api = TelegramApi::paced(&server.uri(), TOKEN, SendLimits::default(), None);
        let pacer = api.pacer().expect("у отправки есть ограничитель").clone();
        assert_eq!(pacer.stats().pauses, 0);
        let err = api.send_document(1, upload, "", None).await.unwrap_err();
        assert_eq!(err.retry_after(), Some(7));
        assert_eq!(
            pacer.stats().pauses,
            1,
            "отказ на загрузке файла останавливает всю отправку"
        );
    }

    #[tokio::test]
    async fn неразобранная_разметка_отправляется_простым_текстом() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendMessage")))
            .and(body_partial_json(serde_json::json!({"parse_mode": "HTML"})))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "ok": false, "error_code": 400, "description": "Bad Request: can't parse entities"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendMessage")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"message_id": 5, "chat": {"id": 1, "type": "private"}, "date": 0}
            })))
            .mount(&server)
            .await;
        let api = TelegramApi::new(&server.uri(), TOKEN, None);
        let sent = api
            .send_message(1, "<b>Привет</b> &amp; пока", None)
            .await
            .unwrap();
        assert_eq!(sent.message_id, 5);
        let requests = server.received_requests().await.unwrap();
        let plain: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(plain["text"], "Привет & пока");
        assert!(plain.get("parse_mode").is_none());
    }

    #[test]
    fn апдейты_разбираются_в_типы() {
        let update: Update = serde_json::from_value(serde_json::json!({
            "update_id": 10,
            "callback_query": {
                "id": "cb1",
                "from": {"id": 42, "is_bot": false, "first_name": "Алексей", "username": "morozov_a"},
                "message": {"message_id": 7, "chat": {"id": 42, "type": "private"}, "date": 1},
                "data": "m"
            }
        }))
        .unwrap();
        assert_eq!(update.chat_key(), 42);
        assert_eq!(update.sender().map(|u| u.id), Some(42));
        let pre: Update = serde_json::from_value(serde_json::json!({
            "update_id": 11,
            "pre_checkout_query": {
                "id": "q1", "from": {"id": 43, "first_name": "Ксения"}, "currency": "XTR",
                "total_amount": 350, "invoice_payload": "00000000-0000-0000-0000-000000000000"
            }
        }))
        .unwrap();
        assert_eq!(pre.chat_key(), 43);
    }

    #[test]
    fn длинное_сообщение_с_разметкой_обрезается_без_разрыва_тегов() {
        let long = format!("<b>Заголовок</b>\n<i>{}</i> &amp; хвост", "ж".repeat(5_000));
        let body = TelegramApi::message_body(7, &long, None, true);
        let text = body["text"].as_str().unwrap();
        assert!(text.starts_with("<b>Заголовок</b>\n<i>жжж"));
        assert!(text.ends_with("ж…</i>"));
        assert_eq!(sn_domain::bot_content::visible_len(text), MESSAGE_MAX);
        assert!(sn_domain::bot_content::normalize_markup(text).is_ok());
        let short = "<b>Тариф:</b> 30 дней";
        assert_eq!(
            TelegramApi::message_body(7, short, None, true)["text"],
            short
        );
        let plain = "ж".repeat(5_000);
        let body = TelegramApi::message_body(7, &plain, None, false);
        assert_eq!(body["text"].as_str().unwrap().chars().count(), MESSAGE_MAX);
    }

    #[test]
    fn кнопка_с_длинными_данными_не_попадает_в_клавиатуру() {
        let keyboard = Keyboard::new()
            .button("Ок", "m")
            .button("Длинно", "x".repeat(65))
            .link("Оплатить", "https://example.com/pay");
        assert_eq!(keyboard.inline_keyboard.len(), 2);
        assert_eq!(truncate("абвгд", 3), "аб…");
    }
}
