#[cfg(feature = "live")]
use std::sync::{Arc, RwLock};

#[cfg(feature = "live")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(feature = "live")]
use serde_json::Value;

#[cfg(feature = "live")]
use crate::catalog::Catalog;
#[cfg(feature = "live")]
use crate::normalize::{normalize_litellm, normalize_openrouter};

#[cfg(feature = "live")]
pub const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
#[cfg(feature = "live")]
pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";

#[cfg(feature = "live")]
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
#[cfg(feature = "live")]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(feature = "live")]
#[derive(Debug, Clone)]
pub struct PricingCache {
    snapshot: Arc<RwLock<Arc<Catalog>>>,
}

#[cfg(feature = "live")]
impl PricingCache {
    pub fn new(initial: Catalog) -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(Arc::new(initial))),
        }
    }

    pub fn snapshot(&self) -> Arc<Catalog> {
        self.snapshot
            .read()
            .map(|guard| Arc::clone(&guard))
            .unwrap_or_else(|poisoned| Arc::clone(&poisoned.into_inner()))
    }

    pub fn price_for(&self, model: &str) -> Option<crate::schema::ModelPrice> {
        self.snapshot().price_for(model)
    }

    pub fn start_refresh_task(&self) -> tokio::task::JoinHandle<()> {
        let cache = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(REFRESH_INTERVAL);
            tokio::time::sleep(jitter()).await;
            loop {
                if let Ok(catalog) = fetch_catalog().await {
                    cache.swap(catalog);
                }
                interval.tick().await;
            }
        })
    }

    pub async fn refresh_once(&self) -> Result<(), CacheError> {
        let catalog = fetch_catalog().await?;
        self.swap(catalog);
        Ok(())
    }

    fn swap(&self, catalog: Catalog) {
        let mut guard = self
            .snapshot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Arc::new(catalog);
    }
}

#[cfg(feature = "live")]
pub async fn fetch_catalog() -> Result<Catalog, CacheError> {
    assert_fetch_allowed(LITELLM_URL)?;
    assert_fetch_allowed(OPENROUTER_URL)?;

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(CacheError::Request)?;

    let litellm = fetch_json(&client, LITELLM_URL).await?;
    let openrouter = fetch_json(&client, OPENROUTER_URL).await?;

    let mut catalog = Catalog::empty();
    if let Value::Object(entries) = litellm {
        for (id, entry) in entries {
            catalog.insert(normalize_litellm(&id, &entry));
        }
    }
    if let Some(Value::Array(models)) = openrouter.get("data") {
        for model in models {
            if let (Some(id), Some(pricing)) = (
                model.get("id").and_then(Value::as_str),
                model.get("pricing"),
            ) {
                catalog.insert(normalize_openrouter(id, pricing));
            }
        }
    }

    Ok(catalog)
}

#[cfg(feature = "live")]
async fn fetch_json(client: &reqwest::Client, url: &str) -> Result<Value, CacheError> {
    assert_fetch_allowed(url)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(CacheError::Request)?
        .error_for_status()
        .map_err(CacheError::Request)?;
    response.json::<Value>().await.map_err(CacheError::Request)
}

#[cfg(feature = "live")]
fn jitter() -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    Duration::from_secs((nanos % 300) as u64)
}

#[cfg(feature = "live")]
fn assert_fetch_allowed(url: &str) -> Result<(), CacheError> {
    let host = host_from_https_url(url).ok_or(CacheError::BlockedHost)?;
    let host = host.to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".local")
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("169.254.")
        || host == "::1"
        || host.starts_with("fc")
        || host.starts_with("fd")
        || is_private_172(&host)
    {
        return Err(CacheError::BlockedHost);
    }
    Ok(())
}

#[cfg(feature = "live")]
fn host_from_https_url(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split('/').next()?;
    Some(authority.split('@').next_back()?.split(':').next()?)
}

#[cfg(feature = "live")]
fn is_private_172(host: &str) -> bool {
    let Some(rest) = host.strip_prefix("172.") else {
        return false;
    };
    let Some(second) = rest
        .split('.')
        .next()
        .and_then(|part| part.parse::<u8>().ok())
    else {
        return false;
    };
    (16..=31).contains(&second)
}

#[cfg(feature = "live")]
#[derive(Debug)]
pub enum CacheError {
    BlockedHost,
    Request(reqwest::Error),
}

#[cfg(feature = "live")]
impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlockedHost => write!(f, "pricing fetch host rejected by SSRF guard"),
            Self::Request(err) => write!(f, "pricing fetch request failed: {err}"),
        }
    }
}

#[cfg(feature = "live")]
impl std::error::Error for CacheError {}
