# Pricing-Engine Spec — single source, two outputs

> **Canonical specification.** One spec; two conforming outputs:
> - **`tokenomics`** — TypeScript plugin for **OpenClaw**
> - **`zoder-engine`** — Rust crate for **zeroclaw** (shipped as a vendored `zoder-integration` fork patch)
>
> The two outputs MUST NOT carry two divergent normalization codepaths. Normalization is
> defined here once and enforced across both by the shared **conformance vectors** (§7).
> GRAEAE consultation `c3ca23f18c194417b1f3658841630c31` (2026-06-28) is the basis for this design.

## 1. Goal

Price every LLM call from live, auto-refreshing model pricing — never silently record `$0` for a
known model — with a deterministic, source-labeled resolution order and a faithful offline fallback.

## 2. Normalized pricing schema (the contract both outputs share)

All rates are **USD per 1,000,000 tokens** (per-million), non-negative finite floats. A negative or
absent value is the "unpriced/unknown" sentinel.

```
ModelPrice {
  model_id:   string        # canonical id (see §4)
  input:      f64 | null     # prompt tokens
  output:     f64 | null     # completion tokens
  cache_read: f64 | null     # cached-prompt read
  cache_write:f64 | null     # cache creation/write
  source:     "litellm" | "openrouter" | "catalog" | "host"   # provenance label
}
```

Cost of a call:
```
cost = (tokens_in * input + tokens_out * output
        + cache_read_tokens * cache_read + cache_write_tokens * cache_write) / 1e6
```
`tokens_in` MUST be the host's prompt total (`input + cacheRead + cacheWrite`) when available, so
cache-heavy calls are not underreported (this is the OpenClaw P2 already fixed in the plugin mapper).

## 3. Sources & fetch/cache contract (identical in both outputs)

Two live sources, both producing the §2 schema:

| Source | URL | Shape |
|---|---|---|
| LiteLLM | `https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json` | flat map; `input_cost_per_token`, `output_cost_per_token`, `cache_*_input_token_cost`, tiered entries |
| OpenRouter | `https://openrouter.ai/api/v1/models` | `data[]` with nested `pricing.{prompt,completion,input_cache_read,input_cache_write}` (per-token strings) |

- Per-token values are multiplied by `1e6` to per-million (`to_price_per_million`).
- **Cache TTL = 24h.** Background refresh on a timer; bounded request timeouts.
- **SSRF guard:** reject private/loopback/`.local` hosts before fetching.
- **Fail-safe:** a failed refresh keeps the last-good cache (never zeroes it); record a source-failure marker.

## 4. Model-id canonicalization (pinned by vectors — top drift risk)

Single algorithm, byte-for-byte identical across outputs:
1. Lowercase ASCII; trim whitespace.
2. Strip a known provider prefix list (`openai/`, `anthropic/`, `nvidia/`, `azure/`, …) into a `provider` field, keep the bare model id for lookup.
3. Apply the explicit alias/transform table (kept in the spec, not per-runtime) before lookup.
4. OpenRouter lookup id is canonicalized the same way (`canonicalizeOpenRouterLookupId`).

Every transform has a conformance vector (§7). No runtime may add an ad-hoc transform without a vector.

## 5. Resolution order (every lookup, both outputs)

```
host-reported costUsd (authoritative, if present)
  -> live cache (litellm | openrouter)
  -> offline catalog (pricing.json)
  -> $0   (labeled "unpriced")
```
Each result is tagged with its `source`. The plugin prefers `host` (OpenClaw's own engine);
`zoder-engine` has no host cost so it starts at the live cache.

## 6. Offline catalog + updater (the "updater part")

- `pricing.json` — the §2 schema serialized; the offline fallback for **both** outputs.
- **Updater** = a single generator (one codepath) that runs the §3+§4 normalization over the live
  sources (+ optional private enterprise/EIH overrides that LiteLLM/OpenRouter won't carry) and emits
  `pricing.json`. Scheduled (≥ daily). This keeps the fallback tier current so that when the live tier
  or host cost is unavailable, the estimate is still fresh.
- The updater is the *reference* normalization implementation; runtime live-fetch paths must match its
  output on the conformance vectors.

## 7. Conformance vectors — the anti-divergence mechanism

`conformance/` holds raw-input → expected-normalized-output fixtures:
- `litellm/*.json` — raw LiteLLM entries (flat + tiered + cache buckets) → expected `ModelPrice`.
- `openrouter/*.json` — raw OpenRouter `data[]` entries (nested pricing) → expected `ModelPrice`.
- `canon/*.json` — raw model id → canonical id (§4).
- `cost/*.json` — (ModelPrice, tokens_in/out, cache tokens) → expected cost.

**Both** the TS test suite and the Rust test suite load this same directory and assert equality.
Any divergence fails CI in the affected runtime. This is what makes "single spec" real rather than aspirational.

## 8. Parity test (Q3)

A cross-runtime parity check: for each model in the catalog, run identical (model, tokens) through the
TS path and the Rust path; assert costs equal within `epsilon = 1e-9`. In the plugin, when `host`
`costUsd` is present, cross-check it against the catalog estimate and log drift beyond a threshold
(this drift signal feeds the updater — answers Zach's "what if it drifts").

## 9. Rust daemon concurrency (Q4 — zoder-engine only)

- Cache held behind an atomic snapshot (`arc-swap` or `RwLock<Arc<Catalog>>`); **hot-path cost lookups
  read the snapshot lock-free** and never block on a refresh.
- One background refresh task (tokio interval), **jittered** to avoid thundering-herd on the sources.
- Refresh builds a new catalog off-path, then **atomically swaps** it in; a failed build leaves the
  current snapshot untouched.
- No `setTimeout`-style assumptions; the daemon outlives many refresh cycles, so leak-free timer
  handling and graceful shutdown (abort the task on signal) are required.

## 10. Build order

1. **This spec** (done) + `conformance/` vectors.
2. **`zoder-engine`** (Rust) — output #1, built against the vectors; vendored as a `zoder-integration` patch feeding zeroclaw's cost tracker.
3. **Updater** generator (reference normalization) emitting `pricing.json`.
4. **`tokenomics`** (TS) — already consumes OpenClaw's native engine; align its fallback-catalog loader + mapper to this schema and load the same vectors.
