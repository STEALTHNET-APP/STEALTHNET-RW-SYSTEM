mod files;

pub use files::{FileDownload, FileLink};

use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, ClientId, SubscriptionId, TicketFileId, TicketId};
use sn_domain::jobs::JobPayload;
use sn_domain::module::ModuleId;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::support::{
    HandoffReason, MAX_OPEN_TICKETS, MessageAuthor, ReplyTarget, Ticket, TicketAi, TicketChannel,
    TicketFile, TicketFilter, TicketMessage, TicketStatus,
};
use sn_modules::catalog::ai;
use sn_store::{ClientTicket, Db, NewReply, NewTicket};

pub const MAX_MESSAGE_CHARS: usize = 4_000;
pub const MAX_SUBJECT_CHARS: usize = 200;
pub const REPLY_TEMPLATE: &str = "support_reply";
pub const CLIENT_TICKETS_LIMIT: i64 = 100;
pub const MAX_CLOSE_MANY: usize = 500;
pub const MAX_WAITING_DAYS: u32 = 3_650;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseMany {
    Tickets(Vec<TicketId>),
    WaitingDays(u32),
}

fn waiting_days(days: u32) -> ServiceResult<u32> {
    if (1..=MAX_WAITING_DAYS).contains(&days) {
        Ok(days)
    } else {
        Err(ServiceError::rejected(
            "support.waiting_days",
            format!("Укажите от 1 до {MAX_WAITING_DAYS} дней"),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTicket {
    pub subscription_id: Option<SubscriptionId>,
    pub channel: TicketChannel,
    pub subject: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivered {
    Opened(TicketId),
    Added(TicketId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyRequest {
    pub body: String,
    #[serde(default)]
    pub close: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdminName {
    pub id: sn_domain::ids::AdminId,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TicketThread {
    pub ticket: Ticket,
    pub messages: Vec<TicketMessage>,
    pub admins: Vec<AdminName>,
    pub ai: Option<TicketAi>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientMessage {
    pub author: MessageAuthor,
    pub body: String,
    pub attachments: Vec<TicketFile>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientThread {
    pub ticket: Ticket,
    pub messages: Vec<ClientMessage>,
    pub ai: Option<TicketAi>,
    pub unread: i64,
}

fn message_body(body: &str, with_files: bool) -> ServiceResult<String> {
    let body = body.trim();
    if (body.is_empty() && !with_files) || body.chars().count() > MAX_MESSAGE_CHARS {
        return Err(ServiceError::rejected(
            "support.body",
            format!("Сообщение пустое или длиннее {MAX_MESSAGE_CHARS} символов"),
        ));
    }
    Ok(body.to_string())
}

fn files_json(files: &[TicketFile]) -> serde_json::Value {
    serde_json::Value::Array(
        files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "id": f.id,
                    "name": f.name,
                    "mime": f.mime,
                    "kind": f.kind,
                })
            })
            .collect(),
    )
}

pub struct SupportOps<'a> {
    services: &'a Services,
}

impl<'a> SupportOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        SupportOps { services }
    }

    pub async fn open(&self, client_id: ClientId, request: &OpenTicket) -> ServiceResult<TicketId> {
        self.open_with_files(client_id, request, &[]).await
    }

    pub async fn open_with_files(
        &self,
        client_id: ClientId,
        request: &OpenTicket,
        files: &[TicketFileId],
    ) -> ServiceResult<TicketId> {
        let body = message_body(&request.body, !files.is_empty())?;
        let subject = request
            .subject
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(MAX_SUBJECT_CHARS).collect::<String>());
        let mut tx = self.services.store().begin().await?;
        if let Some(subscription_id) = request.subscription_id
            && tx.subscriptions().get(subscription_id).await?.client_id != client_id
        {
            return Err(ServiceError::NotFound("подписка"));
        }
        let files = self
            .checked_files(&mut tx, Actor::Client(client_id), files)
            .await?;
        if tx.tickets().count_open(client_id).await? >= MAX_OPEN_TICKETS as i64 {
            return Err(ServiceError::rejected(
                "support.too_many",
                "Открыто слишком много обращений: дождитесь ответа или закройте решённые",
            ));
        }
        let answered_by_ai = ai::takes_ticket(self.services.modules(), request.channel);
        let id = tx
            .tickets()
            .open(&NewTicket {
                client_id,
                subscription_id: request.subscription_id,
                channel: request.channel,
                subject,
                body,
                ai: answered_by_ai,
            })
            .await?;
        if !files.is_empty() {
            let message = tx.tickets().last_message_id(id).await?;
            let ids: Vec<TicketFileId> = files.iter().map(|f| f.id).collect();
            tx.ticket_files().attach(id, message, &ids).await?;
        }
        if answered_by_ai {
            tx.jobs().enqueue(&ai::wake_job(id), None).await?;
        }
        tx.events()
            .push(&DomainEvent::TicketOpened {
                ticket_id: id,
                client_id,
            })
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn telegram_ticket(
        &self,
        client_id: ClientId,
        preferred: Option<TicketId>,
    ) -> ServiceResult<Option<TicketId>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .tickets()
            .open_in_telegram(client_id, preferred)
            .await?)
    }

    pub async fn from_telegram(
        &self,
        client_id: ClientId,
        body: &str,
        preferred: Option<TicketId>,
    ) -> ServiceResult<Delivered> {
        self.from_telegram_with_files(client_id, body, &[], preferred)
            .await
    }

    pub async fn from_telegram_with_files(
        &self,
        client_id: ClientId,
        body: &str,
        files: &[TicketFileId],
        preferred: Option<TicketId>,
    ) -> ServiceResult<Delivered> {
        let current = {
            let mut conn = self.services.store().conn().await?;
            conn.tickets()
                .open_in_telegram(client_id, preferred)
                .await?
        };
        if let Some(id) = current {
            let request = ReplyRequest {
                body: body.to_string(),
                close: false,
            };
            match self
                .reply_with_files(id, &request, files, Actor::Client(client_id))
                .await
            {
                Ok(()) => return Ok(Delivered::Added(id)),
                Err(ServiceError::Rejected {
                    code: "support.closed",
                    ..
                }) => {}
                Err(err) => return Err(err),
            }
        }
        let id = self
            .open_with_files(
                client_id,
                &OpenTicket {
                    subscription_id: None,
                    channel: TicketChannel::Bot,
                    subject: None,
                    body: body.to_string(),
                },
                files,
            )
            .await?;
        Ok(Delivered::Opened(id))
    }

    pub async fn list(
        &self,
        filter: TicketFilter,
        page: PageRequest,
    ) -> ServiceResult<Page<Ticket>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.tickets().list(filter, page).await?)
    }

    pub async fn list_view(
        &self,
        filter: TicketFilter,
        page: PageRequest,
        after_id: Option<TicketId>,
        query: Option<&str>,
    ) -> ServiceResult<(Page<sn_store::TicketListItem>, sn_store::TicketCounts)> {
        let mut conn = self.services.store().conn().await?;
        let items = conn
            .tickets()
            .list_view(filter, page, after_id, query)
            .await?;
        let counts = conn.tickets().counts().await?;
        Ok((items, counts))
    }

    pub async fn count_waiting_longer(&self, days: u32) -> ServiceResult<i64> {
        let days = waiting_days(days)?;
        let mut conn = self.services.store().conn().await?;
        Ok(conn.tickets().count_waiting_longer(days).await?)
    }

    pub async fn close_many(&self, request: &CloseMany, admin: AdminId) -> ServiceResult<u64> {
        let mut tx = self.services.store().begin().await?;
        let (closed, days) = match request {
            CloseMany::Tickets(ids) => {
                if ids.is_empty() || ids.len() > MAX_CLOSE_MANY {
                    return Err(ServiceError::rejected(
                        "support.close_many",
                        format!("Отметьте от 1 до {MAX_CLOSE_MANY} обращений"),
                    ));
                }
                (tx.tickets().close_many(ids).await?, None)
            }
            CloseMany::WaitingDays(days) => {
                let days = waiting_days(*days)?;
                (tx.tickets().close_waiting_longer(days).await?, Some(days))
            }
        };
        if !closed.is_empty() {
            tx.audit()
                .record(&Actor::Admin(admin).audit(
                    "ticket.bulk_closed",
                    "ticket",
                    None,
                    serde_json::json!({
                        "count": closed.len(),
                        "waiting_days": days,
                        "tickets": closed.iter().take(MAX_CLOSE_MANY).map(|id| id.0).collect::<Vec<_>>(),
                    }),
                ))
                .await?;
        }
        tx.commit().await?;
        Ok(closed.len() as u64)
    }

    pub async fn for_client(&self, client_id: ClientId) -> ServiceResult<Vec<ClientTicket>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .tickets()
            .client_view(client_id, CLIENT_TICKETS_LIMIT)
            .await?)
    }

    pub async fn start_unread(&self) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.tickets().start_unread().await?)
    }

    pub async fn unread(&self, client_id: ClientId) -> ServiceResult<i64> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.tickets().client_unread(client_id).await?)
    }

    pub async fn waiting_count(&self) -> ServiceResult<i64> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM tickets WHERE status = 'open' AND last_author = 'client'",
        )
        .fetch_one(conn.conn())
        .await?)
    }

    async fn ticket_for(
        &self,
        db: &mut impl Db,
        id: TicketId,
        actor: Actor,
    ) -> ServiceResult<Ticket> {
        let ticket = db.tickets().get(id).await?;
        if let Actor::Client(client_id) = actor
            && ticket.client_id != client_id
        {
            return Err(ServiceError::NotFound("обращение"));
        }
        Ok(ticket)
    }

    pub async fn thread(&self, id: TicketId, actor: Actor) -> ServiceResult<TicketThread> {
        let mut conn = self.services.store().conn().await?;
        let ticket = self.ticket_for(&mut conn, id, actor).await?;
        if matches!(actor, Actor::Admin(_)) {
            conn.tickets().mark_read(id).await?;
        }
        let messages = conn.tickets().messages(id).await?;
        let ai = conn.tickets().ai(id).await?.map(|ai| match actor {
            Actor::Admin(_) | Actor::System => ai,
            Actor::Client(_) => ai.for_client(),
        });
        let admins =
            if matches!(actor, Actor::Admin(_)) && messages.iter().any(|m| m.admin_id.is_some()) {
                conn.admins()
                    .list()
                    .await?
                    .into_iter()
                    .map(|admin| AdminName {
                        id: admin.id,
                        email: admin.email,
                    })
                    .collect()
            } else {
                Vec::new()
            };
        Ok(TicketThread {
            ticket,
            messages,
            admins,
            ai,
        })
    }

    pub async fn client_thread(
        &self,
        id: TicketId,
        client_id: ClientId,
        seen: bool,
    ) -> ServiceResult<ClientThread> {
        let thread = self.thread(id, Actor::Client(client_id)).await?;
        let messages: Vec<ClientMessage> = thread
            .messages
            .into_iter()
            .filter(|m| m.author != MessageAuthor::System)
            .map(|m| ClientMessage {
                author: m.author,
                body: m.body,
                attachments: m.attachments,
                created_at: m.created_at,
            })
            .collect();
        let mut conn = self.services.store().conn().await?;
        if seen && let Some(last) = messages.last() {
            conn.tickets()
                .mark_seen(id, client_id, last.created_at)
                .await?;
        }
        let unread = conn.tickets().client_unread(client_id).await?;
        Ok(ClientThread {
            ticket: thread.ticket,
            messages,
            ai: thread.ai,
            unread,
        })
    }

    pub async fn ask_operator(&self, id: TicketId, actor: Actor) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        let ticket = self.ticket_for(&mut tx, id, actor).await?;
        if ticket.status == TicketStatus::Closed {
            return Err(ServiceError::rejected(
                "support.closed",
                "Обращение закрыто, создайте новое",
            ));
        }
        if tx.tickets().ai(id).await?.is_some_and(|ai| ai.answering()) {
            ai::store::hand_off(
                &mut tx,
                id,
                HandoffReason::Human,
                &ai::store::Spent::NONE,
                None,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn reopen(&self, id: TicketId, actor: Actor) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        self.ticket_for(&mut conn, id, actor).await?;
        conn.tickets().reopen(id).await?;
        if matches!(actor, Actor::Admin(_)) {
            conn.tickets().take_over(id).await?;
        }
        Ok(())
    }

    pub async fn reply(
        &self,
        id: TicketId,
        request: &ReplyRequest,
        actor: Actor,
    ) -> ServiceResult<()> {
        self.reply_with_files(id, request, &[], actor).await
    }

    pub async fn reply_with_files(
        &self,
        id: TicketId,
        request: &ReplyRequest,
        files: &[TicketFileId],
        actor: Actor,
    ) -> ServiceResult<()> {
        let body = message_body(&request.body, !files.is_empty())?;
        let mut tx = self.services.store().begin().await?;
        let ticket = self.ticket_for(&mut tx, id, actor).await?;
        let answering = tx.tickets().ai(id).await?.is_some_and(|ai| ai.answering());
        let (author, admin_id, close) = match actor {
            Actor::Client(_) => {
                if ticket.status == TicketStatus::Closed {
                    return Err(ServiceError::rejected(
                        "support.closed",
                        "Обращение закрыто, создайте новое",
                    ));
                }
                (MessageAuthor::Client, None, false)
            }
            Actor::Admin(admin) => (MessageAuthor::Support, Some(admin), request.close),
            Actor::System => (MessageAuthor::System, None, request.close),
        };
        let files = self.checked_files(&mut tx, actor, files).await?;
        tx.tickets()
            .reply(&NewReply {
                ticket_id: id,
                author,
                admin_id,
                body: body.clone(),
                attachments: files.iter().map(|f| f.id).collect(),
                close,
            })
            .await?;
        if answering {
            match actor {
                Actor::Client(_) if self.services.modules().is_enabled(ModuleId::Ai) => {
                    tx.jobs().enqueue(&ai::wake_job(id), None).await?;
                }
                Actor::Admin(_) => {
                    tx.tickets().take_over(id).await?;
                }
                _ => {}
            }
        }
        if author != MessageAuthor::Client {
            for target in ticket.channel.reply_targets() {
                match target {
                    ReplyTarget::Telegram | ReplyTarget::Email => {
                        tx.jobs()
                            .enqueue(
                                &JobPayload::Notify {
                                    client_id: ticket.client_id,
                                    template: REPLY_TEMPLATE.to_string(),
                                    params: serde_json::json!({
                                        "ticket_id": id,
                                        "body": body,
                                        "closed": close,
                                        "files": files_json(&files),
                                    }),
                                },
                                None,
                            )
                            .await?;
                    }
                    ReplyTarget::Cabinet => {}
                }
            }
            tx.events()
                .push(&DomainEvent::TicketAnswered {
                    ticket_id: id,
                    client_id: ticket.client_id,
                })
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete(&self, id: TicketId, admin: AdminId) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        let ticket = tx.tickets().get(id).await?;
        let files = tx.ticket_files().for_ticket(id).await?;
        tx.tickets().delete(id).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "ticket.deleted",
                "ticket",
                Some(id.0),
                serde_json::json!({
                    "client_id": ticket.client_id,
                    "subject": ticket.subject,
                    "files": files.values().map(Vec::len).sum::<usize>(),
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn close(&self, id: TicketId, actor: Actor) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        if let Actor::Client(client_id) = actor {
            let owner: Option<i64> =
                sqlx::query_scalar("SELECT client_id FROM tickets WHERE id = $1")
                    .bind(id.0)
                    .fetch_optional(conn.conn())
                    .await?;
            if owner != Some(client_id.0) {
                return Err(ServiceError::NotFound("обращение"));
            }
        }
        conn.tickets().close(id).await?;
        Ok(())
    }
}
