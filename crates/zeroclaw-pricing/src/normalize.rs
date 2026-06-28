use serde_json::Value;

use crate::canon::canonicalize;
use crate::schema::{ModelPrice, Source};

const PER_MILLION: f64 = 1_000_000.0;

pub fn to_per_million(per_token: &Value) -> Option<f64> {
    let value = match per_token {
        Value::Null => return None,
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.parse::<f64>().ok()?,
        _ => return None,
    };

    if value.is_nan() || value < 0.0 {
        return None;
    }

    Some(value * PER_MILLION)
}

pub fn normalize_litellm(id: &str, entry: &Value) -> ModelPrice {
    let (_, model_id) = canonicalize(id);
    ModelPrice {
        model_id,
        input: to_per_million(entry.get("input_cost_per_token").unwrap_or(&Value::Null)),
        output: to_per_million(entry.get("output_cost_per_token").unwrap_or(&Value::Null)),
        cache_read: to_per_million(
            entry
                .get("cache_read_input_token_cost")
                .unwrap_or(&Value::Null),
        ),
        cache_write: to_per_million(
            entry
                .get("cache_creation_input_token_cost")
                .unwrap_or(&Value::Null),
        ),
        source: Source::LiteLLM,
    }
}

pub fn normalize_openrouter(id: &str, pricing: &Value) -> ModelPrice {
    let (_, model_id) = canonicalize(id);
    ModelPrice {
        model_id,
        input: to_per_million(pricing.get("prompt").unwrap_or(&Value::Null)),
        output: to_per_million(pricing.get("completion").unwrap_or(&Value::Null)),
        cache_read: to_per_million(pricing.get("input_cache_read").unwrap_or(&Value::Null)),
        cache_write: to_per_million(pricing.get("input_cache_write").unwrap_or(&Value::Null)),
        source: Source::OpenRouter,
    }
}
