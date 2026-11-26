use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::ids::ClientId;
use sn_domain::ledger::Wallet;
use sn_domain::module::ModuleId;
use sn_domain::money::Money;
use sn_domain::paging::{Page, PageRequest};
use sn_modules::catalog::start_param;
use sn_store::{
    ClientFilter, ClientPerson, Db, InvitedClient, LockSpace, ReferralLink, StoreError,
    audit_entity, referral_audit,
};
use std::collections::HashMap;

#[cfg(test)]
mod tests;

pub const SELF: &str = "referral.self";
pub const CYCLE: &str = "referral.cycle";
pub const UNKNOWN: &str = "referral.unknown";
pub const NOT_INVITED: &str = "referral.not_invited";
pub const FIRST_PAGE: u32 = 5;
pub const CANDIDATES: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManualLink {
    pub at: DateTime<Utc>,
    pub by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Inviter {
    pub client: ClientPerson,
    pub blocked: bool,
    pub deleted: bool,
    pub linked_at: DateTime<Utc>,
    pub source: Option<String>,
    pub code: Option<String>,
    pub manual: Option<ManualLink>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Invited {
    pub client: ClientPerson,
    pub joined_at: DateTime<Utc>,
    pub manual_at: Option<DateTime<Utc>>,
    pub blocked: bool,
    pub deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bonus: Option<Money>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReferralMoney {
    pub invited_paid: i64,
    pub earned: Money,
    pub withdrawn: Money,
    pub pending: Money,
    pub balance: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferralSection {
    pub module_enabled: bool,
    pub inviter: Option<Inviter>,
    pub invited_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub money: Option<ReferralMoney>,
    pub invited: Page<Invited>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Itself,
    Cycle,
    Current,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub client: ClientPerson,
    pub joined_at: DateTime<Utc>,
    pub blocked: bool,
    pub refusal: Option<Refusal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReferrerChange {
    pub changed: bool,
}

pub fn person_label(person: Option<&ClientPerson>, id: ClientId) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(person) = person {
        if let Some(name) = person
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            parts.push(name.to_string());
        }
        if let Some(user) = person
            .telegram_username
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            parts.push(format!("@{user}"));
        } else if let Some(email) = person.email.as_deref().filter(|e| !e.trim().is_empty()) {
            parts.push(email.trim().to_string());
        }
    }
    if parts.is_empty()
        && let Some(telegram) = person
            .and_then(|p| p.telegram_id.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty())
    {
        parts.push(format!("id {telegram}"));
    }
    parts.push(format!("№{}", id.0));
    parts.join(" · ")
}

fn admin_of(actor: Actor) -> Option<sn_domain::ids::AdminId> {
    match actor {
        Actor::Admin(id) => Some(id),
        Actor::Client(_) | Actor::System => None,
    }
}

fn someone(id: ClientId) -> ClientPerson {
    ClientPerson {
        client_id: id,
        name: None,
        telegram_username: None,
        telegram_id: None,
        email: None,
    }
}

pub struct ReferralLinkOps<'a> {
    services: &'a Services,
}

impl<'a> ReferralLinkOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        ReferralLinkOps { services }
    }

    pub async fn section(
        &self,
        client: ClientId,
        money: bool,
        limit: Option<u32>,
    ) -> ServiceResult<ReferralSection> {
        let currency = self.services.settings().currency;
        let mut conn = self.services.store().conn().await?;
        conn.clients().get(client).await?;
        let inviter = match conn.referrals().link(client).await? {
            Some(link) => Some(inviter_of(&mut conn, client, link).await?),
            None => None,
        };
        let totals = conn.referrals().totals(client, currency).await?;
        let page = conn
            .referrals()
            .invited(
                client,
                currency,
                PageRequest::new(Some(limit.unwrap_or(FIRST_PAGE)), None),
            )
            .await?;
        let invited = invited_page(&mut conn, page, money).await?;
        let money = if money {
            Some(ReferralMoney {
                invited_paid: totals.invited_paid,
                earned: totals.earned,
                withdrawn: totals.withdrawn,
                pending: totals.pending,
                balance: conn
                    .ledger()
                    .balance(client, Wallet::Referral, currency)
                    .await?,
            })
        } else {
            None
        };
        Ok(ReferralSection {
            module_enabled: self.services.modules().is_enabled(ModuleId::Referral),
            inviter,
            invited_count: totals.invited,
            money,
            invited,
        })
    }

    pub async fn invited(
        &self,
        client: ClientId,
        page: PageRequest,
        money: bool,
    ) -> ServiceResult<Page<Invited>> {
        let currency = self.services.settings().currency;
        let mut conn = self.services.store().conn().await?;
        conn.clients().get(client).await?;
        let found = conn.referrals().invited(client, currency, page).await?;
        invited_page(&mut conn, found, money).await
    }

    pub async fn candidates(&self, client: ClientId, query: &str) -> ServiceResult<Vec<Candidate>> {
        let query: String = query.trim().chars().take(200).collect();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        {
            let mut conn = self.services.store().conn().await?;
            conn.clients().get(client).await?;
        }
        let filter = ClientFilter {
            query: Some(query),
            ..ClientFilter::default()
        };
        let found = self
            .services
            .clients()
            .search(&filter, PageRequest::new(Some(CANDIDATES), None))
            .await?;
        let ids: Vec<ClientId> = found.items.iter().map(|item| item.client.id).collect();
        let mut conn = self.services.store().conn().await?;
        let current = conn.referrals().link(client).await?.map(|l| l.referrer_id);
        let below = conn.referrals().reaching(client, &ids).await?;
        Ok(found
            .items
            .into_iter()
            .map(|item| {
                let id = item.client.id;
                let refusal = if id == client {
                    Some(Refusal::Itself)
                } else if below.contains(&id) {
                    Some(Refusal::Cycle)
                } else if current == Some(id) {
                    Some(Refusal::Current)
                } else {
                    None
                };
                Candidate {
                    client: ClientPerson {
                        client_id: id,
                        name: item.name,
                        telegram_username: item.telegram_username,
                        telegram_id: item.telegram_id,
                        email: item.email,
                    },
                    joined_at: item.client.created_at,
                    blocked: item.client.is_blocked(),
                    refusal,
                }
            })
            .collect())
    }

    pub async fn set_referrer(
        &self,
        client: ClientId,
        referrer: ClientId,
        actor: Actor,
    ) -> ServiceResult<ReferrerChange> {
        if client == referrer {
            return Err(ServiceError::rejected(
                SELF,
                "Нельзя назначить пригласившим самого клиента",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Referrals, 0).await?;
        tx.clients().get(client).await?;
        match tx.clients().get(referrer).await {
            Ok(_) => {}
            Err(StoreError::NotFound(_)) => {
                return Err(ServiceError::rejected(
                    UNKNOWN,
                    "Клиент не найден или удалён: выберите другого клиента",
                ));
            }
            Err(err) => return Err(err.into()),
        }
        if !tx
            .referrals()
            .reaching(client, &[referrer])
            .await?
            .is_empty()
        {
            return Err(ServiceError::rejected(
                CYCLE,
                "Так получится круг: выбранный клиент уже в цепочке приглашённых этого клиента",
            ));
        }
        let previous = tx.referrals().lock_referrer(client).await?;
        if previous == Some(referrer) {
            tx.commit().await?;
            return Ok(ReferrerChange { changed: false });
        }
        tx.referrals()
            .set(client, referrer, admin_of(actor))
            .await?;
        let mut ids = vec![client, referrer];
        ids.extend(previous);
        let people = tx.clients().people(&ids).await?;
        tx.audit()
            .record(&actor.audit(
                referral_audit::REFERRER_SET,
                audit_entity::CLIENT,
                Some(client.0),
                serde_json::json!({
                    "from": { "referrer": previous.map(|id| person_label(people.get(&id), id)) },
                    "to": { "referrer": person_label(people.get(&referrer), referrer) },
                    "invitee": person_label(people.get(&client), client),
                    "referrer_id": referrer.0,
                    "previous_referrer_id": previous.map(|id| id.0),
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(ReferrerChange { changed: true })
    }

    pub async fn remove_referrer(
        &self,
        client: ClientId,
        expected: Option<ClientId>,
        actor: Actor,
    ) -> ServiceResult<ReferrerChange> {
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Referrals, 0).await?;
        let Some(previous) = tx.referrals().lock_referrer(client).await? else {
            if expected.is_none() {
                tx.clients().get(client).await?;
            }
            tx.commit().await?;
            return Ok(ReferrerChange { changed: false });
        };
        if expected.is_some_and(|referrer| referrer != previous) {
            return Err(ServiceError::rejected(
                NOT_INVITED,
                "Этого клиента уже нет среди приглашённых: список обновлён",
            ));
        }
        tx.referrals().remove(client).await?;
        let people = tx.clients().people(&[client, previous]).await?;
        tx.audit()
            .record(&actor.audit(
                referral_audit::REFERRER_REMOVED,
                audit_entity::CLIENT,
                Some(client.0),
                serde_json::json!({
                    "from": { "referrer": person_label(people.get(&previous), previous) },
                    "to": { "referrer": serde_json::Value::Null },
                    "invitee": person_label(people.get(&client), client),
                    "previous_referrer_id": previous.0,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(ReferrerChange { changed: true })
    }
}

async fn inviter_of(
    conn: &mut impl Db,
    client: ClientId,
    link: ReferralLink,
) -> ServiceResult<Inviter> {
    let people = conn.clients().people(&[link.referrer_id]).await?;
    let person = people
        .get(&link.referrer_id)
        .cloned()
        .unwrap_or_else(|| someone(link.referrer_id));
    let manual = link.set_at.map(|at| ManualLink {
        at,
        by: link.set_by_email.clone(),
    });
    let (source, code) = if manual.is_some() {
        (None, None)
    } else {
        let origin = conn.referrals().origin(client).await?;
        let source = match origin.source {
            Some(source) => Some(source),
            None => Some(
                conn.clients()
                    .get(client)
                    .await?
                    .registration_source
                    .as_str()
                    .to_string(),
            ),
        };
        (
            source,
            origin
                .start_param
                .as_deref()
                .and_then(start_param::referral_code),
        )
    };
    Ok(Inviter {
        client: person,
        blocked: link.referrer_blocked,
        deleted: link.referrer_deleted,
        linked_at: link.linked_at,
        source,
        code,
        manual,
    })
}

async fn invited_page(
    conn: &mut impl Db,
    page: Page<InvitedClient>,
    money: bool,
) -> ServiceResult<Page<Invited>> {
    let ids: Vec<ClientId> = page.items.iter().map(|row| row.client_id).collect();
    let mut people: HashMap<ClientId, ClientPerson> = conn.clients().people(&ids).await?;
    let items = page
        .items
        .into_iter()
        .map(|row| Invited {
            client: people
                .remove(&row.client_id)
                .unwrap_or_else(|| someone(row.client_id)),
            joined_at: row.joined_at,
            manual_at: row.set_at,
            blocked: row.blocked,
            deleted: row.deleted,
            paid: money.then_some(row.paid),
            bonus: money.then_some(row.bonus),
        })
        .collect();
    Ok(Page {
        items,
        next: page.next,
    })
}
