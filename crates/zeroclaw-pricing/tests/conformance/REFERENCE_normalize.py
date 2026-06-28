"""Reference normalizer for the pricing-engine spec (docs/pricing-engine-spec.md).

This is the SINGLE authoritative normalization codepath. The `tokenomics` (TS) and
`zoder-engine` (Rust) outputs must reproduce its results on the conformance vectors.
It also forms the core of the offline-catalog "updater".
"""
from __future__ import annotations

PER_MILLION = 1_000_000.0
# Only SLASH-style routing prefixes are stripped (provider/model). Dotted Bedrock-style
# ids (amazon.nova-..., anthropic.claude-...) ARE the canonical model id and are kept
# intact, matching how LiteLLM and hosts key them. The trailing generic "/"-split below
# covers any other provider/ prefix deterministically.
KNOWN_PREFIXES = ("openai/", "anthropic/", "nvidia/", "azure/", "google/", "meta/",
                  "mistralai/", "bedrock/", "sakana/")


def to_per_million(per_token):
    if per_token is None:
        return None
    try:
        v = float(per_token)
    except (TypeError, ValueError):
        return None
    if v != v or v < 0:  # NaN or negative sentinel
        return None
    return v * PER_MILLION


def canonicalize(model_id: str):
    """(provider, canonical_id) per spec §4."""
    s = model_id.strip().lower().lstrip("~")  # tilde = OpenRouter variant marker
    provider = None
    for pfx in KNOWN_PREFIXES:
        if s.startswith(pfx):
            provider = pfx.rstrip("/.")
            s = s[len(pfx):]
            break
    else:
        if "/" in s:
            provider, s = s.split("/", 1)
    return provider, s


def normalize_litellm(model_id: str, entry: dict, source="litellm"):
    _, canon = canonicalize(model_id)
    return {
        "model_id": canon,
        "input": to_per_million(entry.get("input_cost_per_token")),
        "output": to_per_million(entry.get("output_cost_per_token")),
        "cache_read": to_per_million(entry.get("cache_read_input_token_cost")),
        "cache_write": to_per_million(entry.get("cache_creation_input_token_cost")),
        "source": source,
    }


def normalize_openrouter(model_id: str, pricing: dict, source="openrouter"):
    _, canon = canonicalize(model_id)
    return {
        "model_id": canon,
        "input": to_per_million(pricing.get("prompt")),
        "output": to_per_million(pricing.get("completion")),
        "cache_read": to_per_million(pricing.get("input_cache_read")),
        "cache_write": to_per_million(pricing.get("input_cache_write")),
        "source": source,
    }


def cost(price: dict, tokens_in=0, tokens_out=0, cache_read_tokens=0, cache_write_tokens=0):
    f = lambda x: x or 0.0
    return (tokens_in * f(price.get("input")) + tokens_out * f(price.get("output"))
            + cache_read_tokens * f(price.get("cache_read"))
            + cache_write_tokens * f(price.get("cache_write"))) / PER_MILLION
