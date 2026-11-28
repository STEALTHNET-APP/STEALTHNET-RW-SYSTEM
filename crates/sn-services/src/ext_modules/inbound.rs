use super::{ExtModulesOps, INBOUND_PREFIX, address, runtime, store};
use crate::error::ServiceResult;
use bytes::Bytes;
use std::net::IpAddr;

pub const BODY_MAX: usize = 1024 * 1024;
pub const RESPONSE_MAX: usize = 1024 * 1024;
const RESPONSE_HEADERS: [&str; 3] = ["content-type", "retry-after", "cache-control"];

pub fn forwarded_header(name: &str) -> bool {
    matches!(
        name,
        "content-type" | "user-agent" | "accept" | "authorization"
    ) || (name.starts_with("x-")
        && !name.starts_with("x-forwarded-")
        && !name.starts_with("x-stealthnet-")
        && name != "x-real-ip")
}

pub struct InboundRequest<'a> {
    pub module: &'a str,
    pub method: &'a str,
    pub rest: &'a str,
    pub query: Option<&'a str>,
    pub headers: &'a [(String, String)],
    pub body: Bytes,
    pub client_ip: Option<IpAddr>,
    pub public_url: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundError {
    Unreachable,
    Timeout,
    TooBig,
}

impl InboundError {
    pub const fn text(self) -> &'static str {
        match self {
            InboundError::Unreachable => "Модуль не отвечает",
            InboundError::Timeout => "Модуль не ответил вовремя",
            InboundError::TooBig => "Ответ модуля больше 1 МБ",
        }
    }
}

impl ExtModulesOps<'_> {
    pub async fn inbound_base(&self, module: &str) -> ServiceResult<Option<String>> {
        if !runtime::valid_module(module) {
            return Ok(None);
        }
        let mut conn = self.services.store().conn().await?;
        let Some(row) = store::get(&mut conn, module).await? else {
            return Ok(None);
        };
        if !row.enabled || !row.managed {
            return Ok(None);
        }
        let Some(run) = row.manifest.run.as_ref() else {
            return Ok(None);
        };
        let Some(path) = run.inbound_path.as_deref() else {
            return Ok(None);
        };
        Ok(Some(runtime::url(
            self.services.update_config().as_deref(),
            module,
            run.port,
            path.trim_end_matches('/'),
        )))
    }
}

pub async fn forward(
    base: &str,
    request: InboundRequest<'_>,
) -> Result<InboundReply, InboundError> {
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|_| InboundError::Unreachable)?;
    let url = match request.query.filter(|q| !q.is_empty()) {
        Some(query) => format!("{base}{}?{query}", request.rest),
        None => format!("{base}{}", request.rest),
    };
    let mut outgoing = address::internal_client().request(method, &url);
    for (name, value) in request.headers {
        if forwarded_header(name) {
            outgoing = outgoing.header(name.as_str(), value.as_str());
        }
    }
    if let Some(ip) = request.client_ip {
        outgoing = outgoing.header("x-forwarded-for", ip.to_string());
    }
    outgoing = outgoing.header(
        "x-forwarded-prefix",
        format!("{INBOUND_PREFIX}{}", request.module),
    );
    if let Some(public) = request.public_url.and_then(|u| reqwest::Url::parse(u).ok()) {
        outgoing = outgoing.header("x-forwarded-proto", public.scheme());
        if let Some(host) = public.host_str() {
            let host = match public.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            };
            outgoing = outgoing.header("x-forwarded-host", host);
        }
    }
    if !request.body.is_empty() {
        outgoing = outgoing.body(request.body);
    }
    let mut response = outgoing.send().await.map_err(|err| {
        if err.is_timeout() {
            InboundError::Timeout
        } else {
            InboundError::Unreachable
        }
    })?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();
            RESPONSE_HEADERS
                .contains(&name.as_str())
                .then(|| value.to_str().ok().map(|v| (name, v.to_string())))
                .flatten()
        })
        .collect();
    let mut body: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > RESPONSE_MAX {
                    return Err(InboundError::TooBig);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(err) if err.is_timeout() => return Err(InboundError::Timeout),
            Err(_) => return Err(InboundError::Unreachable),
        }
    }
    Ok(InboundReply {
        status,
        headers,
        body: Bytes::from(body),
    })
}
