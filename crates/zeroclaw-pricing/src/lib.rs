pub mod cache;
pub mod canon;
pub mod catalog;
pub mod cost;
pub mod normalize;
pub mod schema;

pub use canon::canonicalize;
pub use catalog::{Catalog, CatalogError};
pub use cost::cost;
pub use normalize::{normalize_litellm, normalize_openrouter, to_per_million};
pub use schema::{ModelPrice, Source};

#[cfg(feature = "live")]
pub use cache::*;
#[cfg(feature = "live")]
use std::sync::OnceLock;

#[cfg(feature = "live")]
static LIVE_CACHE: OnceLock<PricingCache> = OnceLock::new();

pub fn offline_catalog() -> Catalog {
    Catalog::from_json_str(include_str!("../pricing.json")).unwrap_or_else(|_| Catalog::empty())
}

#[cfg(feature = "live")]
pub fn init_live_cache(initial: Catalog) -> &'static PricingCache {
    LIVE_CACHE.get_or_init(|| PricingCache::new(initial))
}

pub fn price(model: &str) -> Option<ModelPrice> {
    #[cfg(feature = "live")]
    {
        if let Some(price) = LIVE_CACHE.get().and_then(|cache| cache.price_for(model)) {
            return Some(price);
        }
    }

    offline_catalog().price_for(model)
}
