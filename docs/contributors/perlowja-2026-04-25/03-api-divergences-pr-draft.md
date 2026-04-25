# Draft: Single consolidated zeroclaw PR

This file is the body for ONE upstream PR. It supersedes the prior split into "comment on PR #6099 + separate issue".

**File destination:** `~/zeroclaw-drafts/zeroclaw-divergences-pr-draft.md`
**Target repo + branch:** `zeroclaw-labs/zeroclaw:master`
**Source branch:** `perlowja:fix/api-divergences-c2-c6` (to be created in fork)
**Tested against:** zeroclaw `v0.7.3-46-g165cb335` (image `ghcr.io/perlowja/nclawzero-demo:master-165cb33`, commit `165cb33` on `master`, 46 commits past `v0.7.3`)
**Live walk evidence:** [docs/ZEROCLAW-API-LIVE-WALK.md](https://github.com/perlowja/meta-nclawzero/blob/main/docs/ZEROCLAW-API-LIVE-WALK.md) (39 of 57 catalogued endpoints walked)

---

# fix(api): five HTTP API divergences from documented behavior

**Tested against:** `v0.7.3-46-g165cb335` (commit `165cb335` on `master`).

A live walk of the HTTP API surface against `ghcr.io/perlowja/nclawzero-demo:master-165cb33` (built from `master` at `165cb33`, the demo container we ship for exhibitions) surfaced five small divergences from documented or expected behavior. Each is independently small and surgical; bundling them into one PR keeps the maintainer queue tidy. The full live walk is linked above; this PR addresses §C.2 through §C.6 (§C.1 is covered by sibling PR #6099).

Per-finding rationale + repro + proposed change below. Each commit in this PR is scoped to a single finding so a maintainer can drop or rework any individual fix without losing the rest.

## §C.2 — `/api/cron` allowlist matches whole command instead of head executable

**Repro** (against the demo image; pairing off):
```
curl -X POST http://127.0.0.1:42617/api/cron \
  -H 'Content-Type: application/json' \
  -d '{"name":"t","schedule":"@daily","command":"python3 -c print(1)"}'
```

**Observed:** HTTP 200 with body `{"error":"Failed to add cron job: blocked by security policy: Command not allowed by security policy: python3 -c print(1)"}`. The user's `autonomy.allowed_commands` includes `python3`, but the matcher treats the whole command string `python3 -c print(1)` as the lookup key and finds no exact match.

**Proposed fix:** Split the candidate command on whitespace and match the head token (`python3`) against `allowed_commands`, not the entire string. Existing security posture is preserved (deny-by-default, head-token must be explicitly allow-listed).

## §C.3 — Inconsistent error-code conventions across write endpoints

Four write endpoints currently use three different conventions for validation/policy/runtime failures:

| Endpoint | Status | Body | Convention |
|---|---|---|---|
| `POST /api/cron` (security policy reject) | 200 | `{"error": "..."}` | Envelope JSON, success status |
| `POST /api/memory` (storage write fail) | 500 | `{"error": "..."}` | JSON, server-error status |
| `POST /api/canvas/<id>` (content_type reject) | 400 | `{"error": "..."}` | JSON, client-error status |
| `POST /api/pair` (bad code) | 400 | `Invalid or expired pairing code` | Plaintext, client-error status |

**Proposed fix:** Normalize to the JSON-error + 4xx-status convention. Concretely:
- `POST /api/cron`: replace the success-status JSON wrapper with HTTP 422 `{"error":"...","code":"security_policy"}` for security rejections; HTTP 400 for shape errors.
- `POST /api/pair`: change the plaintext body to `{"error":"Invalid or expired pairing code"}` and keep 400.
- `POST /api/memory`: keep 500 for genuine storage failures (the read-only-DB case observed in the demo image is a real server-state failure), but distinguish from request-validation 4xx.

## §C.4 — `/api/sessions/<id>/messages` returns 200+empty for unknown id

**Repro:**
```
curl http://127.0.0.1:42617/api/sessions/does-not-exist/messages
```

**Observed:** HTTP 200 with body `{"messages":[],"session_id":"does-not-exist","session_persistence":false}`. Compare with `/api/sessions/<id>/state` for the same unknown id, which returns HTTP 404 `{"error":"Session persistence is disabled"}`. Asymmetric within the same resource family.

**Proposed fix:** Make `/api/sessions/<id>/messages` 404 on unknown ids when persistence is enabled (current behavior is "200 + empty" regardless of id existence). When persistence is disabled, both endpoints should return the same shape — either both 404 with a "persistence disabled" message, or both 200 with an empty result and a `session_persistence:false` flag. Pick one and apply uniformly.

## §C.5 — `/api/plugins` is shadowed by SPA fallback when feature is off

**Repro:** Default demo build (no `plugins-wasm` feature, no `web_dist_dir` configured):
```
curl -i http://127.0.0.1:42617/api/plugins
```

**Observed:** HTTP 503 with body `Web dashboard not available. Set gateway.web_dist_dir in your config and build the frontend with: cd web && npm ci && npm run build`. The static-files SPA fallback intercepts the unrouted `/api/plugins` path and returns the dashboard-not-built error, masking the fact that the API endpoint is feature-gated off rather than misconfigured.

**Proposed fix:** Register a feature-aware stub for `/api/plugins` that returns HTTP 404 `{"error":"plugins endpoint not enabled","feature":"plugins-wasm"}` when the feature is compiled out, so client feature-detection works without confusion. The same pattern applies to any other `/api/*` route that the SPA fallback currently shadows when its feature is off.

## §C.6 — `/metrics` returns 200+plaintext explanation when Prometheus backend is disabled

**Repro:**
```
curl -i http://127.0.0.1:42617/metrics
```

**Observed:** HTTP 200 with body `# Prometheus backend not enabled. Set [observability] backend = "prometheus" in config.`. A scraping Prometheus server treats this as a valid empty exposition (one comment line, no samples) — observability is silently broken.

**Proposed fix:** Return HTTP 404 (or 503 with the same body) when `[observability] backend` is not `prometheus`. Prometheus scrapers will mark the target as down and surface the misconfiguration to operators instead of silently scraping empty.

---

## Test approach

Each finding gets a unit test in the relevant crate (most live in `zeroclaw-gateway` and `zeroclaw-runtime`). Tests assert the expected status code + body shape for the failure path. The live walk doc is linked for any maintainer who wants the broader context.

## Cross-references

- **Sibling PR:** #6099 (`fix(config): preserve user-supplied providers.fallback through load/save`) covers §C.1 of the same live walk. This PR explicitly does not duplicate that work; the two PRs can land independently.
- **Live walk source:** `perlowja/meta-nclawzero` repo at `docs/ZEROCLAW-API-LIVE-WALK.md` — full record of the 39 endpoints probed, with sanitized response bodies and divergences flagged.

## Constraints honored

- Each commit author: `Jason Perlow <jperlow@gmail.com>`.
- No corporate-employer references in code or commit messages.
- All examples use neutral provider names (`primary`, `together`, `gemini-flash-latest`) — no Anthropic/Claude default suggestions.
- Each fix is independently revertable (one commit per finding).

## Checklist

- [x] Branch `fix/api-divergences-c2-c6` created (lives on ARGONAS bare `zeroclaw.git` for safekeeping; not yet on perlowja fork)
- [ ] **§C.2 cron allowlist matcher commit — RE-EVALUATE.** The original draft claimed `is_command_allowed` matches the whole command string. Reading current source (`crates/zeroclaw-config/src/policy.rs:1140-1163`), it already extracts the head token via `command_basename(executable)`. The actual rejection of `python3 -c print(1)` comes from `is_args_safe` at lines 1207-1219 which **intentionally blocks `-c` and `-m`** (arbitrary code exec / module loader vectors). The real divergence is the **error message**: it says "Command not allowed by security policy" for what is actually an argument-pattern rejection. Recommended fix: distinguish in the error string ("Argument pattern blocked: `python3 -c` runs arbitrary code; use a script file instead"). NO CODE CHANGE TO MATCHER NEEDED.
- [ ] §C.3 error response normalization commit. Current code at `crates/zeroclaw-gateway/src/api.rs:347-354` returns 500 INTERNAL_SERVER_ERROR for ALL cron add failures (including security policy rejections, which should be 422). Distinguish in the match arm: error string starting with "Command not allowed" or containing "security policy" → 422 + `code: "security_policy"`; storage failures → 500. Same-PR change to `/api/pair` plaintext → JSON.
- [ ] §C.4 sessions/messages 404 commit. Current handler at `crates/zeroclaw-gateway/src/api.rs:1333` (`/api/sessions/{id}/messages`) returns 200+empty for unknown ids when persistence is enabled. Compare with `/api/sessions/{id}/state` (line 1485) which returns 404. Make symmetric.
- [ ] §C.5 plugins feature-aware stub commit. The `plugins-wasm` feature gate is in `crates/zeroclaw-gateway/src/api_plugins.rs` (route registration is conditional). When the feature is off, the SPA fallback shadows the path. Add a feature-OFF stub returning 404+JSON `{"error":"plugins endpoint not enabled","feature":"plugins-wasm"}` registered unconditionally in `lib.rs`.
- [x] **§C.6 metrics 503-when-disabled commit** — DONE on local branch. Commit on `fix/api-divergences-c2-c6`: replaces 200+plaintext disabled-hint with 503 SERVICE_UNAVAILABLE so Prometheus scrapers mark the target down. Test `metrics_endpoint_returns_503_when_prometheus_is_disabled` updated. Branch pushed to ARGONAS bare; not yet on perlowja fork.
- [ ] Unit tests for each finding green — needs `cargo test` on a Rust workspace host (ULTRA arm64 per fleet build routing).
- [ ] `cargo fmt --all -- --check` clean (rustup match CI toolchain version)
- [ ] `cargo clippy --workspace -- -D warnings` clean (or scoped + documented)
- [ ] Squash-or-keep decision per maintainer convention (default: keep one commit per finding for revertability)
- [ ] PR body posted, sibling PR #6099 cross-referenced

## Implementation hand-off (2026-04-25 evening)

**State on `fix/api-divergences-c2-c6` branch:**
- 1 commit: §C.6 metrics 503 fix (gateway/lib.rs, with test).
- Branch lives on ARGONAS at `/mnt/datapool/git/zeroclaw.git` and on local `/tmp/zeroclaw`.
- NOT yet pushed to `github.com/perlowja/zeroclaw` (held for review).

**Remaining work for ULTRA-side execution** (Rust workspace builds best on arm64 macOS per fleet routing):
1. Clone the branch from ARGONAS or fast-forward from local
2. Implement C.2 (error-message clarification only — no matcher change), C.3 (error code distinction), C.4 (sessions/messages 404), C.5 (plugins stub) per pointers in checklist above
3. `cargo test --all-features` for each affected crate (`zeroclaw-config`, `zeroclaw-gateway`)
4. `cargo fmt --all -- --check` + `cargo clippy --workspace -- -D warnings`
5. Push to `github.com/perlowja/zeroclaw:fix/api-divergences-c2-c6` and open PR against `zeroclaw-labs/zeroclaw:master`
