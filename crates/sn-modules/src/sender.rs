use async_trait::async_trait;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Photo<'a> {
    Cached(&'a str),
    Upload {
        bytes: &'a [u8],
        content_type: &'a str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outgoing<'a> {
    pub html: &'a str,
    pub photo: Option<Photo<'a>>,
    pub buttons: &'a [OutButton],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Sent { file_id: Option<String> },
    Blocked(String),
    Wait(Duration),
    StaleFile,
    Failed { error: String, retry: bool },
    NoBot(String),
}

impl Delivery {
    pub fn is_sent(&self) -> bool {
        matches!(self, Delivery::Sent { .. })
    }
}

#[async_trait]
pub trait BotSender: Send + Sync {
    fn bot_id(&self) -> Option<i64>;

    async fn send(&self, chat_id: i64, message: &Outgoing<'_>) -> Delivery;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailSite {
    pub base_url: String,
    pub brand: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailOut<'a> {
    pub to: &'a str,
    pub subject: &'a str,
    pub text: &'a str,
    pub html: &'a str,
    pub unsubscribe: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailDelivery {
    Sent,
    Wait(Duration),
    Rejected(String),
    Failed { error: String, retry: bool },
    Off(String),
}

pub const NO_SITE_ADDRESS: &str = "у панели не задан адрес сервиса, а без него в письме не будет ссылки «Отписаться». Адрес задаётся на сервере командой stealthnet domain";

#[async_trait]
pub trait MailSender: Send + Sync {
    fn site(&self) -> Result<MailSite, String>;

    async fn send(&self, mail: &MailOut<'_>) -> MailDelivery;
}
