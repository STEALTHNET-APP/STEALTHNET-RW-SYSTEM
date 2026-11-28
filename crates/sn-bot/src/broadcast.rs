use crate::telegram::{
    Button, Keyboard, Message, PhotoInput, TelegramApi, TelegramError, VideoInput, strip_tags,
};
use crate::text::escape;
use async_trait::async_trait;
use sn_domain::bot_content::strip_custom_emoji;
use sn_modules::{BotSender, Delivery, Media, MediaFile, OutButton, Outgoing};
use sn_services::Services;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Cached = (String, crate::telegram::Proxy, Arc<TelegramApi>);

pub struct TelegramSender {
    services: Services,
    api_base: String,
    cache: Mutex<Option<Cached>>,
}

impl TelegramSender {
    pub fn new(services: Services, api_base: &str) -> Self {
        TelegramSender {
            services,
            api_base: api_base.to_string(),
            cache: Mutex::new(None),
        }
    }

    fn api(&self, token: &str) -> Arc<TelegramApi> {
        let proxy = self
            .services
            .egress()
            .proxy_for(sn_egress::Route::Telegram, None);
        let mut cache = match self.cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((cached, via, api)) = cache.as_ref()
            && cached == token
            && *via == proxy
        {
            return api.clone();
        }
        let api = Arc::new(
            TelegramApi::paced(
                &self.api_base,
                token,
                crate::SendLimits::default(),
                proxy.clone(),
            )
            .bulk(),
        );
        *cache = Some((token.to_string(), proxy, api.clone()));
        api
    }
}

pub fn keyboard(buttons: &[OutButton]) -> Keyboard {
    buttons.iter().fold(Keyboard::new(), |keyboard, button| {
        let made = match (&button.url, &button.callback) {
            (Some(url), _) => Some(Button::url(&button.text, url)),
            (None, Some(data)) => Some(Button::callback(&button.text, data)),
            (None, None) => None,
        };
        keyboard.push(made.map(|made| made.look(button.emoji_id.clone(), button.style.as_deref())))
    })
}

fn bot_of(token: &str) -> Option<i64> {
    token.split(':').next()?.parse::<i64>().ok()
}

async fn transmit(
    api: &TelegramApi,
    chat_id: i64,
    html: &str,
    media: Option<Media<'_>>,
    keyboard: &Keyboard,
    markup: bool,
) -> Result<Message, TelegramError> {
    let caption = || {
        if markup {
            html.to_string()
        } else {
            escape(html)
        }
    };
    match media {
        None => {
            let mut body = serde_json::json!({
                "chat_id": chat_id,
                "text": html,
                "link_preview_options": { "is_disabled": true },
            });
            if markup {
                body["parse_mode"] = serde_json::json!("HTML");
            }
            if !keyboard.is_empty() {
                body["reply_markup"] = serde_json::json!(keyboard);
            }
            api.call("sendMessage", &body).await
        }
        Some(Media::Photo(file)) => {
            let input = match file {
                MediaFile::Cached(file_id) => PhotoInput::FileId(file_id),
                MediaFile::Upload {
                    bytes,
                    content_type,
                } => PhotoInput::Upload {
                    bytes,
                    content_type,
                },
            };
            api.send_photo(chat_id, input, &caption(), Some(keyboard))
                .await
        }
        Some(Media::Video(file, info)) => {
            let input = match file {
                MediaFile::Cached(file_id) => VideoInput::FileId(file_id),
                MediaFile::Upload {
                    bytes,
                    content_type,
                } => VideoInput::Upload {
                    bytes,
                    content_type,
                    info,
                },
            };
            api.send_video(chat_id, input, &caption(), Some(keyboard))
                .await
        }
    }
}

fn description(err: &TelegramError) -> &str {
    match err {
        TelegramError::Api { description, .. } => description,
        _ => "",
    }
}

fn chat_gone(err: &TelegramError) -> Option<&'static str> {
    let text = description(err).to_ascii_lowercase();
    match err {
        TelegramError::Api { code: 403, .. } if text.contains("deactivated") => {
            Some("Аккаунт Telegram удалён")
        }
        TelegramError::Api { code: 403, .. } => Some("Клиент остановил бота"),
        TelegramError::Api { code: 400, .. }
            if text.contains("chat not found")
                || text.contains("user not found")
                || text.contains("peer_id_invalid") =>
        {
            Some("Чат с клиентом не найден")
        }
        _ => None,
    }
}

fn stale_file(err: &TelegramError) -> bool {
    let text = description(err).to_ascii_lowercase();
    matches!(err, TelegramError::Api { code: 400, .. })
        && (text.contains("file identifier")
            || text.contains("file reference")
            || text.contains("remote file")
            || text.contains("wrong file"))
}

fn readable(err: &TelegramError) -> String {
    let text = description(err).to_ascii_lowercase();
    if text.contains("button_url_invalid") {
        "Telegram не принял адрес ссылки в кнопке".into()
    } else if text.contains("message is too long") || text.contains("message_too_long") {
        "Текст слишком длинный для Telegram".into()
    } else if text.contains("caption is too long") || text.contains("caption_too_long") {
        "Подпись к картинке или видео слишком длинная для Telegram".into()
    } else if text.contains("photo_invalid") || text.contains("image_process_failed") {
        "Telegram не принял картинку".into()
    } else if text.contains("too big") || text.contains("too large") {
        "Telegram не принял файл: он слишком большой".into()
    } else {
        match err {
            TelegramError::Transport(_) => "Нет связи с Telegram".into(),
            TelegramError::Decode(_) => "Telegram ответил непонятно".into(),
            TelegramError::Api { code, .. } if *code >= 500 => {
                format!("Telegram не ответил: ошибка {code}")
            }
            _ => err.to_string(),
        }
    }
}

fn delivered(message: Message, media: Option<Media<'_>>) -> Delivery {
    let file_id = match media {
        Some(Media::Photo(MediaFile::Upload { .. })) => {
            message.largest_photo().map(|p| p.file_id.clone())
        }
        Some(Media::Video(MediaFile::Upload { .. }, _)) => {
            if message.video.is_none() {
                tracing::warn!(
                    "рассылка: Telegram принял видео не как видео, код файла не запоминаем"
                );
            }
            message.video.map(|video| video.file_id)
        }
        _ => None,
    };
    Delivery::Sent { file_id }
}

fn classify(err: &TelegramError, cached: bool) -> Delivery {
    if let Some(seconds) = err.retry_after() {
        return Delivery::Wait(Duration::from_secs(seconds.max(1)));
    }
    if let Some(reason) = chat_gone(err) {
        return Delivery::Blocked(reason.into());
    }
    if cached && stale_file(err) {
        return Delivery::StaleFile;
    }
    match err {
        TelegramError::Api {
            code: 401 | 404, ..
        } => Delivery::NoBot("Telegram не принял токен бота".into()),
        _ if err.is_retryable() => Delivery::Failed {
            error: readable(err),
            retry: true,
        },
        _ => Delivery::Failed {
            error: readable(err),
            retry: false,
        },
    }
}

#[async_trait]
impl BotSender for TelegramSender {
    fn bot_id(&self) -> Option<i64> {
        self.services.bot_token().as_deref().and_then(bot_of)
    }

    async fn send(&self, chat_id: i64, message: &Outgoing<'_>) -> Delivery {
        let Some(token) = self.services.bot_token() else {
            return Delivery::NoBot("не задан токен".into());
        };
        let api = self.api(&token);
        let keyboard = keyboard(message.buttons);
        let cached = message.media.is_some_and(|media| media.cached());
        let first = transmit(&api, chat_id, message.html, message.media, &keyboard, true).await;
        let err = match first {
            Ok(sent) => return delivered(sent, message.media),
            Err(err) => err,
        };
        let plain_emoji = (message.html.contains("<tg-emoji") || keyboard.has_custom_emoji())
            && matches!(err, TelegramError::Api { code: 400, .. })
            && chat_gone(&err).is_none()
            && !stale_file(&err);
        let retried = if plain_emoji {
            tracing::info!(error = %err, "рассылка: Telegram не принял премиум-эмодзи, отправляем без них");
            Some(
                transmit(
                    &api,
                    chat_id,
                    &strip_custom_emoji(message.html),
                    message.media,
                    &keyboard.plain(),
                    true,
                )
                .await,
            )
        } else if err.is_parse_error() {
            Some(
                transmit(
                    &api,
                    chat_id,
                    &strip_tags(message.html),
                    message.media,
                    &keyboard,
                    false,
                )
                .await,
            )
        } else {
            None
        };
        match retried {
            Some(Ok(sent)) => delivered(sent, message.media),
            Some(Err(again)) => classify(&again, cached),
            None => classify(&err, cached),
        }
    }
}
