use crate::BotError;
use crate::telegram::{TelegramApi, TelegramError};
use serde::Deserialize;
use sn_services::Services;
use sn_store::{Db, NewPhoto, PHOTO_MAX_BYTES, PhotoTarget};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const REFRESH_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
pub const ROUND_EVERY: Duration = Duration::from_secs(15);
const BATCH: i64 = 10;
const RETRY_LATER: Duration = Duration::from_secs(15 * 60);
const WANTED_SIDE: i64 = 320;

#[derive(Debug, Clone, Deserialize)]
struct PhotoSize {
    file_id: String,
    file_unique_id: String,
    width: i64,
    height: i64,
    #[serde(default)]
    file_size: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct ProfilePhotos {
    #[serde(default)]
    photos: Vec<Vec<PhotoSize>>,
}

#[derive(Debug, Deserialize)]
struct TelegramFile {
    #[serde(default)]
    file_path: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Saved {
        version: String,
        content: Vec<u8>,
        content_type: &'static str,
    },
    Kept,
    Missing,
    Later(Duration),
    Stop,
}

pub struct PhotoKeeper {
    services: Services,
    api_base: String,
    every: Duration,
}

fn pick(sizes: Vec<PhotoSize>) -> Option<PhotoSize> {
    let area = |s: &PhotoSize| s.width.saturating_mul(s.height);
    let enough = sizes
        .iter()
        .filter(|s| s.width.min(s.height) >= WANTED_SIDE)
        .min_by_key(|s| area(s))
        .cloned();
    enough.or_else(|| sizes.into_iter().max_by_key(area))
}

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() < 256
        && !path.starts_with('/')
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.' | b'-'))
}

pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn failure(err: &TelegramError) -> Outcome {
    match err {
        TelegramError::Api { code: 401, .. } => Outcome::Stop,
        TelegramError::Api { code: 429, .. } => {
            Outcome::Later(Duration::from_secs(err.retry_after().unwrap_or(60).max(1)))
        }
        TelegramError::Api { code, .. } if (400..500).contains(code) => Outcome::Missing,
        _ => Outcome::Later(RETRY_LATER),
    }
}

impl PhotoKeeper {
    pub fn new(services: Services, api_base: &str, every: Duration) -> Self {
        PhotoKeeper {
            services,
            api_base: api_base.trim_end_matches('/').to_string(),
            every,
        }
    }

    pub async fn run(self, cancel: CancellationToken) {
        loop {
            if !self.services.pause_gate().is_paused()
                && let Some(token) = self.services.bot_token()
                && let Err(err) = self.round(&token).await
            {
                tracing::debug!(error = %err, "фото клиентов не обновлены");
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(self.every) => {}
            }
        }
    }

    pub async fn round(&self, token: &str) -> Result<usize, BotError> {
        let due = {
            let mut conn = self.services.store().conn().await?;
            conn.client_photos().due(BATCH).await?
        };
        if due.is_empty() {
            return Ok(0);
        }
        let proxy = self.services.telegram_proxy();
        let api = TelegramApi::new(&self.api_base, token, proxy.as_deref());
        let http = crate::telegram::http_client(proxy.as_deref(), Duration::from_secs(20));
        let mut done = 0;
        for target in due {
            let outcome = self.fetch(&api, &http, token, &target).await;
            if outcome == Outcome::Stop {
                break;
            }
            self.record(&target, outcome).await?;
            done += 1;
        }
        Ok(done)
    }

    async fn fetch(
        &self,
        api: &TelegramApi,
        http: &reqwest::Client,
        token: &str,
        target: &PhotoTarget,
    ) -> Outcome {
        let photos: ProfilePhotos = match api
            .call(
                "getUserProfilePhotos",
                &serde_json::json!({ "user_id": target.telegram_id, "limit": 1 }),
            )
            .await
        {
            Ok(photos) => photos,
            Err(err) => return failure(&err),
        };
        let Some(size) = pick(photos.photos.into_iter().next().unwrap_or_default()) else {
            return Outcome::Missing;
        };
        if target.known.as_deref() == Some(size.file_unique_id.as_str()) {
            return Outcome::Kept;
        }
        if size
            .file_size
            .is_some_and(|bytes| bytes > PHOTO_MAX_BYTES as u64)
        {
            return Outcome::Missing;
        }
        let file: TelegramFile = match api
            .call("getFile", &serde_json::json!({ "file_id": size.file_id }))
            .await
        {
            Ok(file) => file,
            Err(err) => return failure(&err),
        };
        let Some(path) = file.file_path.filter(|p| safe_path(p)) else {
            return Outcome::Missing;
        };
        match self.download(http, token, &path).await {
            Ok(content) => match sniff(&content) {
                Some(content_type) => Outcome::Saved {
                    version: size.file_unique_id,
                    content,
                    content_type,
                },
                None => Outcome::Missing,
            },
            Err(outcome) => outcome,
        }
    }

    async fn download(
        &self,
        http: &reqwest::Client,
        token: &str,
        path: &str,
    ) -> Result<Vec<u8>, Outcome> {
        let url = format!("{}/file/bot{token}/{path}", self.api_base);
        let mut response = http
            .get(url)
            .send()
            .await
            .map_err(|_| Outcome::Later(RETRY_LATER))?;
        let status = response.status();
        if !status.is_success() {
            return Err(if status.is_server_error() || status.as_u16() == 429 {
                Outcome::Later(RETRY_LATER)
            } else {
                Outcome::Missing
            });
        }
        if response
            .content_length()
            .is_some_and(|bytes| bytes > PHOTO_MAX_BYTES as u64)
        {
            return Err(Outcome::Missing);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Outcome::Later(RETRY_LATER))?
        {
            if body.len() + chunk.len() > PHOTO_MAX_BYTES {
                return Err(Outcome::Missing);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn record(&self, target: &PhotoTarget, outcome: Outcome) -> Result<(), BotError> {
        let mut conn = self.services.store().conn().await?;
        let mut photos = conn.client_photos();
        match outcome {
            Outcome::Saved {
                version,
                content,
                content_type,
            } => {
                photos
                    .save(
                        target.client_id,
                        Some(NewPhoto {
                            version: &version,
                            content: &content,
                            content_type,
                        }),
                    )
                    .await?
            }
            Outcome::Kept => photos.keep(target.client_id).await?,
            Outcome::Missing => photos.save(target.client_id, None).await?,
            Outcome::Later(delay) => photos.postpone(target.client_id, delay).await?,
            Outcome::Stop => {}
        }
        Ok(())
    }
}
