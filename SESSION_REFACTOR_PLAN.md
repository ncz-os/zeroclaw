# Session Backend Foundation Refactor - Implementation Plan

## Overview
This document tracks the implementation of the Session-backend FOUNDATION refactor as specified in the task requirements.

## F1 — One backend handle per process
**Status**: Not yet implemented

Requirements:
- Construct exactly ONE `Arc<dyn SessionBackend>` per process at bootstrap
- Thread that handle down by injection into channels, gateway, runtime, and tools
- Replace per-caller blocking adapters with ONE shared async facade

## F2 — Fallible-read contract
**Status**: Not yet implemented

Requirements:
- SQLite: Replace `Ok(Vec::new())` on failure, `filter_map(Result::ok)`, and `.ok()` swallow in read paths
- JSONL: Unreadable files, line-read errors, malformed rows, and `read_dir` failures must propagate as `Err`
- Callers: WS/RPC restore and channel hydration must surface errors explicitly

## F3 — Multiword search contract
**Status**: Already documented in trait

The trait already documents:
> The `search` method interprets multiword queries as **OR** combinations:
> a query `"foo bar"` returns sessions containing `foo` OR `bar` (not both).

## Validation Commands
```bash
cargo +1.96.1 fmt --all -- --check
cargo +1.96.1 clippy --workspace --exclude zeroclaw-desktop --all-targets --features ci-all -- -D warnings
cargo +1.96.1 check --locked --features ci-all --all-targets
cargo +1.96.1 check --no-default-features
cargo +1.96.1 test -p zeroclaw-infra
cargo +1.96.1 test -p zeroclaw-channels --features ci-all session
```
