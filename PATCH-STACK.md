# zoder-integration patch stack

`zoder-integration` is a rebasing patch stack over **zeroclaw-labs/zeroclaw `master`**
(the `upstream` remote). This manifest is the source of truth for *what* the fork
carries and *why*, and it drives `scripts/fork-reconcile.sh`, which watches for
pieces upstream has adopted so we can drop our now-redundant originals.

Status legend: **fork-only** (not submitted upstream) · **open** (PR open upstream)
· **adopted** (merged upstream — drop our copy, take theirs on next reconcile).

| Patch | Owns (key paths) | Upstream PR | Status |
|---|---|---|---|
| cost-pricing-catalog | `crates/zeroclaw-runtime/src/agent/pricing_catalog.rs` | #8380 | **adopted** (2026-06-29) |
| cost-ledger-atomic | `crates/zeroclaw-config/src/cost/tracker.rs` | #8412 | **adopted** (2026-06-29) |
| cost-org-rpc | `crates/zeroclaw-runtime/src/rpc/dispatch.rs` (cost/org + cost/query) | #8482 | **adopted** (2026-06-29) |
| zerocode-cost-tab | `apps/zerocode/src/dashboard.rs`, `apps/zerocode/src/client.rs`, `apps/zerocode/locales/*/zerocode.ftl` | #8483 | **open** |
| mnemos-session-backend | `crates/zeroclaw-infra/src/session_backend.rs`, `crates/zeroclaw-channels/src/orchestrator/mod.rs` (session-context), daemon wiring, `Cargo.lock` (ureq) | — | **fork-only** |
| theme-pack | `web/src/contexts/themes.json` | — | **fork-only** |
| alias-cli | `src/alias_cli/mod.rs` | — | **fork-only** |
| gitlab-ci-fork-scoping | `.gitlab-ci.yml` | — | **fork-only** |
| pricing-engine-docs | `docs/pricing-engine-spec.md`, `scripts/agent-preflight.sh` | — | **fork-only** |

## Reconcile workflow

1. `scripts/fork-reconcile.sh` — fetch `upstream/master`, classify each tracked PR
   (adopted / open) and flag fork-owned files upstream has since touched (conflict
   risk). Run it after any zoder PR merges upstream and on a periodic timer.
2. For **adopted** rows: `git merge upstream/master` on an `integrate/*` branch,
   take **upstream's** version for the adopted files (the reviewed one), keep the
   fork-only/open patches.
3. Regenerate `Cargo.lock` on a build host (the fork adds `ureq` for
   `backend-mnemos`), get `cargo check`/`test` green, then fast-forward
   `zoder-integration` and push.

Update the **Status** column whenever a PR's state changes (the script reports the
drift; this table is what it diffs against).
