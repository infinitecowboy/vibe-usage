<p align="center">
  <img src="vibe-usage-icon.png" alt="Vibe Usage" width="128" height="128">
</p>

<h1 align="center">Vibe Usage</h1>

<p align="center">
  A macOS menubar app that tracks your Claude Code API usage in real time.
</p>

---

Vibe Usage lives in your menubar and shows session, weekly, and Sonnet-only usage limits with graphical progress bars, a 24-hour sparkline chart, and desktop notifications when you're approaching your limits.

![macOS](https://img.shields.io/badge/macOS-11%2B-blue)
![Rust](https://img.shields.io/badge/Rust-2021-orange)

## Features

- **Live usage tracking** — Session (5-hour), weekly (7-day), and Sonnet-only limits with progress bars and reset countdowns
- **Menubar indicator** — Pill-style icon with color-coded usage dots and configurable text
- **24-hour sparkline** — Trend chart showing session and weekly usage over time
- **Desktop notifications** — Configurable alerts when usage crosses thresholds
- **Auto-refresh** — Polls every 1/2/5/10 minutes (configurable)
- **Dark mode native** — Built with native Cocoa APIs, adapts to system appearance
- **Auto display mode** — Automatically adapts the menubar pill based on display size: dots only on small screens (<16"), labels on medium (16–25"), full percentages on large (>25"). Includes a simulate submenu for testing each layout
- **Launch at Login** — Optional LaunchAgent for auto-start

## Prerequisites

- **macOS 11.0+** (Big Sur or later)
- **Rust toolchain** — Install via [rustup](https://rustup.rs/)
- **Claude Code CLI** — Must be signed in with an active session so an OAuth token is present in your keychain

## Setup

### 1. Clone the repo

```bash
git clone https://github.com/infinitecowboy/vibe-usage.git
cd vibe-usage
```

### 2. Sign in to Claude Code

Vibe Usage reads the OAuth token from your macOS keychain. You need to have the Claude Code CLI installed and signed in:

```bash
claude
```

Sign in when prompted. This populates the `Claude Code-credentials` keychain entry that Vibe Usage reads.

### 3. Build and run

```bash
# Build the .app bundle
./bundle.sh

# Launch
open "target/release/Vibe Usage.app"
```

The app will appear in your menubar — click the indicator to see your usage breakdown.

### Running without the .app bundle

If you prefer running the bare binary (icon won't show in Activity Monitor):

```bash
cargo build --release
./target/release/vibe-usage
```

## Configuration

Settings are stored at `~/.vibe-usage/settings.json` and can be changed from the **Settings** submenu in the app. Options include:

| Setting | Options | Default |
|---|---|---|
| Show Icon / Show Number | Toggle each independently | Both on |
| Visible Sections | Session, Weekly, Sonnet, Extra Usage | Session + Weekly |
| Pill Outline | Bordered outline style for the menubar pill | Off |
| Auto Display Mode | Auto-adapt pill layout to display size | Off (Manual) |
| Simulate Display | Force Compact / Medium / Large for testing | Off (Real Display) |
| Color Palette | Default (green→red) or Monochrome | Default |
| Usage Thresholds | Default (50/75/90%) or Conservative (30/60/90%) | Default |
| Auto Refresh Interval | 1, 2, 5, or 10 minutes | 5 min |
| Notifications | Per-section thresholds (50–95%) | 80% |
| Launch at Login | On/Off | Off |

## How it works

1. Reads your OAuth token from the macOS keychain on each refresh
2. Fetches usage data from `api.anthropic.com/api/oauth/usage`
3. Parses session/weekly/sonnet utilization windows and extra usage status
4. Renders a native NSMenu with custom views (progress bars, sparkline, status info)
5. Updates the menubar icon based on current usage level and your settings

## Project structure

```
src/
├── main.rs       # Entry point
├── menubar.rs    # NSStatusItem, NSMenu, event loop
├── api.rs        # HTTP client for Anthropic usage API
├── keychain.rs   # OAuth token from macOS keychain
├── icons.rs      # Tray icon rendering (RGBA pixels)
├── settings.rs   # Persistent preferences
└── history.rs    # Usage history for sparkline chart
```

## Troubleshooting

**App shows "Loading..." and never updates**
- Make sure you're signed in to Claude Code (`claude` CLI) and have an active session
- Check Console.app for `vibe-usage` log messages

**Token errors**
- Vibe Usage re-reads the token from keychain on every fetch, so token rotation is handled automatically
- If issues persist, sign out and back in to Claude Code

**Build warnings**
- `objc` crate `unexpected_cfgs` warnings and `cocoa` deprecation warnings are expected and harmless
