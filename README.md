<div align="center">

# tandem-vpn ⟁

### Unblock YouTube, Discord & Telegram on Windows — one click, no server, no subscription.

A desktop GUI that drives the best open-source DPI-bypass engines (**Zapret / Flowseal**) for you.
No `.bat` files, no command line, no config archaeology. Download, click once, done.

[![Rust CI](https://github.com/etern1ty-crypto/tandem-vpn/actions/workflows/release.yml/badge.svg)](https://github.com/etern1ty-crypto/tandem-vpn/actions/workflows/release.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![OS: Windows 10/11](https://img.shields.io/badge/OS-Windows_10%2F11-0078D6?logo=windows)](https://microsoft.com)
[![Download](https://img.shields.io/github/v/release/etern1ty-crypto/tandem-vpn?label=Download&color=3ddc97)](https://github.com/etern1ty-crypto/tandem-vpn/releases/latest)
[![Русский](https://img.shields.io/badge/README-Русский-red)](README.ru.md)

![tandem-vpn — one-click DPI bypass](docs/media/hero.gif)

</div>

---

## Why this exists

Tools like Zapret work brilliantly — but they ship as a folder of `.bat` files you have to
download, unzip, pick the right strategy from, and manage from a console. Most people bounce
before it ever works.

**tandem-vpn is the missing GUI.** Everything `service.bat` does, in a window with buttons and
live status. No backend of ours sits between you and the internet — the bypass runs **entirely
on your machine.**

| | Manual Zapret (`.bat`) | **tandem-vpn** |
|---|---|---|
| Install | Unzip, hunt for the right folder | **One button** |
| Updates | Re-download, delete old, replace | **Auto check & fetch** |
| Interface | Windows console, `service.bat` menu | **Native GUI** |
| Hosts file | Hand-edit system files | **Idempotent merge, one click** |
| Diagnostics | Read console logs | **Live status for every service** |

---

## ⚡ Quick start (3 steps)

1. **[Download the latest release](https://github.com/etern1ty-crypto/tandem-vpn/releases/latest)**, then run it as Administrator.
2. Click **“Download & install Zapret”** — it fetches the current Flowseal build and unpacks it correctly.
3. Pick a strategy from the dropdown and hit **“Install to autostart.”**
   - *Optional:* click **“Update hosts file”** to fix Telegram Web and Discord voice.

**Done.** Blocked resources should now open normally.

![Main window](docs/media/screenshot-main.png)

---

## 🛡️ “My antivirus flagged it / Windows warned me” — read this

This is expected, and here’s the honest why:

- tandem-vpn loads the **WinDivert kernel driver** and edits your **hosts file** to steer packets.
  That’s the same low-level behavior real malware uses, so heuristic AV and **SmartScreen** flag
  *any* tool in this category (including Zapret itself).
- We ship **no telemetry and no backend.** The Zapret payload is downloaded straight from
  [Flowseal’s official releases](https://github.com/Flowseal/zapret-discord-youtube/releases) at runtime.
- **Verify it yourself:** every release lists SHA-256 checksums and a
  [VirusTotal](https://www.virustotal.com/) link. Full source is here (GPL-3.0); CI builds are public.

If SmartScreen appears: **More info → Run anyway.** To stop AV quarantining the driver, add the
install folder to exclusions.

---

## 🛠️ Features (Phase 1 — Zapret)

- **Full service control** — install any strategy to autostart, remove cleanly (service + WinDivert).
- **Flowseal integration** — fetch the latest builds and IPSet lists from inside the app.
- **Hosts management** — idempotent merge into `drivers\etc\hosts` for Discord voice & Telegram Web.
- **Diagnostics** — checks BFE, the `.sys` driver, `winws.exe`, and common conflicts.
- **Connectivity tests** — one click to verify YouTube / Discord / Telegram actually open.

## 🗺️ Roadmap

| Phase | Engine | What it does |
|---|---|---|
| **1 — Done** | **Zapret (Flowseal)** | Packet-level DPI bypass (WinDivert). Full `service.bat` feature set in a GUI. |
| 2 — In progress | **Cloudflare WARP** (`usque`) | Local SOCKS/HTTP proxy over Cloudflare’s edge. |
| 3 — Planned | **Goida (AvenCores)** | Pulls public configs from GitHub, dedupes, speed-tests, keeps the top N. |

> **No central server.** Zapret runs locally, WARP goes to Cloudflare, configs come straight from
> GitHub. Nothing of ours in the middle, no telemetry.

---

## ❓ FAQ

**Why Administrator rights?** WinDivert is a kernel driver and editing `hosts` needs system privileges.

**A game / anti-cheat broke.** Toggle Zapret off (“Remove services”) while playing, or enable **Game Filter**.

**Is this a VPN?** No traffic is routed through us. Phase 1 is a local DPI-bypass; it doesn’t hide your
IP. It’s about *reachability*, not anonymity.

---

## 💻 Building from source

Requirements: Rust (stable), Node 18+, and WebView2 for Windows builds.

```bash
cd app && npm install && npm run build     # frontend
cargo test -p tandem-core                  # core tests (cross-platform)
cd app && cargo tauri dev                  # run the app (needs: cargo install tauri-cli)
```

Windows specifics are isolated behind the `Sys` abstraction, so command planning is unit-tested on
any OS. Contributions welcome — see the [open issues](https://github.com/etern1ty-crypto/tandem-vpn/issues).

## 📄 License & credits

GPL-3.0-or-later. Built on the excellent work of
[Flowseal/zapret-discord-youtube](https://github.com/Flowseal/zapret-discord-youtube) and
[AvenCores/goida-vpn-configs](https://github.com/AvenCores/goida-vpn-configs).

> tandem-vpn is a tool for accessing lawful information and defeating network censorship.
> Use it in accordance with your local laws.
