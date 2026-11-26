use crate::error::{PaymentError, PaymentResult};
use crate::json::{self, Json};
use crate::md5;
use crate::provider::{
    CheckStatus, ConnectionCheck, PaymentRef, ProviderConfig, ProviderStatus, WebhookOutcome,
    WebhookReply,
};
use crate::util;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sn_domain::money::{Amount, PayCurrency, parse_units};
use sn_domain::payment::NoticeKind;

pub fn signature(body: &[u8], api_key: &str) -> String {
    md5::hex(format!("{}{api_key}", STANDARD.encode(body)).as_bytes())
}

fn signed_variants(raw: &[u8]) -> Option<(String, Vec<Vec<u8>>)> {
    let members = json::members(raw)?;
    let sign = members.iter().find(|m| m.key == "sign")?;
    let received = Json::parse(sign.raw_value)?.as_str()?.to_ascii_lowercase();
    let mut original = vec![b'{'];
    for (i, member) in members.iter().filter(|m| m.key != "sign").enumerate() {
        if i > 0 {
            original.push(b',');
        }
        original.extend_from_slice(member.raw_key);
        original.push(b':');
        original.extend_from_slice(member.raw_value);
    }
    original.push(b'}');
    let mut variants = vec![original];
    if let Some(Json::Object(mut all)) = Json::parse(raw) {
        all.retain(|(key, _)| key != "sign");
        let php = Json::Object(all).to_php().into_bytes();
        if !variants.contains(&php) {
            variants.push(php);
        }
    }
    Some((received, variants))
}

pub(crate) fn verify(api_key: &str, raw: &[u8]) -> bool {
    let Some((received, variants)) = signed_variants(raw) else {
        return false;
    };
    received.len() == 32
        && variants
            .iter()
            .any(|variant| util::secret_eq(&signature(variant, api_key), &received))
}

pub(crate) fn error_text(parsed: &Json) -> Option<String> {
    if let Some(message) = parsed
        .get("message")
        .and_then(Json::as_str)
        .filter(|m| !m.trim().is_empty())
    {
        return Some(message.to_string());
    }
    let mut texts = Vec::new();
    if let Some(Json::Object(fields)) = parsed.get("errors") {
        for (_, value) in fields {
            match value {
                Json::String(text) => texts.push(text.clone()),
                Json::Array(items) => {
                    texts.extend(items.iter().filter_map(Json::as_str).map(str::to_string))
                }
                _ => {}
            }
        }
    }
    let texts: Vec<&str> = texts
        .iter()
        .map(|t| t.trim().trim_end_matches('.'))
        .filter(|t| !t.is_empty())
        .collect();
    (!texts.is_empty()).then(|| texts.join("; "))
}

pub(crate) async fn call(
    http: &sn_egress::Http,
    url: String,
    config: &ProviderConfig,
    body: &Json,
) -> PaymentResult<Json> {
    let merchant = config.require("merchant_id")?;
    let key = config.require("api_key")?;
    let bytes = body.to_bytes();
    let reply = util::send(
        http.post(url)
            .header("merchant", merchant)
            .header("sign", signature(&bytes, key))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes),
    )
    .await?;
    let parsed = reply.json()?;
    let accepted = parsed.get("state").and_then(Json::scalar_text) == Some("0");
    match parsed.get("result") {
        Some(result) if reply.is_success() && accepted => Ok(result.clone()),
        _ => Err(reply.failure(error_text(&parsed).as_deref())),
    }
}

pub(crate) fn invoice_currency(currency: PayCurrency) -> PaymentResult<&'static str> {
    match currency {
        PayCurrency::Usd => Ok("USD"),
        PayCurrency::Rub => Ok("RUB"),
        other => Err(PaymentError::UnsupportedCurrency(other.code().into())),
    }
}

pub(crate) fn received_usd(info: &Json) -> Option<Amount> {
    let raw = info
        .get("payment_amount_usd")
        .and_then(Json::scalar_text)?
        .trim();
    let cents = match raw.split_once('.') {
        Some((whole, fraction)) => {
            format!("{whole}.{}", fraction.chars().take(2).collect::<String>())
        }
        None => raw.to_string(),
    };
    parse_units(&cents, PayCurrency::Usd).map(|minor| Amount::new(minor, PayCurrency::Usd))
}

fn invoice_amount(info: &Json) -> PaymentResult<Amount> {
    util::json_amount(info.get("amount"), info.get("currency"))
}

fn settled(info: &Json, status: &str) -> PaymentResult<(ProviderStatus, Option<Amount>)> {
    Ok(match status {
        "paid" => (ProviderStatus::Paid, Some(invoice_amount(info)?)),
        "paid_over" => {
            let invoice = invoice_amount(info)?;
            match received_usd(info) {
                Some(received)
                    if invoice.currency == PayCurrency::Usd && received.minor > invoice.minor =>
                {
                    (ProviderStatus::Overpaid { received }, Some(invoice))
                }
                _ => (ProviderStatus::Paid, Some(invoice)),
            }
        }
        "wrong_amount" => match received_usd(info).filter(|r| r.minor > 0) {
            Some(received) => (ProviderStatus::Underpaid { received }, Some(received)),
            None => (ProviderStatus::Failed, None),
        },
        "fail" | "system_fail" => (ProviderStatus::Failed, None),
        "cancel" => (ProviderStatus::Canceled, None),
        "locked" => (ProviderStatus::Held, None),
        _ => (ProviderStatus::Pending, None),
    })
}

pub(crate) fn outcome(info: &Json, refund_note: &str) -> PaymentResult<WebhookOutcome> {
    let uuid = info
        .get("uuid")
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| util::bad("нет uuid счёта"))?;
    let status = info
        .get("status")
        .or_else(|| info.get("payment_status"))
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет статуса счёта"))?;
    let (mapped, amount) = settled(info, status)?;
    let outcome = WebhookOutcome {
        event_key: format!("{uuid}:{status}"),
        payment: util::public_ref(info.get("order_id").and_then(Json::as_str))
            .or_else(|| Some(PaymentRef::ProviderTxid(uuid.to_string()))),
        provider_txid: Some(uuid.to_string()),
        status: mapped,
        amount,
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    };
    Ok(match status {
        "refund_paid" => outcome.with_notice(NoticeKind::RefundReported, refund_note),
        _ => outcome,
    })
}

pub(crate) fn requested(info: Json, uuid: &str, brand: &str) -> PaymentResult<Json> {
    if info.get("uuid").and_then(Json::as_str).map(str::trim) == Some(uuid) {
        Ok(info)
    } else {
        Err(util::bad(format!("{brand} вернул сведения о другом счёте")))
    }
}

pub(crate) fn skipped_type(body: &Json, kind: &str) -> WebhookOutcome {
    util::skipped(
        format!(
            "{kind}:{}",
            body.get("uuid").and_then(Json::as_str).unwrap_or_default()
        ),
        None,
        None,
        WebhookReply::ok(),
    )
}

pub(crate) fn serves(service: &Json, currencies: &[&str], networks: &[&str]) -> bool {
    let text = |key: &str| service.get(key).and_then(Json::as_str);
    text("currency").is_some_and(|c| currencies.iter().any(|x| c.eq_ignore_ascii_case(x)))
        && text("network").is_some_and(|n| networks.iter().any(|x| n.eq_ignore_ascii_case(x)))
}

pub(crate) fn service_closed(service: &Json) -> bool {
    service.get("is_available").and_then(Json::as_bool) == Some(false)
}

pub(crate) async fn services(
    http: &sn_egress::Http,
    url: String,
    config: &ProviderConfig,
    brand: &str,
) -> Result<Vec<Json>, ConnectionCheck> {
    let (merchant, key) = match (config.require("merchant_id"), config.require("api_key")) {
        (Ok(merchant), Ok(key)) => (merchant, key),
        (Err(err), _) | (_, Err(err)) => return Err(ConnectionCheck::failed(&err)),
    };
    let reply = match util::send(
        http.post(url)
            .header("merchant", merchant)
            .header("sign", signature(b"", key)),
    )
    .await
    {
        Ok(reply) => reply,
        Err(err) => return Err(ConnectionCheck::failed(&err)),
    };
    let parsed = Json::parse(&reply.body);
    let accepted = reply.is_success()
        && parsed
            .as_ref()
            .and_then(|p| p.get("state"))
            .and_then(Json::scalar_text)
            == Some("0");
    if accepted {
        return Ok(parsed
            .as_ref()
            .and_then(|p| p.get("result"))
            .and_then(Json::as_array)
            .unwrap_or_default()
            .to_vec());
    }
    let message = parsed.as_ref().and_then(error_text);
    if message
        .as_deref()
        .is_some_and(|m| m.to_ascii_lowercase().contains("merchant unknown"))
    {
        return Err(ConnectionCheck::new(
            CheckStatus::KeysRejected,
            format!("{brand} не знает мерчанта с таким ID"),
        )
        .detail(format!(
            "ID мерчанта — это UUID из личного кабинета {brand}, раздел «Настройки»"
        )));
    }
    if matches!(reply.status, 401 | 403) || reply.is_success() {
        let check = ConnectionCheck::new(
            CheckStatus::KeysRejected,
            format!("{brand} не принял ID мерчанта и платёжный ключ"),
        );
        return Err(match message {
            Some(message) => check.detail(format!(
                "Ответ {brand}: «{}». Нужен платёжный ключ из раздела API, ключ выплат не подходит",
                util::clip(&message, 200)
            )),
            None => check.detail("Нужен платёжный ключ из раздела API, ключ выплат не подходит"),
        });
    }
    Err(ConnectionCheck::failed(&reply.failure(message.as_deref())))
}
