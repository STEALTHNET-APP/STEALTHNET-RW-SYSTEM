use super::anthropic;
use super::provider::Provider;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(25);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DETAIL_MAX: usize = 300;
pub const MODELS_MAX: usize = 1_000;
const TEMPERATURE: f64 = 0.2;
const NOT_CHAT: [&str; 24] = [
    "embed",
    "whisper",
    "tts",
    "transcribe",
    "audio",
    "realtime",
    "dall-e",
    "image",
    "imagen",
    "veo",
    "sora",
    "moderation",
    "guard",
    "lyria",
    "aqa",
    "orpheus",
    "computer-use",
    "search",
    "-live",
    "live-",
    "rerank",
    "babbage",
    "davinci",
    "speech",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatMessage {
    pub role: &'static str,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "system",
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "user",
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "assistant",
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Param {
    Reasoning,
    Json,
    Temperature,
    Tokens,
}

impl Param {
    const fn title(self) -> &'static str {
        match self {
            Param::Reasoning => "настройку рассуждения",
            Param::Json => "режим JSON",
            Param::Temperature => "настройку temperature",
            Param::Tokens => "поле лимита токенов",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatOptions {
    pub max_tokens: u32,
    pub json: bool,
    pub reasoning: bool,
    pub temperature: bool,
    pub other_tokens: bool,
}

impl ChatOptions {
    pub fn full(max_tokens: u32, provider: Provider, model: &str) -> Self {
        ChatOptions {
            max_tokens,
            json: true,
            reasoning: sends_reasoning(provider, model),
            temperature: sends_temperature(provider),
            other_tokens: false,
        }
    }

    const fn has(&self, param: Param) -> bool {
        match param {
            Param::Reasoning => self.reasoning,
            Param::Json => self.json,
            Param::Temperature => self.temperature,
            Param::Tokens => !self.other_tokens,
        }
    }

    pub const fn without(mut self, param: Param) -> Self {
        match param {
            Param::Reasoning => self.reasoning = false,
            Param::Json => self.json = false,
            Param::Temperature => self.temperature = false,
            Param::Tokens => self.other_tokens = true,
        }
        self
    }

    fn step_down(self) -> Option<Self> {
        [Param::Reasoning, Param::Json, Param::Temperature]
            .into_iter()
            .find(|p| self.has(*p))
            .map(|p| self.without(p))
    }

    pub fn degrade(self, fault: &Fault) -> Option<Self> {
        match fault {
            Fault::JsonFailed if self.json => Some(self.without(Param::Json)),
            Fault::Unsupported(param, _) if self.has(*param) => Some(self.without(*param)),
            Fault::Unsupported(..) | Fault::Rejected(_) => self.step_down(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub content: String,
    pub finish_reason: Option<String>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelOption {
    pub id: String,
    pub free: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_out: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    Unauthorized,
    Forbidden(String),
    Region(String),
    Payment(String),
    Policy(String),
    Model(String),
    RateLimited { retry_after: Option<u64> },
    JsonFailed,
    Unsupported(Param, String),
    TooLong,
    Redirect,
    Rejected(String),
    Unavailable(u16),
    Network(String),
    Malformed(String),
    Address(String),
}

impl Fault {
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Fault::RateLimited { .. } | Fault::Unavailable(_) | Fault::Network(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiError {
    pub provider: Provider,
    pub fault: Fault,
}

impl AiError {
    pub const fn new(provider: Provider, fault: Fault) -> Self {
        AiError { provider, fault }
    }
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.provider.name();
        match &self.fault {
            Fault::Unauthorized => match self.provider.keys_page() {
                Some(page) => write!(f, "{name} не принял ключ: проверьте его на {page}"),
                None => write!(
                    f,
                    "{name} не принял ключ: проверьте ключ у владельца адреса API"
                ),
            },
            Fault::Forbidden(detail) => {
                write!(f, "{name} отказал в доступе (403): {detail}")?;
                if let Some(page) = self.provider.keys_page() {
                    write!(f, ". Проверьте ключ и доступ на {page}")?;
                }
                if self.provider.blocked_by_country() {
                    write!(
                        f,
                        ". Так бывает, если {name} недоступен из страны сервера панели: укажите в «Адрес API» свой прокси или выберите DeepSeek или OpenRouter"
                    )?;
                }
                Ok(())
            }
            Fault::Region(detail) => write!(
                f,
                "{name} недоступен из страны сервера панели: {detail}. Укажите в «Адрес API» свой прокси или выберите DeepSeek или OpenRouter"
            ),
            Fault::Payment(detail) => {
                write!(
                    f,
                    "{name} не выполнил запрос из-за оплаты: {detail}. Проверьте баланс в кабинете провайдера"
                )?;
                if self.provider == Provider::OpenRouter {
                    write!(f, " или выберите бесплатную модель с :free")?;
                }
                Ok(())
            }
            Fault::Policy(detail) => write!(
                f,
                "{name} не нашёл сервер для модели: {detail}. Бесплатные модели работают, только если в openrouter.ai/settings/privacy разрешены провайдеры, которые сохраняют переписку; иначе выберите другую модель"
            ),
            Fault::Model(detail) => write!(
                f,
                "{name} не нашёл модель: {detail}. Укажите другую модель в настройках модуля"
            ),
            Fault::RateLimited { .. } => write!(f, "{name} ограничил частоту запросов"),
            Fault::JsonFailed => write!(f, "{name} не получил от модели ответ в виде JSON"),
            Fault::Unsupported(param, detail) => write!(
                f,
                "{name}: модель не поддерживает {}: {detail}",
                param.title()
            ),
            Fault::TooLong => write!(f, "{name} отклонил запрос: слишком длинная переписка"),
            Fault::Redirect => write!(
                f,
                "{name} перенаправляет запрос на другой адрес, а ключ туда не отправляется: укажите в «Адрес API» конечный адрес"
            ),
            Fault::Rejected(detail) => write!(f, "{name} отклонил запрос: {detail}"),
            Fault::Unavailable(code) => write!(f, "{name} временно недоступен (код {code})"),
            Fault::Network(detail) => write!(f, "{name} не ответил: {detail}"),
            Fault::Malformed(detail) => write!(f, "{name} прислал непонятный ответ: {detail}"),
            Fault::Address(detail) => write!(f, "Адрес API не подходит: {detail}"),
        }
    }
}

impl std::error::Error for AiError {}

pub(super) fn cut(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= DETAIL_MAX {
        return flat;
    }
    let mut out: String = flat.chars().take(DETAIL_MAX).collect();
    out.push('…');
    out
}

pub(super) fn tokens(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[derive(Debug, Default)]
struct ErrorInfo {
    message: String,
    code: String,
    kind: String,
    param: String,
}

fn scalar(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn error_info(body: &str) -> ErrorInfo {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return ErrorInfo::default();
    };
    let root = match parsed {
        Value::Array(items) => items.into_iter().next().unwrap_or(Value::Null),
        other => other,
    };
    let error = match root.get("error") {
        Some(Value::String(message)) => {
            return ErrorInfo {
                message: message.clone(),
                ..ErrorInfo::default()
            };
        }
        Some(inner @ Value::Object(_)) => inner,
        _ => &root,
    };
    let mut info = ErrorInfo {
        message: scalar(error.get("message")),
        code: scalar(error.get("code")),
        kind: scalar(error.get("type")),
        param: scalar(error.get("param")),
    };
    if info.message.is_empty() {
        info.message = scalar(root.get("message").or_else(|| root.get("detail")));
    }
    if info.kind.is_empty() {
        info.kind = scalar(error.get("status"));
    }
    if let Some(details) = error.get("details").and_then(Value::as_array) {
        let reasons: Vec<String> = details
            .iter()
            .filter_map(|d| d.get("reason").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        if !reasons.is_empty() {
            info.param = format!("{} {}", info.param, reasons.join(" "));
        }
    }
    info
}

fn any(probe: &str, words: &[&str]) -> bool {
    words.iter().any(|w| probe.contains(w))
}

fn param_of(probe: &str) -> Option<Param> {
    if probe.contains("temperature") {
        Some(Param::Temperature)
    } else if any(probe, &["reasoning", "thinking", "effort"]) {
        Some(Param::Reasoning)
    } else if any(
        probe,
        &[
            "response_format",
            "json_schema",
            "json_object",
            "json mode",
            "structured output",
            "output_config.format",
            "output_format",
        ],
    ) {
        Some(Param::Json)
    } else if any(probe, &["max_completion_tokens", "max_tokens"])
        && any(
            probe,
            &[
                "unsupported",
                "not supported",
                "unknown",
                "unrecognized",
                "extra inputs",
                "not permitted",
            ],
        )
    {
        Some(Param::Tokens)
    } else {
        None
    }
}

pub(super) fn fault(status: u16, body: &str, retry_after: Option<u64>) -> Fault {
    let info = error_info(body);
    let message = if info.message.trim().is_empty() {
        cut(body)
    } else {
        cut(&info.message)
    };
    let probe = format!(
        "{} {} {} {}",
        info.message, info.code, info.kind, info.param
    )
    .to_lowercase();
    if any(
        &probe,
        &[
            "location is not supported",
            "unsupported_country",
            "country, region, or territory",
            "not available in your country",
            "not available in your region",
        ],
    ) {
        return Fault::Region(message);
    }
    if matches!(status, 400 | 401 | 403)
        && any(
            &probe,
            &[
                "api key not valid",
                "api_key_invalid",
                "invalid api key",
                "invalid_api_key",
                "incorrect api key",
                "authentication fails",
                "invalid x-api-key",
            ],
        )
    {
        return Fault::Unauthorized;
    }
    let money = any(
        &probe,
        &[
            "insufficient_quota",
            "credit_balance",
            "credit balance",
            "insufficient balance",
            "insufficient credits",
            "exceeded your current quota",
            "billing",
        ],
    );
    match status {
        300..=399 => Fault::Redirect,
        401 => Fault::Unauthorized,
        402 => Fault::Payment(message),
        400 | 429 if money => Fault::Payment(message),
        403 => Fault::Forbidden(message),
        404 if probe.contains("data policy") => Fault::Policy(message),
        404 => Fault::Model(message),
        408 => Fault::Unavailable(408),
        413 => Fault::TooLong,
        429 => Fault::RateLimited { retry_after },
        400 | 422 if probe.contains("json_validate_failed") => Fault::JsonFailed,
        400 | 422
            if any(
                &probe,
                &[
                    "model_not_found",
                    "decommissioned",
                    "does not exist",
                    "model not found",
                    "not a valid model",
                    "unknown model",
                    "is not found for api version",
                ],
            ) =>
        {
            Fault::Model(message)
        }
        400 | 422
            if any(
                &probe,
                &[
                    "context_length_exceeded",
                    "context length",
                    "prompt is too long",
                    "maximum context",
                    "too many tokens",
                ],
            ) =>
        {
            Fault::TooLong
        }
        400 | 422 => match param_of(&probe) {
            Some(param) => Fault::Unsupported(param, message),
            None => Fault::Rejected(message),
        },
        498 | 500..=599 => Fault::Unavailable(status),
        _ => Fault::Rejected(message),
    }
}

pub fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reply": { "type": "string" },
            "handoff": { "type": "boolean" },
            "reason": { "type": "string" }
        },
        "required": ["reply", "handoff", "reason"],
        "additionalProperties": false
    })
}

fn gemini_effort(model: &str) -> Option<&'static str> {
    if model.starts_with("gemini-2.5-flash") {
        Some("none")
    } else if model.contains("flash-lite") {
        None
    } else if model.starts_with("gemini-") {
        Some("low")
    } else {
        None
    }
}

fn reasoning_fields(provider: Provider, model: &str) -> Vec<(&'static str, Value)> {
    match provider {
        Provider::Groq if model.starts_with("openai/gpt-oss") => vec![
            ("reasoning_effort", json!("low")),
            ("include_reasoning", json!(false)),
        ],
        Provider::Groq if model.starts_with("qwen/") => {
            vec![("reasoning_format", json!("hidden"))]
        }
        Provider::Gemini => gemini_effort(model)
            .map(|effort| vec![("reasoning_effort", json!(effort))])
            .unwrap_or_default(),
        Provider::DeepSeek => vec![("thinking", json!({ "type": "disabled" }))],
        Provider::OpenRouter => vec![("reasoning", json!({ "effort": "low", "exclude": true }))],
        Provider::OpenAi => vec![("reasoning_effort", json!("low"))],
        _ => Vec::new(),
    }
}

fn sends_reasoning(provider: Provider, model: &str) -> bool {
    provider == Provider::Anthropic || !reasoning_fields(provider, model).is_empty()
}

const fn sends_temperature(provider: Provider) -> bool {
    matches!(
        provider,
        Provider::Groq | Provider::DeepSeek | Provider::OpenRouter | Provider::Custom
    )
}

const fn token_field(provider: Provider, other: bool) -> &'static str {
    let completion = matches!(provider, Provider::Groq | Provider::OpenAi);
    if completion != other {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

fn json_format(provider: Provider) -> Value {
    match provider {
        Provider::Gemini | Provider::OpenAi => json!({
            "type": "json_schema",
            "json_schema": { "name": "support_answer", "strict": true, "schema": answer_schema() }
        }),
        _ => json!({ "type": "json_object" }),
    }
}

pub fn request_body(
    provider: Provider,
    model: &str,
    messages: &[ChatMessage],
    options: ChatOptions,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!(model));
    body.insert("messages".into(), json!(messages));
    body.insert(
        token_field(provider, options.other_tokens).into(),
        json!(options.max_tokens),
    );
    if options.temperature && sends_temperature(provider) {
        body.insert("temperature".into(), json!(TEMPERATURE));
    }
    if options.json {
        body.insert("response_format".into(), json_format(provider));
    }
    if options.reasoning {
        for (key, value) in reasoning_fields(provider, model) {
            body.insert(key.into(), value);
        }
    }
    Value::Object(body)
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    message: Option<ResponseMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<Value>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

fn content_text(value: Option<Value>) -> String {
    match value {
        Some(Value::String(text)) => text,
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    }
}

pub fn completion(body: &str) -> Result<Completion, Fault> {
    let parsed: ChatResponse =
        serde_json::from_str(body).map_err(|e| Fault::Malformed(cut(&e.to_string())))?;
    let Some(choice) = parsed.choices.into_iter().next() else {
        let info = error_info(body);
        if info.message.trim().is_empty() {
            return Err(Fault::Malformed("в ответе нет вариантов".into()));
        }
        let status = info.code.parse::<u16>().unwrap_or(502);
        return Err(fault(status, body, None));
    };
    let usage = parsed.usage.unwrap_or_default();
    let answered = usage
        .completion_tokens
        .max(usage.total_tokens.saturating_sub(usage.prompt_tokens));
    Ok(Completion {
        content: content_text(choice.message.and_then(|m| m.content)),
        finish_reason: choice.finish_reason,
        prompt_tokens: tokens(usage.prompt_tokens),
        completion_tokens: tokens(answered),
    })
}

#[derive(Deserialize)]
struct ModelList {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    pricing: Option<Pricing>,
    #[serde(default)]
    architecture: Option<Architecture>,
    #[serde(default)]
    context_length: Option<Value>,
    #[serde(default)]
    context_window: Option<Value>,
    #[serde(default)]
    max_input_tokens: Option<Value>,
    #[serde(default)]
    top_provider: Option<TopProvider>,
}

#[derive(Deserialize)]
struct TopProvider {
    #[serde(default)]
    context_length: Option<Value>,
}

#[derive(Deserialize)]
struct Pricing {
    #[serde(default)]
    prompt: Option<Value>,
    #[serde(default)]
    completion: Option<Value>,
}

#[derive(Deserialize)]
struct Architecture {
    #[serde(default)]
    output_modalities: Option<Vec<String>>,
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

fn zero(value: Option<&Value>) -> bool {
    number(value) == Some(0.0)
}

fn per_million(value: Option<&Value>) -> Option<f64> {
    number(value)
        .filter(|n| *n >= 0.0)
        .map(|n| (n * 1_000_000.0 * 10_000.0).round() / 10_000.0)
}

fn context(entry: &ModelEntry) -> Option<u64> {
    [
        entry.context_length.as_ref(),
        entry.context_window.as_ref(),
        entry.max_input_tokens.as_ref(),
        entry
            .top_provider
            .as_ref()
            .and_then(|t| t.context_length.as_ref()),
    ]
    .into_iter()
    .filter_map(number)
    .find(|n| *n >= 1.0)
    .map(|n| n as u64)
}

fn chat_model(provider: Provider, entry: &ModelEntry, id: &str) -> bool {
    let lowered = id.to_lowercase();
    if lowered.is_empty() || any(&lowered, &NOT_CHAT) {
        return false;
    }
    if provider == Provider::Gemini
        && !(lowered.starts_with("gemini") || lowered.starts_with("gemma"))
    {
        return false;
    }
    entry
        .architecture
        .as_ref()
        .and_then(|a| a.output_modalities.as_ref())
        .is_none_or(|out| out.iter().any(|m| m == "text"))
}

pub fn model_options(provider: Provider, body: &str) -> Result<Vec<ModelOption>, Fault> {
    let parsed: ModelList =
        serde_json::from_str(body).map_err(|e| Fault::Malformed(cut(&e.to_string())))?;
    let mut entries: Vec<(ModelEntry, String)> = parsed
        .data
        .into_iter()
        .map(|entry| {
            let id = provider.model_id(&entry.id);
            (entry, id)
        })
        .filter(|(entry, id)| chat_model(provider, entry, id))
        .collect();
    if entries.iter().any(|(e, _)| e.created.is_some()) {
        entries.sort_by_key(|(e, _)| std::cmp::Reverse(e.created.unwrap_or(0)));
    }
    let mut seen = std::collections::HashSet::new();
    let mut options: Vec<ModelOption> = Vec::new();
    for (entry, id) in entries {
        if !seen.insert(id.clone()) {
            continue;
        }
        let pricing = entry.pricing.as_ref();
        let free = provider == Provider::OpenRouter
            && (id.ends_with(":free")
                || pricing.is_some_and(|p| zero(p.prompt.as_ref()) && zero(p.completion.as_ref())));
        options.push(ModelOption {
            context: context(&entry),
            price_in: pricing.and_then(|p| per_million(p.prompt.as_ref())),
            price_out: pricing.and_then(|p| per_million(p.completion.as_ref())),
            id,
            free,
        });
    }
    options.sort_by_key(|o| !o.free);
    Ok(options)
}

pub fn shortlist(mut options: Vec<ModelOption>) -> Vec<ModelOption> {
    options.truncate(MODELS_MAX);
    options
}

fn loopback(url: &reqwest::Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

pub fn check_address(raw: &str, strict: bool) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("укажите адрес API".into());
    }
    let url = reqwest::Url::parse(trimmed)
        .map_err(|_| "нужен адрес вида https://api.example.com/v1".to_string())?;
    let secure = url.scheme() == "https" || (!strict && url.scheme() == "http" && loopback(&url));
    if !secure || url.host_str().is_none() {
        return Err("адрес должен начинаться с https://".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("в адресе не должно быть логина и пароля".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("в адресе не должно быть параметров после ? и #".into());
    }
    Ok(trimmed.to_string())
}

fn http(proxy: Option<&str>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")));
    if let Some(raw) = proxy.map(str::trim).filter(|value| !value.is_empty()) {
        match reqwest::Proxy::all(raw) {
            Ok(proxy) => builder = builder.proxy(proxy),
            Err(err) => tracing::warn!(
                proxy = %sn_domain::settings::mask_proxy(raw),
                error = %err,
                "прокси нейросети не разобран, запрос пойдёт напрямую"
            ),
        }
    }
    builder.build().unwrap_or_default()
}

fn transport(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "истекло время ожидания ответа".into()
    } else if err.is_connect() {
        "не удалось соединиться с сервером".into()
    } else {
        cut(&err.to_string())
    }
}

#[derive(Clone)]
pub struct Llm {
    http: reqwest::Client,
    provider: Provider,
    base: String,
    key: String,
}

impl Llm {
    pub fn new(
        provider: Provider,
        api_url: &str,
        key: &str,
        proxy: Option<&str>,
    ) -> Result<Self, AiError> {
        let address = if api_url.trim().is_empty() {
            provider.base_url().unwrap_or_default()
        } else {
            api_url
        };
        let base = check_address(address, false)
            .map_err(|problem| AiError::new(provider, Fault::Address(problem)))?;
        Ok(Llm {
            http: http(proxy),
            provider,
            base,
            key: key.trim().to_string(),
        })
    }

    pub const fn provider(&self) -> Provider {
        self.provider
    }

    pub fn source(&self) -> super::models::Source {
        super::models::Source::new(self.provider, &self.base, &self.key)
    }

    pub const fn error(&self, fault: Fault) -> AiError {
        AiError::new(self.provider, fault)
    }

    fn scrub(&self, text: &str) -> String {
        if self.key.chars().count() >= 8 {
            text.replace(&self.key, "***")
        } else {
            text.to_string()
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<String, AiError> {
        let request = match self.provider {
            Provider::Anthropic => request
                .header("x-api-key", &self.key)
                .header("anthropic-version", anthropic::VERSION),
            _ => request.bearer_auth(&self.key),
        };
        let response = request
            .send()
            .await
            .map_err(|e| self.error(Fault::Network(transport(&e.without_url()))))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|s| s.ceil().max(0.0) as u64);
        let body = response
            .text()
            .await
            .map_err(|e| self.error(Fault::Network(transport(&e.without_url()))))?;
        let body = self.scrub(&body);
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            Err(self.error(fault(status, &body, retry_after)))
        }
    }

    pub async fn chat(
        &self,
        model: &str,
        messages: &[ChatMessage],
        options: ChatOptions,
    ) -> Result<Completion, AiError> {
        let model = self.provider.model_id(model);
        if self.provider == Provider::Anthropic {
            let body = anthropic::body(&model, messages, options);
            let text = self
                .send(
                    self.http
                        .post(format!("{}/messages", self.base))
                        .json(&body),
                )
                .await?;
            return anthropic::completion(&text).map_err(|f| self.error(f));
        }
        let body = request_body(self.provider, &model, messages, options);
        let text = self
            .send(
                self.http
                    .post(format!("{}/chat/completions", self.base))
                    .json(&body),
            )
            .await?;
        completion(&text).map_err(|f| self.error(f))
    }

    pub async fn models(&self) -> Result<Vec<ModelOption>, AiError> {
        let url = match self.provider {
            Provider::Anthropic => format!("{}/models?limit=1000", self.base),
            _ => format!("{}/models", self.base),
        };
        let text = self.send(self.http.get(url)).await?;
        model_options(self.provider, &text).map_err(|f| self.error(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(provider: Provider, model: &str) -> Value {
        let messages = [ChatMessage::system("правила"), ChatMessage::user("привет")];
        request_body(
            provider,
            model,
            &messages,
            ChatOptions::full(700, provider, model),
        )
    }

    fn ids(options: &[ModelOption]) -> Vec<&str> {
        options.iter().map(|o| o.id.as_str()).collect()
    }

    fn text(provider: Provider, status: u16, body: &str) -> String {
        AiError::new(provider, fault(status, body, None)).to_string()
    }

    #[test]
    fn тело_запроса_у_каждого_провайдера_своё() {
        let groq = body(Provider::Groq, "openai/gpt-oss-120b");
        assert_eq!(groq["max_completion_tokens"], 700);
        assert_eq!(groq["temperature"], 0.2);
        assert_eq!(groq["reasoning_effort"], "low");
        assert_eq!(groq["include_reasoning"], false);
        assert_eq!(groq["response_format"]["type"], "json_object");
        assert_eq!(groq["messages"][0]["role"], "system");
        let qwen = body(Provider::Groq, "qwen/qwen3.8-27b");
        assert_eq!(qwen["reasoning_format"], "hidden");
        assert!(qwen.get("reasoning_effort").is_none());

        let gemini = body(Provider::Gemini, "gemini-3.8-flash");
        assert_eq!(gemini["max_tokens"], 700);
        assert!(gemini.get("max_completion_tokens").is_none());
        assert!(gemini.get("temperature").is_none());
        assert_eq!(gemini["reasoning_effort"], "low");
        assert_eq!(gemini["response_format"]["type"], "json_schema");
        assert_eq!(
            gemini["response_format"]["json_schema"]["schema"]["required"],
            json!(["reply", "handoff", "reason"])
        );
        assert!(!ChatOptions::full(700, Provider::Gemini, "gemini-3.5-flash-lite").reasoning);
        assert!(
            body(Provider::Gemini, "gemini-3.5-flash-lite")
                .get("reasoning_effort")
                .is_none()
        );
        assert_eq!(
            body(Provider::Gemini, "gemini-2.5-flash")["reasoning_effort"],
            "none"
        );

        let deepseek = body(Provider::DeepSeek, "deepseek-flash");
        assert_eq!(deepseek["max_tokens"], 700);
        assert_eq!(deepseek["thinking"], json!({ "type": "disabled" }));
        assert_eq!(deepseek["response_format"]["type"], "json_object");
        assert_eq!(deepseek["temperature"], 0.2);

        let router = body(Provider::OpenRouter, "openai/gpt-oss-120b");
        assert_eq!(router["max_tokens"], 700);
        assert_eq!(
            router["reasoning"],
            json!({ "effort": "low", "exclude": true })
        );
        assert_eq!(router["response_format"]["type"], "json_object");
        assert!(router.get("reasoning_effort").is_none());

        let openai = body(Provider::OpenAi, "gpt-6-luna");
        assert_eq!(openai["max_completion_tokens"], 700);
        assert!(openai.get("max_tokens").is_none());
        assert!(openai.get("temperature").is_none());
        assert_eq!(openai["reasoning_effort"], "low");
        assert_eq!(openai["response_format"]["json_schema"]["strict"], true);

        let custom = body(Provider::Custom, "llama-4");
        assert_eq!(custom["max_tokens"], 700);
        assert_eq!(custom["temperature"], 0.2);
        assert_eq!(custom["response_format"]["type"], "json_object");
        assert!(!ChatOptions::full(700, Provider::Custom, "llama-4").reasoning);
        for key in [
            "reasoning_effort",
            "reasoning",
            "thinking",
            "reasoning_format",
            "include_reasoning",
        ] {
            assert!(custom.get(key).is_none(), "{key}");
        }
    }

    #[test]
    fn запасные_запросы_снимают_параметры_по_очереди() {
        let start = ChatOptions::full(500, Provider::OpenAi, "gpt-4.1");
        assert!(start.reasoning && start.json && !start.temperature);
        let calm = start
            .degrade(&Fault::Unsupported(Param::Reasoning, String::new()))
            .unwrap();
        assert!(!calm.reasoning && calm.json);
        let plain = calm.degrade(&Fault::Rejected("bad".into())).unwrap();
        assert!(!plain.json);
        assert_eq!(plain.degrade(&Fault::Rejected("bad".into())), None);
        assert_eq!(start.degrade(&Fault::Unauthorized), None);
        assert_eq!(start.degrade(&Fault::Unavailable(503)), None);
        assert_eq!(start.degrade(&Fault::Redirect), None);
        let custom = ChatOptions::full(500, Provider::Custom, "x");
        let swapped = custom
            .degrade(&Fault::Unsupported(Param::Tokens, String::new()))
            .unwrap();
        assert!(swapped.other_tokens);
        let swapped_body = request_body(Provider::Custom, "x", &[], swapped);
        assert_eq!(swapped_body["max_completion_tokens"], 500);
        assert!(swapped_body.get("max_tokens").is_none());
        assert_eq!(
            custom.degrade(&Fault::JsonFailed).map(|o| o.json),
            Some(false)
        );
        let groq = ChatOptions::full(500, Provider::Groq, "openai/gpt-oss-20b");
        let steps: Vec<ChatOptions> =
            std::iter::successors(Some(groq), |o| o.degrade(&Fault::Rejected(String::new())))
                .collect();
        assert_eq!(steps.len(), 4);
        assert!(!steps[3].temperature && !steps[3].json && !steps[3].reasoning);
    }

    #[test]
    fn ошибки_каждого_провайдера_понятны_оператору() {
        assert_eq!(
            fault(
                401,
                r#"{"error":{"message":"Invalid API Key","type":"invalid_request_error","code":"invalid_api_key"}}"#,
                None
            ),
            Fault::Unauthorized
        );
        assert_eq!(
            fault(
                400,
                r#"{"error":{"message":"Failed to generate JSON","code":"json_validate_failed"}}"#,
                None
            ),
            Fault::JsonFailed
        );
        assert!(matches!(
            fault(
                400,
                r#"{"error":{"message":"The model `x` has been decommissioned","code":"model_decommissioned"}}"#,
                None
            ),
            Fault::Model(_)
        ));
        assert_eq!(
            fault(429, "{}", Some(3)),
            Fault::RateLimited {
                retry_after: Some(3)
            }
        );
        assert_eq!(fault(503, "upstream down", None), Fault::Unavailable(503));

        assert_eq!(
            fault(
                400,
                r#"[{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID"}]}}]"#,
                None
            ),
            Fault::Unauthorized
        );
        assert!(matches!(
            fault(
                400,
                r#"[{"error":{"code":400,"message":"User location is not supported for the API use.","status":"FAILED_PRECONDITION"}}]"#,
                None
            ),
            Fault::Region(_)
        ));
        assert!(matches!(
            fault(
                400,
                r#"[{"error":{"code":400,"message":"Invalid JSON payload received. Unknown name \"reasoning_effort\": Cannot find field.","status":"INVALID_ARGUMENT"}}]"#,
                None
            ),
            Fault::Unsupported(Param::Reasoning, _)
        ));
        assert_eq!(
            fault(
                429,
                r#"[{"error":{"code":429,"message":"Resource has been exhausted","status":"RESOURCE_EXHAUSTED"}}]"#,
                None
            ),
            Fault::RateLimited { retry_after: None }
        );

        assert_eq!(
            fault(
                401,
                r#"{"error":{"message":"Authentication Fails, Your api key: ****alid is invalid","type":"authentication_error","param":null,"code":"invalid_request_error"}}"#,
                None
            ),
            Fault::Unauthorized
        );
        assert!(matches!(
            fault(402, r#"{"error":{"message":"Insufficient Balance","type":"unknown_error","param":null,"code":"invalid_request_error"}}"#, None),
            Fault::Payment(m) if m == "Insufficient Balance"
        ));

        assert!(matches!(
            fault(
                402,
                r#"{"error":{"code":402,"message":"Insufficient credits. Add more using https://openrouter.ai/settings/credits"}}"#,
                None
            ),
            Fault::Payment(_)
        ));
        assert!(matches!(
            fault(
                404,
                r#"{"error":{"code":404,"message":"No endpoints found matching your data policy (Free model publication)."}}"#,
                None
            ),
            Fault::Policy(_)
        ));
        assert!(matches!(
            fault(
                403,
                r#"{"error":{"code":403,"message":"Input was flagged by moderation","metadata":{"reasons":["violence"]}}}"#,
                None
            ),
            Fault::Forbidden(_)
        ));

        assert!(matches!(
            fault(
                400,
                r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.","type":"invalid_request_error","param":"max_tokens","code":"unsupported_parameter"}}"#,
                None
            ),
            Fault::Unsupported(Param::Tokens, _)
        ));
        assert!(matches!(
            fault(
                400,
                r#"{"error":{"message":"Unsupported value: 'temperature' does not support 0.2 with this model. Only the default (1) value is supported.","type":"invalid_request_error","param":"temperature","code":"unsupported_value"}}"#,
                None
            ),
            Fault::Unsupported(Param::Temperature, _)
        ));
        assert!(matches!(
            fault(
                400,
                r#"{"error":{"message":"Invalid parameter: 'response_format' of type 'json_object' is not supported with this model.","type":"invalid_request_error","param":"response_format","code":null}}"#,
                None
            ),
            Fault::Unsupported(Param::Json, _)
        ));
        assert!(matches!(
            fault(
                429,
                r#"{"error":{"message":"You exceeded your current quota, please check your plan and billing details.","type":"insufficient_quota","code":"insufficient_quota"}}"#,
                None
            ),
            Fault::Payment(_)
        ));
        assert!(matches!(
            fault(
                403,
                r#"{"error":{"code":"unsupported_country_region_territory","message":"Country, region, or territory not supported","param":null,"type":"request_forbidden"}}"#,
                None
            ),
            Fault::Region(_)
        ));
        assert!(matches!(
            fault(
                404,
                r#"{"error":{"message":"The model `gpt-9` does not exist or you do not have access to it.","type":"invalid_request_error","code":"model_not_found"}}"#,
                None
            ),
            Fault::Model(_)
        ));

        assert_eq!(
            fault(
                401,
                r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
                None
            ),
            Fault::Unauthorized
        );
        assert!(matches!(
            fault(
                403,
                r#"{"type":"error","error":{"type":"permission_error","message":"Request not allowed"}}"#,
                None
            ),
            Fault::Forbidden(_)
        ));
        assert!(matches!(
            fault(
                404,
                r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-x"}}"#,
                None
            ),
            Fault::Model(_)
        ));
        assert_eq!(
            fault(
                529,
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                None
            ),
            Fault::Unavailable(529)
        );
        assert!(matches!(
            fault(
                402,
                r#"{"type":"error","error":{"type":"billing_error","message":"Your credit balance is too low"}}"#,
                None
            ),
            Fault::Payment(_)
        ));
        assert!(matches!(
            fault(
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"output_config.effort: this model does not support effort"}}"#,
                None
            ),
            Fault::Unsupported(Param::Reasoning, _)
        ));
        assert!(matches!(
            fault(
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"output_config.format: structured outputs are not supported for this model"}}"#,
                None
            ),
            Fault::Unsupported(Param::Json, _)
        ));
        assert_eq!(
            fault(
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 1200000 tokens > 1000000 maximum"}}"#,
                None
            ),
            Fault::TooLong
        );
        assert_eq!(fault(307, "", None), Fault::Redirect);
        assert!(fault(503, "", None).retryable());
        assert!(!Fault::Unauthorized.retryable());

        let groq = text(Provider::Groq, 403, r#"{"error":{"message":"Forbidden"}}"#);
        assert!(
            groq.starts_with("Groq отказал в доступе (403): Forbidden"),
            "{groq}"
        );
        assert!(groq.contains("console.groq.com/keys"), "{groq}");
        assert!(groq.contains("выберите DeepSeek или OpenRouter"), "{groq}");
        let deepseek = text(
            Provider::DeepSeek,
            403,
            r#"{"error":{"message":"Forbidden"}}"#,
        );
        assert!(
            deepseek.contains("platform.deepseek.com/api_keys"),
            "{deepseek}"
        );
        assert!(!deepseek.contains("страны"), "{deepseek}");
        assert_eq!(
            text(Provider::Gemini, 401, "{}"),
            "Google Gemini не принял ключ: проверьте его на aistudio.google.com/apikey"
        );
        assert_eq!(
            text(Provider::Anthropic, 401, "{}"),
            "Anthropic не принял ключ: проверьте его на platform.claude.com/settings/keys"
        );
        assert!(text(Provider::Custom, 401, "{}").starts_with("Сервер API не принял ключ"));
        assert!(
            text(
                Provider::OpenRouter,
                402,
                r#"{"error":{"code":402,"message":"Insufficient credits"}}"#
            )
            .contains(":free")
        );
        assert!(
            text(Provider::OpenAi, 403, r#"{"error":{"code":"unsupported_country_region_territory","message":"Country, region, or territory not supported"}}"#)
                .starts_with("OpenAI недоступен из страны сервера панели")
        );
        assert_eq!(
            text(Provider::OpenAi, 503, ""),
            "OpenAI временно недоступен (код 503)"
        );
        for provider in Provider::ALL.into_iter().filter(|p| *p != Provider::Groq) {
            for status in [401, 403, 404, 429, 500] {
                assert!(!text(provider, status, "{}").contains("Groq") || status == 403);
            }
        }
    }

    #[test]
    fn ответ_совместимого_api_разбирается() {
        let plain = completion(
            r#"{"choices":[{"message":{"role":"assistant","content":"{\"reply\":\"ok\"}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":180}}"#,
        )
        .unwrap();
        assert_eq!(plain.content, r#"{"reply":"ok"}"#);
        assert_eq!((plain.prompt_tokens, plain.completion_tokens), (100, 80));
        let parts = completion(
            r#"{"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]},"finish_reason":"length"}]}"#,
        )
        .unwrap();
        assert_eq!(parts.content, "ab");
        assert_eq!(parts.finish_reason.as_deref(), Some("length"));
        assert_eq!(
            completion(r#"{"error":{"code":502,"message":"Provider returned error"}}"#),
            Err(Fault::Unavailable(502))
        );
        assert!(matches!(
            completion(r#"{"choices":[]}"#),
            Err(Fault::Malformed(_))
        ));
    }

    #[test]
    fn список_моделей_разбирается_у_каждого_провайдера() {
        let gemini = model_options(
            Provider::Gemini,
            r#"{"object":"list","data":[{"id":"models/gemini-3.8-flash","object":"model","owned_by":"google"},{"id":"models/gemini-embedding-2","object":"model"},{"id":"models/imagen-4.0-generate","object":"model"},{"id":"gemini-3.5-flash-lite","object":"model"},{"id":"models/gemini-2.5-flash-preview-tts"},{"id":"models/gemini-3.8-flash"}]}"#,
        )
        .unwrap();
        assert_eq!(ids(&gemini), ["gemini-3.8-flash", "gemini-3.5-flash-lite"]);

        let router = model_options(
            Provider::OpenRouter,
            r#"{"data":[{"id":"openai/gpt-oss-120b","created":1754414231,"context_length":131072,"pricing":{"prompt":"0.00000015","completion":"0.0000006"},"architecture":{"output_modalities":["text"]}},{"id":"google/gemma-4-31b-it:free","created":1775148486,"context_length":262144,"pricing":{"prompt":"0","completion":"0"},"architecture":{"output_modalities":["text"]}},{"id":"openrouter/free","created":1769917427,"pricing":{"prompt":"0","completion":"0"},"top_provider":{"context_length":null}},{"id":"openrouter/auto","created":1700000000,"pricing":{"prompt":"-1","completion":"-1"}},{"id":"google/lyria-3-pro-preview","created":1790000000,"pricing":{"prompt":"0","completion":"0"},"architecture":{"output_modalities":["audio"]}},{"id":"qwen/qwen3.7-flash","created":1785190561,"top_provider":{"context_length":65536},"pricing":{"prompt":"0.00000003","completion":"0.00000013"}}]}"#,
        )
        .unwrap();
        assert_eq!(
            ids(&router),
            [
                "google/gemma-4-31b-it:free",
                "openrouter/free",
                "qwen/qwen3.7-flash",
                "openai/gpt-oss-120b",
                "openrouter/auto"
            ]
        );
        assert!(router[0].free && router[1].free && !router[2].free);
        assert_eq!(router[0].context, Some(262_144));
        assert_eq!(
            (router[0].price_in, router[0].price_out),
            (Some(0.0), Some(0.0))
        );
        assert_eq!(router[3].context, Some(131_072));
        assert_eq!(
            (router[3].price_in, router[3].price_out),
            (Some(0.15), Some(0.6))
        );
        assert_eq!(router[2].context, Some(65_536));
        assert_eq!(router[1].context, None);
        assert_eq!((router[4].price_in, router[4].price_out), (None, None));
        assert!(!router[4].free);

        let openai = model_options(
            Provider::OpenAi,
            r#"{"object":"list","data":[{"id":"gpt-4.1","created":1744316542,"owned_by":"system"},{"id":"text-embedding-3-small","created":1705948997},{"id":"gpt-6-luna","created":1790000000},{"id":"gpt-realtime","created":1790000001},{"id":"whisper-1","created":1677532384},{"id":"gpt-image-2","created":1790000002},{"id":"tts-1","created":1681940951},{"id":"omni-moderation-latest","created":1731689265}]}"#,
        )
        .unwrap();
        assert_eq!(ids(&openai), ["gpt-6-luna", "gpt-4.1"]);
        assert!(openai.iter().all(|m| !m.free));

        let claude = model_options(
            Provider::Anthropic,
            r#"{"data":[{"type":"model","id":"claude-haiku-5-5","display_name":"Claude Haiku 5.5","created_at":"2026-10-01T00:00:00Z","max_input_tokens":1000000,"max_tokens":128000},{"type":"model","id":"claude-opus-5-5","display_name":"Claude Opus 5.5","created_at":"2026-09-01T00:00:00Z"}],"has_more":false}"#,
        )
        .unwrap();
        assert_eq!(ids(&claude), ["claude-haiku-5-5", "claude-opus-5-5"]);
        assert_eq!(claude[0].context, Some(1_000_000));

        let groq = model_options(
            Provider::Groq,
            r#"{"data":[{"id":"openai/gpt-oss-120b","created":1754000000,"context_window":131072},{"id":"whisper-large-v3","created":1700000000},{"id":"meta-llama/llama-guard-4-12b","created":1750000000},{"id":"playai-tts","created":1740000000},{"id":"qwen/qwen3.8-27b","created":1790000000}]}"#,
        )
        .unwrap();
        assert_eq!(ids(&groq), ["qwen/qwen3.8-27b", "openai/gpt-oss-120b"]);
        assert_eq!(groq[1].context, Some(131_072));
        assert!(groq.iter().all(|m| m.price_in.is_none() && !m.free));

        let deepseek = model_options(
            Provider::DeepSeek,
            r#"{"object":"list","data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek","context_window":1048576},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )
        .unwrap();
        assert_eq!(ids(&deepseek), ["deepseek-flash", "deepseek-v4-pro"]);
        assert_eq!(deepseek[0].context, Some(1_048_576));
        assert!(
            gemini
                .iter()
                .all(|m| m.context.is_none() && m.price_in.is_none())
        );

        let many: Vec<ModelOption> = (0..1_200)
            .map(|i| ModelOption {
                id: format!("m{i}"),
                free: false,
                context: None,
                price_in: None,
                price_out: None,
            })
            .collect();
        assert_eq!(shortlist(many).len(), MODELS_MAX);
        assert!(matches!(
            model_options(Provider::Groq, "<html>"),
            Err(Fault::Malformed(_))
        ));
    }

    #[test]
    fn адрес_api_только_https_без_логина_и_параметров() {
        assert_eq!(
            check_address("https://proxy.example.com/v1/", true).unwrap(),
            "https://proxy.example.com/v1"
        );
        assert!(check_address("http://proxy.example.com/v1", true).is_err());
        assert!(check_address("http://127.0.0.1:8080", true).is_err());
        assert!(check_address("http://127.0.0.1:8080", false).is_ok());
        assert!(check_address("http://proxy.example.com", false).is_err());
        assert!(check_address("https://user:pass@proxy.example.com", true).is_err());
        assert!(check_address("https://proxy.example.com/v1?key=1", true).is_err());
        assert!(check_address("ftp://proxy.example.com", true).is_err());
        assert!(check_address("не адрес", true).is_err());
        assert!(matches!(
            Llm::new(Provider::Custom, "", "k", None),
            Err(AiError {
                fault: Fault::Address(_),
                ..
            })
        ));
        assert!(Llm::new(Provider::Groq, "", "k", None).is_ok());
    }
}
