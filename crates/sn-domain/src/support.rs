use crate::DomainError;
use crate::ids::{AdminId, ClientId, TicketFileId, TicketId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_OPEN_TICKETS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketChannel {
    Bot,
    Cabinet,
    Miniapp,
    Unknown,
}

impl TicketChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            TicketChannel::Bot => "bot",
            TicketChannel::Cabinet => "cabinet",
            TicketChannel::Miniapp => "miniapp",
            TicketChannel::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "bot" => Ok(TicketChannel::Bot),
            "cabinet" => Ok(TicketChannel::Cabinet),
            "miniapp" => Ok(TicketChannel::Miniapp),
            "unknown" => Ok(TicketChannel::Unknown),
            other => Err(DomainError::unknown("ticket_channel", other)),
        }
    }

    pub const fn reply_targets(self) -> &'static [ReplyTarget] {
        match self {
            TicketChannel::Bot => &[ReplyTarget::Telegram],
            TicketChannel::Cabinet | TicketChannel::Miniapp => {
                &[ReplyTarget::Cabinet, ReplyTarget::Email]
            }
            TicketChannel::Unknown => &[ReplyTarget::Cabinet, ReplyTarget::Telegram],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyTarget {
    Telegram,
    Cabinet,
    Email,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageAuthor {
    Client,
    Support,
    Ai,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiState {
    Active,
    HandedOff,
}

impl AiState {
    pub const fn as_str(self) -> &'static str {
        match self {
            AiState::Active => "active",
            AiState::HandedOff => "handed_off",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "active" => Ok(AiState::Active),
            "handed_off" => Ok(AiState::HandedOff),
            other => Err(DomainError::unknown("ai_state", other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffReason {
    Human,
    Money,
    Unsure,
    #[serde(alias = "groq")]
    NoAnswer,
    ClientLimit,
    Budget,
    Operator,
    Disabled,
}

impl HandoffReason {
    pub const ALL: [HandoffReason; 8] = [
        HandoffReason::Human,
        HandoffReason::Money,
        HandoffReason::Unsure,
        HandoffReason::NoAnswer,
        HandoffReason::ClientLimit,
        HandoffReason::Budget,
        HandoffReason::Operator,
        HandoffReason::Disabled,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            HandoffReason::Human => "human",
            HandoffReason::Money => "money",
            HandoffReason::Unsure => "unsure",
            HandoffReason::NoAnswer => "no_answer",
            HandoffReason::ClientLimit => "client_limit",
            HandoffReason::Budget => "budget",
            HandoffReason::Operator => "operator",
            HandoffReason::Disabled => "disabled",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        if value == "groq" {
            return Ok(HandoffReason::NoAnswer);
        }
        HandoffReason::ALL
            .into_iter()
            .find(|r| r.as_str() == value)
            .ok_or_else(|| DomainError::unknown("handoff_reason", value))
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            HandoffReason::Human => "клиент попросил оператора",
            HandoffReason::Money => "вопрос про оплату, деньги или возврат",
            HandoffReason::Unsure => "ИИ не уверен в ответе",
            HandoffReason::NoAnswer => "ИИ не ответил",
            HandoffReason::ClientLimit => "клиент исчерпал ответы ИИ на сегодня",
            HandoffReason::Budget => "дневной бюджет ИИ закончился",
            HandoffReason::Operator => "оператор ответил сам",
            HandoffReason::Disabled => "ИИ-чат выключен",
        }
    }

    pub const fn alerts(self) -> bool {
        !matches!(self, HandoffReason::Operator | HandoffReason::Disabled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketAi {
    pub state: AiState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<HandoffReason>,
    pub handoff_at: Option<DateTime<Utc>>,
}

impl TicketAi {
    pub const fn answering(&self) -> bool {
        matches!(self.state, AiState::Active)
    }

    pub const fn for_client(self) -> Self {
        TicketAi {
            state: self.state,
            reason: None,
            handoff_at: self.handoff_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketFilter {
    Waiting,
    Answered,
    Closed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketOrder {
    #[default]
    Recent,
    Longest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub id: TicketId,
    pub client_id: ClientId,
    pub status: TicketStatus,
    pub channel: TicketChannel,
    pub subject: Option<String>,
    pub last_author: MessageAuthor,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Ticket {
    pub const fn filter(&self) -> TicketFilter {
        match (self.status, self.last_author) {
            (TicketStatus::Closed, _) => TicketFilter::Closed,
            (TicketStatus::Open, MessageAuthor::Client) => TicketFilter::Waiting,
            (TicketStatus::Open, _) => TicketFilter::Answered,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Image,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketFile {
    pub id: TicketFileId,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub kind: FileKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(default)]
    pub removed: bool,
}

impl TicketFile {
    pub fn mention(&self) -> String {
        format!("[вложение: {}]", self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketMessage {
    pub ticket_id: TicketId,
    pub author: MessageAuthor,
    pub admin_id: Option<AdminId>,
    pub body: String,
    pub attachments: Vec<TicketFile>,
    pub created_at: DateTime<Utc>,
}

impl TicketMessage {
    pub fn text_with_files(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        let body = self.body.trim();
        if !body.is_empty() {
            lines.push(body.to_string());
        }
        lines.extend(self.attachments.iter().map(TicketFile::mention));
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn фильтры_поддержки() {
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
        let mut ticket = Ticket {
            id: TicketId(1),
            client_id: ClientId(1),
            status: TicketStatus::Open,
            channel: TicketChannel::Bot,
            subject: None,
            last_author: MessageAuthor::Client,
            created_at: at,
            updated_at: at,
            closed_at: None,
        };
        assert_eq!(ticket.filter(), TicketFilter::Waiting);
        ticket.last_author = MessageAuthor::Support;
        assert_eq!(ticket.filter(), TicketFilter::Answered);
        ticket.status = TicketStatus::Closed;
        assert_eq!(ticket.filter(), TicketFilter::Closed);
        assert_eq!(TicketChannel::Bot.reply_targets(), [ReplyTarget::Telegram]);
    }

    #[test]
    fn вложения_видны_в_тексте_сообщения_строкой_с_именем() {
        let at = Utc.with_ymd_and_hms(2026, 11, 18, 9, 0, 0).unwrap();
        let file = |name: &str| TicketFile {
            id: TicketFileId(7),
            name: name.into(),
            mime: "image/png".into(),
            size: 10,
            kind: FileKind::Image,
            width: None,
            height: None,
            removed: false,
        };
        let mut message = TicketMessage {
            ticket_id: TicketId(1),
            author: MessageAuthor::Client,
            admin_id: None,
            body: "  не подключается  ".into(),
            attachments: vec![file("экран.png"), file("лог.txt")],
            created_at: at,
        };
        assert_eq!(
            message.text_with_files(),
            "не подключается\n[вложение: экран.png]\n[вложение: лог.txt]"
        );
        message.body = String::new();
        message.attachments.truncate(1);
        assert_eq!(message.text_with_files(), "[вложение: экран.png]");
        let json = serde_json::to_value(&message.attachments[0]).unwrap();
        assert_eq!(json["kind"], "image");
        assert!(json.get("width").is_none());
    }

    #[test]
    fn причины_передачи_оператору_читаются_обратно() {
        for reason in HandoffReason::ALL {
            assert_eq!(HandoffReason::parse(reason.as_str()).unwrap(), reason);
            assert!(!reason.title_ru().is_empty());
        }
        assert!(HandoffReason::parse("other").is_err());
        assert_eq!(
            HandoffReason::parse("groq").unwrap(),
            HandoffReason::NoAnswer
        );
        assert_eq!(
            serde_json::from_value::<HandoffReason>(serde_json::json!("groq")).unwrap(),
            HandoffReason::NoAnswer
        );
        assert_eq!(HandoffReason::NoAnswer.title_ru(), "ИИ не ответил");
        assert_eq!(AiState::parse("handed_off").unwrap(), AiState::HandedOff);
        let at = Utc.with_ymd_and_hms(2026, 11, 18, 9, 0, 0).unwrap();
        let ai = TicketAi {
            state: AiState::HandedOff,
            reason: Some(HandoffReason::Money),
            handoff_at: Some(at),
        };
        let client = serde_json::to_value(ai.for_client()).unwrap();
        assert_eq!(client["state"], "handed_off");
        assert!(client.get("reason").is_none());
        assert!(!ai.answering());
    }
}
