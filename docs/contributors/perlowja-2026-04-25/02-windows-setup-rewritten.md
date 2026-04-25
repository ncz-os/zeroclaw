# Windows

Install, update, run as a Windows scheduled task, and uninstall on Windows 10 / 11.

If you’re running WSL2, you can follow the Linux setup instead — `install.sh` runs unchanged under WSL.

> **Note on `setup.bat`.** The release `setup.bat` wrapper has known bugs that prevent it from completing on Windows 11 with drives larger than ~2 TB (32-bit overflow in the disk-space pre-flight check) and on shells that strictly parse `if/else` blocks (unescaped parens in an echo). Until those land in a release, the **manual prebuilt** path (Option 1 below) is the recommended install. Building from source (Option 3) also works.

## Install

### Option 1 — Prebuilt binary (recommended)

Download the latest Windows release zip, extract `zeroclaw.exe`, and put it on your `PATH`.

From a PowerShell prompt:

```powershell
$ver = '0.7.3'   # current; check https://github.com/zeroclaw-labs/zeroclaw/releases/latest
$url = "https://github.com/zeroclaw-labs/zeroclaw/releases/download/v$ver/zeroclaw-x86_64-pc-windows-msvc.zip"
$dst = "$env:USERPROFILE\.zeroclaw\bin"
New-Item -ItemType Directory -Force -Path $dst | Out-Null
Invoke-WebRequest -Uri $url -OutFile "$env:TEMP\zeroclaw.zip" -UseBasicParsing
Expand-Archive -Force -Path "$env:TEMP\zeroclaw.zip" -DestinationPath $dst
[Environment]::SetEnvironmentVariable('Path', "$dst;$([Environment]::GetEnvironmentVariable('Path','User'))", 'User')
& "$dst\zeroclaw.exe" onboard
```

The zip ships a self-contained binary — no Rust toolchain, no Visual Studio Build Tools needed.

After install, verify:

```powershell
zeroclaw --version    # should report 0.7.3 (or whatever you installed)
```

### Option 2 — `setup.bat` (from a release)

```cmd
setup.bat --prebuilt
```

Flags:

| Flag | Behaviour |
|---|---|
| `--prebuilt` | Download prebuilt binary from GitHub Releases (fastest — no Rust toolchain needed) |
| `--minimal`  | Build core only (no channels, no hardware) |
| `--standard` | Build with common channels (Telegram, Discord, Slack, Matrix) |
| `--full`     | Build everything |

> ⚠️ **Known issues.** As of v0.7.3, `setup.bat` may abort during prerequisite checks on Windows 11 with drives larger than ~2 TB or with shells that strictly parse parens inside `if/else` blocks. If you see `Invalid number. Numbers are limited to 32-bits of precision.` or `.[0m was unexpected at this time.`, fall back to **Option 1** above.

### Option 3 — From source

Requires Rust (`rustup`) and Visual Studio Build Tools:

```cmd
git clone https://github.com/zeroclaw-labs/zeroclaw
cd zeroclaw
cargo install --locked --path .
zeroclaw onboard
```

### Option 4 — Scoop (currently stale)

> ⚠️ **The Scoop manifest in the repo is pinned to v0.5.9** (23 patch releases behind master). Until a release-time CI hook bumps it, prefer Option 1 or 3. If you do use Scoop and hit issues, please open a PR against `dist/scoop/zeroclaw.json`.

```
scoop install zeroclaw     # currently installs an older release; see warning above
zeroclaw onboard
```

### Option 5 — Docker

ZeroClaw publishes a Linux container image at **`ghcr.io/zeroclaw-labs/zeroclaw:latest`** (and `:vX.Y.Z` for tagged releases). On Windows, run it via Docker Desktop or via `sudo apt install docker.io` inside a WSL distro — both work, the container behaviour is identical.

Quick start:

```powershell
# Persistent volume for config + workspace; ZeroClaw's data dir inside the container is /zeroclaw-data
docker run -d --name zeroclaw `
  --restart=unless-stopped `
  -p 42617:42617 `
  -v zeroclaw-data:/zeroclaw-data `
  ghcr.io/zeroclaw-labs/zeroclaw:latest

# Watch the first-run logs — a one-time pairing code is printed
docker logs -f zeroclaw

# Health check (no auth)
curl http://localhost:42617/health

# Pair a client (use the code from `docker logs`)
curl -X POST http://localhost:42617/pair -H 'X-Pairing-Code: <code-from-logs>'
```

**Image facts (verified against `ghcr.io/zeroclaw-labs/zeroclaw:latest`):**

- **Base:** `gcr.io/distroless/cc-debian13:nonroot` (release stage; the `dev` stage is `debian:trixie-slim`)
- **`ENTRYPOINT ["zeroclaw"]`**, `CMD ["daemon"]` — running with no args starts the daemon and gateway
- **`EXPOSE 42617`** — both the daemon and gateway listen on this port
- **Data dir:** `/zeroclaw-data` (config: `/zeroclaw-data/.zeroclaw/config.toml`, workspace: `/zeroclaw-data/workspace`). Mount a named volume or bind here for persistence — note this is **not** `/root/.zeroclaw`.
- **Pairing:** the daemon prints a one-time pairing code to stdout on first start; clients POST to `/pair` with that code in the `X-Pairing-Code` header before any authenticated endpoint will respond.
- **Web dashboard:** disabled by default in the published image. To enable, build the frontend (`cd web && npm ci && npm run build`) and set `gateway.web_dist_dir` in config or `ZEROCLAW_WEB_DIST_DIR` env.

Build from source against the bundled Dockerfile:

```powershell
git clone https://github.com/zeroclaw-labs/zeroclaw
cd zeroclaw
docker build -t zeroclaw:local -f Dockerfile.debian .
```

**Verified on Windows + Docker:**

- **Container behaviour matches Linux.** Pulled and ran `ghcr.io/zeroclaw-labs/zeroclaw:latest` in WSL Debian on Windows 11 build 26200.8313. Image starts cleanly, gateway listens on `:42617`, `/health` returns valid JSON, pairing-code flow works.
- **Docker without Docker Desktop.** `wsl --install` to enable WSL2, then `sudo apt install docker.io` inside the WSL distro, gives you the daemon directly — verified pulls and runs the published image without modification.

**Host-side best practices** — general Docker + WSL2 guidance, not zeroclaw-specific runtime claims. Sourced from Microsoft Learn and Docker’s own docs where applicable:

- **Volume mounts.** Bind-mounting Windows-side paths (`-v C:/Users/...:/zeroclaw-data`) into a Linux container crosses the WSL2 ⇄ Windows filesystem boundary; Microsoft documents the layout and the cross-OS path implications in the [WSL file systems](https://learn.microsoft.com/en-us/windows/wsl/filesystems) reference. Prefer Docker named volumes (`-v zeroclaw-data:/zeroclaw-data`) or store the workspace inside the WSL filesystem (`\\wsl$\Debian\home\...`) for near-native performance.
- **Networking.** Default WSL2 networking is NAT’d — services in the container are reachable from Windows via `localhost:<port>` after `-p` forwarding (verified on TYPHON). If you need to reach the container from another box on the LAN, or run multi-container setups where intra-container DNS matters, switch to mirrored mode per Microsoft’s [Mirrored mode networking](https://learn.microsoft.com/en-us/windows/wsl/networking#mirrored-mode-networking) reference, by adding to `%USERPROFILE%\.wslconfig`:
  ```
  [wsl2]
  networkingMode=mirrored
  ```
- **Daemon under Docker, not Task Scheduler.** Inside the container there is no Windows Task Scheduler. Use Docker’s [restart policy](https://docs.docker.com/engine/containers/start-containers-automatically/) — `--restart=unless-stopped` as in the example above — for daemon-mode startup. The published image runs as PID 1 / nonroot user; the container *is* the service — don’t run `zeroclaw service install` inside it.
- **Skill sandbox via host Docker socket.** ZeroClaw’s skill-execution sandbox can shell out to Docker. If you’re running ZeroClaw itself in a container and want skill sandboxing to also use Docker, mount the host Docker socket so child containers run on the host daemon rather than nesting Docker-in-Docker:
  ```
  -v //var/run/docker.sock:/var/run/docker.sock
  ```
  Be aware that mounting the Docker socket grants root-equivalent host access to anything inside the container — Docker’s [Protect the Docker daemon socket](https://docs.docker.com/engine/security/protect-access/) page covers the trade-off. On Docker Desktop for Windows, the host socket is `\\.\pipe\docker_engine`; the bind-mount syntax above translates correctly. The pattern is general; it has not been benchmarked specifically against zeroclaw’s skill sandbox in this doc.
- **Resource limits.** Docker Desktop on Windows allocates RAM/CPU via `%USERPROFILE%\.wslconfig`, which defaults to half host RAM. The full configuration surface is documented in Microsoft’s [Advanced settings configuration in WSL](https://learn.microsoft.com/en-us/windows/wsl/wsl-config) reference. A reasonable starting envelope for a single-user ZeroClaw deployment is:
  ```
  [wsl2]
  memory=8GB
  processors=4
  ```
  Bump up if you’re running heavy skill workloads or local LLM inference inside the same WSL distro; this is sizing guidance, not a hard requirement of the image.

## System dependencies

Windows builds use the MSVC toolchain. To build from source you need:

- Visual Studio Build Tools (or full Visual Studio) with the “Desktop development with C++” workload
- Rust stable (via `rustup`)

If you’re using **Option 1** or **Option 2 `--prebuilt`** you don’t need the Rust toolchain — the binary is self-contained.

## Running as a service

On Windows, ZeroClaw installs as a **user-scoped scheduled task** named `ZeroClaw Daemon`. There is no Windows Service / LocalSystem option in the current release — the underlying code path always installs a scheduled task, regardless of whether `zeroclaw service install` is run from an elevated or non-elevated shell.

```cmd
zeroclaw service install
zeroclaw service start
```

This creates a task in Task Scheduler (`taskschd.msc`) under your user account that starts on login. Manage it via:

```cmd
zeroclaw service status
zeroclaw service restart
zeroclaw service stop
zeroclaw service logs
```

> **About `--service-init`.** The CLI exposes a `--service-init [auto|systemd|openrc]` flag for cross-platform consistency, but on Windows it is a no-op — the scheduled-task path is always used.

Logs go to `%USERPROFILE%\.zeroclaw\logs\` (specifically, `<config_dir>/logs/` where `<config_dir>` defaults to `%USERPROFILE%\.zeroclaw\`). The wrapper script that the scheduled task runs is at `%USERPROFILE%\.zeroclaw\logs\zeroclaw-daemon.cmd`.

> **Server / multi-user installs.** Native Windows Service / LocalSystem support is on the roadmap but not yet implemented. For now, on a server box, install ZeroClaw under the account that the agent should run as; the scheduled-task path will start it on that user’s login. If you need it to start before any user logs in, use **Task Scheduler → ZeroClaw Daemon → Properties → General → “Run whether user is logged on or not.”**

## Update

### Manual (Option 1 path)

Re-run the PowerShell install block from **Option 1** with the new `$ver`. The new zip overwrites the existing `zeroclaw.exe` in place. Then:

```powershell
zeroclaw service restart
```

### `setup.bat`

Re-download the latest release and re-run `setup.bat --prebuilt` (or whichever flag you used originally). Then:

```cmd
zeroclaw service restart
```

### Scoop

```
scoop update zeroclaw       # subject to the staleness caveat above
zeroclaw service restart
```

### From source

```cmd
cd C:\path\to\zeroclaw
git pull
cargo install --locked --path . --force
zeroclaw service restart
```

## Uninstall

Stop and remove the scheduled task:

```cmd
zeroclaw service stop
zeroclaw service uninstall
```

Remove the binary:

```cmd
:: Option 1 (manual prebuilt) or setup.bat
rmdir /s /q "%USERPROFILE%\.zeroclaw\bin"

:: Option 3 (cargo install)
del "%USERPROFILE%\.cargo\bin\zeroclaw.exe"

:: Option 4 (Scoop)
scoop uninstall zeroclaw
```

Remove config, workspace, and logs (optional — this deletes conversation history):

```cmd
rmdir /s /q "%USERPROFILE%\.zeroclaw"
```

> The previous version of this doc referenced `%LOCALAPPDATA%\ZeroClaw\` — that path is **not** used by the current release; only `%USERPROFILE%\.zeroclaw\` is.

## Gotchas

- **Long paths.** Some Windows file systems still cap path lengths at 260 characters. Enable long path support if you hit `path too long` errors during a source build:
  ```
  reg add HKLM\SYSTEM\CurrentControlSet\Control\FileSystem /v LongPathsEnabled /t REG_DWORD /d 1 /f
  ```

- **SmartScreen.** The unsigned binary may trip SmartScreen on first launch from Explorer (double-click). Right-click → Properties → “Unblock” is the standard workaround until we add a signed MSI. Launching from PowerShell or `cmd.exe` typically does not trigger SmartScreen.

- **Task Scheduler stop-at-idle / battery.** By default Windows may terminate scheduled tasks on idle or battery. The installed `ZeroClaw Daemon` task disables these conditions, but if you’ve installed via an older release you can verify under **Task Scheduler → ZeroClaw Daemon → Properties → Conditions**:
  - “Start the task only if the computer is on AC power” — unchecked
  - “Stop if the computer switches to battery power” — unchecked
  - “Start the task only if the computer is idle for…” — unchecked

- **`set /a` on >2 TB drives** *(setup.bat only)*. The disk-space pre-flight check in `setup.bat` uses 32-bit cmd arithmetic and overflows on free-space readings above ~2 GB. Use Option 1 (manual prebuilt) until this is fixed in a release.

- **OpenSSH password auth.** If you’re driving Windows over SSH and pubkey isn’t accepted, drop your key into `C:\Users\<user>\.ssh\authorized_keys` (regular user) or `C:\ProgramData\ssh\administrators_authorized_keys` (when logged in as a member of `Administrators`).

## Next

- Service management
- Quick start
- Operations → Overview
