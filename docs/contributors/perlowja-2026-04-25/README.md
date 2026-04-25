# perlowja contributions — 2026-04-25 bundle

This directory packages four deliverables from a TYDEUS / Jetson Orin Nano +
Windows 11 zeroclaw validation pass. Bundled here for handoff review by
@singlerider / zeroclaw-labs maintainers.

## Files

| File | What it is | Status |
|---|---|---|
| `01-windows-doc-review.md` | Validation review of the upstream Windows setup doc against `master` HEAD `4ab49989` (v0.7.3). Six findings, three confirmed live on TYPHON Windows 11 build 26200.8313, four new (setup.bat 32-bit overflow on >2 TB drives, unescaped parens in `if/else` echo, hardcoded `VERSION=0.6.2` in setup.bat, `--service-init` no-op on Windows). | Findings confirmed; ready for upstream |
| `02-windows-setup-rewritten.md` | Drop-in replacement for `docs/setup-guides/windows-setup.md`. Covers manual prebuilt (recommended), setup.bat (with known-issue callouts), source build, Scoop (currently stale), Docker (verified against `ghcr.io/zeroclaw-labs/zeroclaw:latest`). Removes the Windows Service / LocalSystem section that doesn't exist in code; fixes log path; clarifies `--service-init` no-op; adds host-side WSL2/Docker best practices with citations to Microsoft Learn and Docker docs. | Validated end-to-end on TYPHON Windows |
| `03-api-divergences-pr-draft.md` | Five HTTP API divergences caught during a separate live API walk against `ghcr.io/perlowja/nclawzero-demo:master-165cb33`. §C.1 is on PR #6099; §C.2-C.6 are queued as a single consolidated PR (commit for §C.6 already on local branch `fix/api-divergences-c2-c6`). | Draft — full PR ships when C.2-C.5 land on ULTRA |
| `zeroclaw-windows-doc.html` | Combined deliverable as a single self-contained HTML file (mdBook-rendered with CSS inlined, matches the upstream zeroclaw doc style). Contains all three docs in order. Open in any browser. | Ready to circulate |

## Test environment

- Host: TYPHON (`192.168.207.61`) — Threadripper PRO 5945WX, RTX 5060
- OS: Windows 11 build 26200.8313 — fresh install, dual-boot with Debian 13
- Tooling: Rust 1.95.0, Git for Windows, NoMachine 9.4.14, Docker 26.1.5 (in WSL Debian 13)
- Container image tested: `ghcr.io/zeroclaw-labs/zeroclaw:latest`
- All claims marked "verified" in the docs were exercised against this environment

## Bundle author

Jason Perlow <jperlow@gmail.com> · github @perlowja · gitlab @perlowja

## License

Same as upstream zeroclaw (Apache 2.0 / MIT dual).
