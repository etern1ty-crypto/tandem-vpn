# Release notes template

Paste this into every GitHub Release. The checksum + VirusTotal block is what neutralizes the
"is this a virus?" comments before they start — never ship a release without it.

---

## tandem-vpn vX.Y.Z

**One-click DPI bypass for Windows.** Unblock YouTube, Discord & Telegram — no server, no telemetry.

### 📥 Install
1. Download `tandem-vpn_X.Y.Z_x64-setup.exe` below and run it **as Administrator**.
2. Click **“Download & install Zapret”**, pick a strategy, **“Install to autostart.”**
3. See the [README](https://github.com/etern1ty-crypto/tandem-vpn#readme) for the full guide.

> **SmartScreen / antivirus warning?** Expected for any tool that loads the WinDivert kernel driver.
> More info → Run anyway. Verify the build with the checksums and VirusTotal link below.

### ✅ Verify this build
| File | SHA-256 | VirusTotal |
|------|---------|------------|
| `tandem-vpn_X.Y.Z_x64-setup.exe` | `<paste sha256>` | [scan](https://www.virustotal.com/gui/file/<hash>) |

<details>
<summary>How to compute the checksum yourself</summary>

```powershell
Get-FileHash .\tandem-vpn_X.Y.Z_x64-setup.exe -Algorithm SHA256
```
</details>

### 🔧 What's new
- …
- …

### 🐛 Fixes
- …

### 📦 Bundled engine
- Zapret / Flowseal: fetched at runtime from the official [Flowseal releases](https://github.com/Flowseal/zapret-discord-youtube/releases).

**Full changelog:** https://github.com/etern1ty-crypto/tandem-vpn/compare/vPREV...vX.Y.Z
