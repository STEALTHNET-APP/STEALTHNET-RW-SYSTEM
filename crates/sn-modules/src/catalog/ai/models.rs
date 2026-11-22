use super::llm::{Llm, ModelOption};
use super::provider::Provider;
use sha2::{Digest, Sha256};
use sn_domain::ids::AdminId;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

pub const FRESH: Duration = Duration::from_secs(300);
pub const FAILED: Duration = Duration::from_secs(60);
pub const PER_MINUTE: usize = 10;
const WINDOW: Duration = Duration::from_secs(60);
const KEEP: usize = 32;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Source {
    provider: Provider,
    base: String,
    key: [u8; 32],
}

impl Source {
    pub fn new(provider: Provider, base: &str, key: &str) -> Self {
        Source {
            provider,
            base: base.trim().trim_end_matches('/').to_string(),
            key: Sha256::digest(key.trim().as_bytes()).into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Listing {
    Models(Vec<ModelOption>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    Fresh(Listing),
    Cached(Listing),
    Limited,
}

struct Entry {
    at: Instant,
    listing: Listing,
}

impl Entry {
    fn alive(&self, now: Instant) -> bool {
        let ttl = match self.listing {
            Listing::Models(_) => FRESH,
            Listing::Failed(_) => FAILED,
        };
        now.saturating_duration_since(self.at) < ttl
    }
}

#[derive(Default)]
pub struct Catalog {
    entries: Mutex<HashMap<Source, Entry>>,
    calls: Mutex<HashMap<AdminId, VecDeque<Instant>>>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Catalog {
    pub fn shared() -> &'static Catalog {
        static SHARED: OnceLock<Catalog> = OnceLock::new();
        SHARED.get_or_init(Catalog::default)
    }

    pub fn cached(&self, source: &Source, now: Instant) -> Option<Listing> {
        guard(&self.entries)
            .get(source)
            .filter(|entry| entry.alive(now))
            .map(|entry| entry.listing.clone())
    }

    pub fn store(&self, source: Source, listing: Listing, now: Instant) {
        let mut entries = guard(&self.entries);
        entries.retain(|_, entry| entry.alive(now));
        if entries.len() >= KEEP
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.at)
                .map(|(source, _)| source.clone())
        {
            entries.remove(&oldest);
        }
        entries.insert(source, Entry { at: now, listing });
    }

    pub fn allow(&self, admin: AdminId, now: Instant) -> bool {
        let mut calls = guard(&self.calls);
        calls.retain(|_, times| {
            times
                .back()
                .is_some_and(|last| now.saturating_duration_since(*last) < WINDOW)
        });
        let times = calls.entry(admin).or_default();
        while times
            .front()
            .is_some_and(|first| now.saturating_duration_since(*first) >= WINDOW)
        {
            times.pop_front();
        }
        if times.len() >= PER_MINUTE {
            return false;
        }
        times.push_back(now);
        true
    }

    pub async fn load(&self, admin: AdminId, llm: &Llm, refresh: bool) -> Loaded {
        let source = llm.source();
        if !refresh && let Some(listing) = self.cached(&source, Instant::now()) {
            return Loaded::Cached(listing);
        }
        if !self.allow(admin, Instant::now()) {
            return Loaded::Limited;
        }
        let listing = match llm.models().await {
            Ok(models) => Listing::Models(models),
            Err(err) => Listing::Failed(err.to_string()),
        };
        self.store(source, listing.clone(), Instant::now());
        Loaded::Fresh(listing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn option(id: &str) -> ModelOption {
        ModelOption {
            id: id.into(),
            free: false,
            context: None,
            price_in: None,
            price_out: None,
        }
    }

    fn listing(id: &str) -> Listing {
        Listing::Models(vec![option(id)])
    }

    #[test]
    fn кэш_держит_список_по_отпечатку_ключа_несколько_минут() {
        let catalog = Catalog::default();
        let now = Instant::now();
        let source = Source::new(
            Provider::OpenRouter,
            "https://openrouter.ai/api/v1/",
            " sk-or-1 ",
        );
        assert!(
            source
                == Source::new(
                    Provider::OpenRouter,
                    "https://openrouter.ai/api/v1",
                    "sk-or-1"
                )
        );
        assert!(
            source
                != Source::new(
                    Provider::OpenRouter,
                    "https://openrouter.ai/api/v1",
                    "sk-or-2"
                )
        );
        assert!(source != Source::new(Provider::Groq, "https://openrouter.ai/api/v1", "sk-or-1"));
        assert!(
            source
                != Source::new(
                    Provider::OpenRouter,
                    "https://proxy.example.com/v1",
                    "sk-or-1"
                )
        );
        assert_eq!(catalog.cached(&source, now), None);
        catalog.store(source.clone(), listing("a"), now);
        assert_eq!(
            catalog.cached(&source, now + Duration::from_secs(299)),
            Some(listing("a"))
        );
        assert_eq!(catalog.cached(&source, now + FRESH), None);
        let other = Source::new(
            Provider::OpenRouter,
            "https://openrouter.ai/api/v1",
            "sk-or-2",
        );
        assert_eq!(catalog.cached(&other, now), None);
        let failed = Listing::Failed("OpenRouter не принял ключ".into());
        catalog.store(other.clone(), failed.clone(), now);
        assert_eq!(
            catalog.cached(&other, now + Duration::from_secs(59)),
            Some(failed)
        );
        assert_eq!(catalog.cached(&other, now + FAILED), None);
        for n in 0..(KEEP + 5) {
            catalog.store(
                Source::new(
                    Provider::Groq,
                    "https://api.groq.com/openai/v1",
                    &format!("gsk-{n}"),
                ),
                listing("b"),
                now + Duration::from_millis(n as u64),
            );
        }
        assert!(guard(&catalog.entries).len() <= KEEP);
    }

    #[test]
    fn лимит_запросов_считает_каждого_админа_отдельно() {
        let catalog = Catalog::default();
        let now = Instant::now();
        for n in 0..PER_MINUTE {
            assert!(
                catalog.allow(AdminId(1), now + Duration::from_secs(n as u64)),
                "{n}"
            );
        }
        assert!(!catalog.allow(AdminId(1), now + Duration::from_secs(30)));
        assert!(catalog.allow(AdminId(2), now + Duration::from_secs(30)));
        assert!(catalog.allow(AdminId(1), now + Duration::from_secs(60)));
        assert!(!catalog.allow(AdminId(1), now + Duration::from_secs(60)));
        assert!(catalog.allow(AdminId(1), now + Duration::from_secs(200)));
    }

    #[tokio::test]
    async fn повторная_загрузка_берётся_из_кэша_а_обновление_идёт_к_провайдеру() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer sk-or-cache-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    { "id": "openai/gpt-oss-120b", "context_length": 131072,
                      "pricing": { "prompt": "0.00000015", "completion": "0.0000006" } },
                    { "id": "google/gemma-4-31b-it:free", "context_length": 262144,
                      "pricing": { "prompt": "0", "completion": "0" } }
                ]
            })))
            .expect(2)
            .mount(&server)
            .await;
        let catalog = Catalog::default();
        let llm = Llm::new(
            Provider::OpenRouter,
            &server.uri(),
            "sk-or-cache-test",
            None,
        )
        .unwrap();
        let first = catalog.load(AdminId(7), &llm, false).await;
        let Loaded::Fresh(Listing::Models(models)) = &first else {
            panic!("{first:?}");
        };
        assert_eq!(models[0].id, "google/gemma-4-31b-it:free");
        assert!(models[0].free);
        assert_eq!(models[1].context, Some(131_072));
        assert_eq!(
            (models[1].price_in, models[1].price_out),
            (Some(0.15), Some(0.6))
        );
        assert!(matches!(
            catalog.load(AdminId(7), &llm, false).await,
            Loaded::Cached(Listing::Models(_))
        ));
        assert!(matches!(
            catalog.load(AdminId(8), &llm, false).await,
            Loaded::Cached(Listing::Models(_))
        ));
        assert!(matches!(
            catalog.load(AdminId(7), &llm, true).await,
            Loaded::Fresh(Listing::Models(_))
        ));
        server.verify().await;
    }

    #[tokio::test]
    async fn частые_запросы_упираются_в_лимит_а_ошибка_ключа_не_раскрывает_его() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": { "code": 401, "message": "Invalid key sk-or-secret-value" }
            })))
            .mount(&server)
            .await;
        let catalog = Catalog::default();
        let llm = Llm::new(
            Provider::OpenRouter,
            &server.uri(),
            "sk-or-secret-value",
            None,
        )
        .unwrap();
        let Loaded::Fresh(Listing::Failed(text)) = catalog.load(AdminId(3), &llm, false).await
        else {
            panic!("ожидалась ошибка");
        };
        assert!(text.starts_with("OpenRouter не принял ключ"), "{text}");
        assert!(!text.contains("sk-or-secret-value"), "{text}");
        assert!(matches!(
            catalog.load(AdminId(3), &llm, false).await,
            Loaded::Cached(Listing::Failed(_))
        ));
        for _ in 1..PER_MINUTE {
            assert!(matches!(
                catalog.load(AdminId(3), &llm, true).await,
                Loaded::Fresh(_)
            ));
        }
        assert_eq!(catalog.load(AdminId(3), &llm, true).await, Loaded::Limited);
        assert_eq!(server.received_requests().await.unwrap().len(), PER_MINUTE);
        let other = Llm::new(
            Provider::OpenRouter,
            &server.uri(),
            "sk-or-other-value",
            None,
        )
        .unwrap();
        assert_eq!(
            catalog.load(AdminId(3), &other, false).await,
            Loaded::Limited
        );
        assert!(matches!(
            catalog.load(AdminId(4), &other, false).await,
            Loaded::Fresh(_)
        ));
    }
}
