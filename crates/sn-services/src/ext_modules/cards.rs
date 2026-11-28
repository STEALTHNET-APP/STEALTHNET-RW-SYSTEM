use super::{ExtModulesOps, admin_claims, proxy, secret_key, signing, store};
use crate::error::ServiceResult;
use crate::settings::read_secret;
use chrono::Duration;
use serde::Serialize;
use sn_domain::access::AdminPrincipal;
use sn_domain::ids::ClientId;
use sn_store::Db;

pub const CLIENT_PARAM: &str = "client_id";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardFrame {
    pub module: String,
    pub name: String,
    pub title: String,
    pub url: String,
}

impl ExtModulesOps<'_> {
    pub async fn client_cards(
        &self,
        actor: &AdminPrincipal,
        client: ClientId,
    ) -> ServiceResult<Vec<CardFrame>> {
        if actor.api_key.is_some() {
            return Ok(Vec::new());
        }
        let mut conn = self.services.store().conn().await?;
        conn.clients().get(client).await?;
        let rows = store::list(&mut conn).await?;
        let wanted: Vec<_> = rows
            .into_iter()
            .filter(|row| row.enabled && row.managed && row.manifest.run.is_some())
            .filter(|row| {
                row.manifest
                    .client_card
                    .as_ref()
                    .is_some_and(|card| actor.can(card.permission))
            })
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let email = conn.admins().get(actor.admin_id).await?.email;
        let now = self.services.now();
        let mut out = Vec::new();
        for row in wanted {
            let Some(card) = row.manifest.client_card.as_ref() else {
                continue;
            };
            let Some(key) = read_secret(&mut conn, self.services.secrets(), &secret_key(row.id()))
                .await?
                .as_deref()
                .and_then(signing::key)
            else {
                continue;
            };
            let (claims, _) = admin_claims(actor, email.clone(), row.id(), now);
            let Some(token) = signing::page_token(&key, &claims) else {
                continue;
            };
            let cap = proxy::card_cap(
                &key,
                row.id(),
                actor.admin_id.0,
                actor.session_id,
                (now + Duration::hours(proxy::CAP_HOURS)).timestamp(),
            );
            let Ok(mut url) = reqwest::Url::parse("http://panel.invalid").and_then(|base| {
                base.join(&format!("{}{}/{cap}{}", proxy::PREFIX, row.id(), card.path))
            }) else {
                continue;
            };
            url.query_pairs_mut()
                .append_pair(CLIENT_PARAM, &client.0.to_string())
                .append_pair(signing::TOKEN_PARAM, &token);
            let relative = match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_string(),
            };
            out.push(CardFrame {
                module: row.manifest.id.clone(),
                name: row.manifest.name.clone(),
                title: card.title.clone(),
                url: relative,
            });
        }
        out.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.module.cmp(&b.module)));
        Ok(out)
    }
}
