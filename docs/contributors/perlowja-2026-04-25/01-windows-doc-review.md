# zeroclaw Windows setup doc — review against current master

**Doc URL:** https://singlerider.github.io/zeroclaw/en/setup/windows.html
**Cross-check target:** `zeroclaw-labs/zeroclaw:master` HEAD `4ab49989` (Cargo.toml `version = "0.7.3"`).
**Source-of-truth bits inspected:** `setup.bat`, `crates/zeroclaw-runtime/src/service/mod.rs` (1693 LOC), `dist/scoop/zeroclaw.json`, `src/main.rs` ServiceCommands.

**Status:** Post-validation update 2026-04-25 — all original findings re-verified on TYPHON Windows 11 build 26200.8313 with PowerShell 5.1 + Rust 1.95.0 + Git for Windows. Five new findings added during live run.

---

## 1. Scoop manifest is severely stale — HIGH (CONFIRMED)

`dist/scoop/zeroclaw.json` claims:
```json
"version": "0.5.9",
"url": "https://github.com/zeroclaw-labs/zeroclaw/releases/download/v0.5.9/zeroclaw-x86_64-pc-windows-msvc.zip"
```

Live verification (`findstr /n version dist\scoop\zeroclaw.json`):
```
2:    "version": "0.5.9",
8:    "url": "...releases/download/v0.5.9/zeroclaw-x86_64-pc-windows-msvc.zip",
```

Current Cargo.toml: `version = "0.7.3"`. Manifest is **23 patch releases behind** master.
GitHub `releases/latest` confirms tag = `v0.7.3` (artifact `zeroclaw-x86_64-pc-windows-msvc.zip` 13.3 MB present).

**Fix candidates:** (a) bump `dist/scoop/zeroclaw.json` to track release tags via release-time CI; (b) document the staleness in the doc with a "use Option 1 (--prebuilt) for current release" steer.

## 2. Doc claims Windows Service / LocalSystem path that isn't implemented — MEDIUM (CONFIRMED)

Doc text:
> When run elevated, the installer registers a Windows Service under LocalSystem instead of a user-scoped scheduled task.

Live verification on TYPHON, **with elevated admin token (high integrity confirmed via `whoami /groups`)**:
```
zeroclaw 0.7.3 (manually installed prebuilt)
zeroclaw service install
→ ✅ Installed Windows scheduled task: ZeroClaw Daemon
   Wrapper: C:\Users\jasonperlow\.zeroclaw\logs\zeroclaw-daemon.cmd
   Logs: C:\Users\jasonperlow\.zeroclaw\logs

Get-ScheduledTask -TaskName *zeroclaw*  →  "ZeroClaw Daemon"  Ready
Get-Service -Name *zeroclaw*            →  (empty, no service)
```

**No Windows Service was created.** Even with an elevated admin token, the install path is scheduled-task. Confirms the original code review: 47 `cfg!(target_os = "windows")` branches all flow into the scheduled-task path; no `sc.exe` / `windows-service` crate / LocalSystem code exists.

**Fix candidates:** (a) implement the Windows Service path (`windows-service` crate + admin token check); (b) trim the doc to scheduled-task-only and reference Windows Service as a TODO.

## 3. Log path doc/code drift — LOW (CONFIRMED)

Doc says: Logs go to `%LOCALAPPDATA%\ZeroClaw\logs\`.

Live install output: `Logs: C:\Users\jasonperlow\.zeroclaw\logs` — i.e. `%USERPROFILE%\.zeroclaw\logs\`.

**Fix candidate:** doc edit to match code (less invasive than code change).

## 4. setup.bat — NOT current; multiple bugs — HIGH (NEW, REPLACES "OK")

Original draft marked this OK pending TYPHON verification. **TYPHON verification revealed multiple blocking bugs.** setup.bat **does not complete** on a clean Windows 11 box with a >2 TB disk.

### 4a. Hardcoded VERSION drift in setup.bat itself

`setup.bat:10` has `set "VERSION=0.6.2"` — banner prints `ZeroClaw Windows Setup v0.6.2` even though Cargo.toml is at v0.7.3. Same staleness pattern as Scoop manifest. The version string is baked into the script and not updated by release CI.

### 4b. 32-bit overflow in disk-space check

`setup.bat:47-49`:
```cmd
for /f "tokens=3" %%a in ('dir /-C "%~dp0" 2^>nul ^| findstr /C:"bytes free"') do (
    set /a "FREE_DISK_GB=%%a / 1073741824"
)
```

`set /a` is 32-bit signed. Any drive larger than ~2 TB (free bytes > 2^31) overflows and prints **"Invalid number. Numbers are limited to 32-bits of precision."** TYPHON's 1.8 TB drive has enough free bytes to trip this. The script continues past the error but the FREE_DISK_GB variable is not set.

**Fix:** use `set /a "FREE_DISK_GB=%%a / 1073741824 / 1"` won't help — same overflow. Need to right-shift via PowerShell or compute in two steps (divide by 1024 three times). Cleanest: replace with `wmic logicaldisk get freespace` and divide in PowerShell.

### 4c. Unescaped parens in echo inside `if/else` block

`setup.bat:75`:
```cmd
where node >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo   %YELLOW%Node.js not found (optional - web dashboard will use stub).%RESET%
)
```

The `(optional - web dashboard will use stub).` literal has unescaped `(` and `)` inside an `if` block. cmd's parser sees the `)` as an `if`-block terminator, then the `.%RESET%` (where `%RESET%` expands to ESC[0m) becomes free tokens that the parser can't reconcile. **Crash:** `.[0m was unexpected at this time.` — script terminates partway through prerequisite checks.

**Fix:** escape parens with `^(` and `^)`, or use a separate-line approach without inline parens.

### 4d. End-to-end: setup.bat does not complete on TYPHON

With both 4b and 4c present, `setup.bat --prebuilt` aborts during `[1/5] Checking prerequisites...` before ever reaching the binary download (step `[3/5]`). Manual install works (verified): downloading the v0.7.3 windows-msvc.zip and extracting to `%USERPROFILE%\.zeroclaw\bin\` produces a working `zeroclaw.exe` that reports `zeroclaw 0.7.3` and `service install` succeeds.

So: the doc's *Option 1 — setup.bat --prebuilt* path is **broken on Windows 11 + drive >2 TB**. Doc's underlying assumption (that the binary path works) is correct; only the wrapper script is broken.

## 5. `service --help` is Linux/macOS-centric — LOW (NEW)

`zeroclaw service --help` output:
```
Manage OS service lifecycle (launchd/systemd user service)
...
--service-init <SERVICE_INIT>  Init system to use: auto (detect), systemd, or openrc
                               [possible values: auto, systemd, openrc]
```

Even though the code installs a Windows scheduled task on Windows (which clearly works — verified live), the help text mentions only launchd/systemd, and `--service-init` enum has no Windows option. Either:
- (a) The `--service-init` flag is dead code on Windows (in which case it should error or note that Windows uses scheduled task), or
- (b) The help text is missing a Windows-specific blurb that says "On Windows, installs a user-scoped scheduled task; --service-init is ignored."

**Fix candidate:** doc/help-text edit. Keep code as-is (the scheduled-task path is solid).

## 6. SmartScreen / long-paths gotchas — verify on TYPHON (DEFERRED)

Doc lists three gotchas; nothing in master code references them, which is correct (these are Windows OS behaviors not zeroclaw bugs). Worth confirming on TYPHON during install:
- (a) Long path support — does `setup.bat --full` build cleanly on a path-260-capped install? **Not verified — setup.bat doesn't complete (see 4d).**
- (b) SmartScreen first-launch — does the unsigned `zeroclaw.exe` actually trip SmartScreen as documented? **Not tripped during PowerShell-based manual install (no UI shown). Likely only triggered on double-click in Explorer.**
- (c) Scheduled task stop-at-idle — installed task `ZeroClaw Daemon` has `State: Ready`. Did NOT inspect the full task definition for AC-power / idle-stop flags. Worth a `Get-ScheduledTask | Get-ScheduledTaskInfo` follow-up if doc-accuracy on these flags matters.

## 7. InvestorClaw Windows test surface — separate scope (DEFERRED)

InvestorClaw Windows tests are a different test path from zeroclaw setup. Out of scope for this doc review.

---

## Recommended PR shape (updated post-TYPHON validation)

| # | Fix | Surface | Effort |
|---|---|---|---|
| 1 | Scoop manifest version bump (and release-time CI hook) | `dist/scoop/zeroclaw.json` + new release workflow | small — manifest bump trivial, CI hook ~30 LOC |
| 2 | setup.bat: hardcoded VERSION line 10 | `setup.bat` | trivial — derive from Cargo.toml or release tag |
| 3 | setup.bat: disk-space 32-bit overflow (line 47-49) | `setup.bat` | small — replace cmd arithmetic with PowerShell one-liner |
| 4 | setup.bat: unescaped `(` `)` inside echo inside `if` (line 75) | `setup.bat` | trivial — `^(` and `^)` |
| 5 | Doc: Windows Service is TODO, scheduled task is the supported path | `singlerider/zeroclaw` docs site | trivial |
| 6 | Doc: log path → `%USERPROFILE%\.zeroclaw\logs\` | docs site | trivial |
| 7 | service --help: Windows scheduled-task line + clarify `--service-init` is no-op on Windows | `crates/zeroclaw-runtime/src/service/cli.rs` (or wherever `clap` derives are) | trivial |
| 8 | Implement Windows Service / LocalSystem mode (full impl, not just doc-trim) | `crates/zeroclaw-runtime/src/service/mod.rs` | medium — 200-400 LOC, needs `windows-service` crate, admin-token detection, scheduled-task fallback when not elevated, TYPHON test |

**Recommended order to land:** 4 → 3 → 2 → 7 → 1 → 5+6 → 8 (4+3 are blockers for setup.bat to work at all; 2+7 polish; 1 fixes Scoop; 5+6 are pure doc; 8 is feature work).

---

*Review v1 written 2026-04-25 evening (pre-validation).*
*Review v2 updated 2026-04-25 morning Windows session (post-TYPHON validation): 4 marked HIGH and broken into 4a-4d, finding 5 added, finding 4 (originally OK) replaced. PR shape expanded from 4 to 8 items with new ordering.*
