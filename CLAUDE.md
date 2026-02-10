# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## Project Overview

**vibe-usage** is a macOS native menubar application written in Rust that displays Claude Code API usage statistics. Clicking the menubar icon opens a native NSMenu with graphical progress bars showing session (5-hour), weekly (7-day), and Sonnet-only usage limits, plus extra usage status. The menu also includes a 24-hour sparkline chart, status info (Claude version, model, plan), and a settings submenu for customizing the icon, colors, refresh interval, notifications, and more. The tray icon is dynamic — it supports 4 icon types (Dot, SignalBars, MiniBars, DotGrid) with configurable color palettes and thresholds.

## Build Commands

```bash
cargo build --release                  # Build the application
cargo run --release --bin vibe-usage   # Run the application
cargo run --release --bin test_api     # Test API connectivity
cargo fmt                              # Format code
cargo clippy --release                 # Lint (expect objc/cocoa deprecation warnings)
```

## Architecture

### Module Structure

```
src/
├── main.rs      # Entry point, initializes tracing logger and MenubarApp
├── menubar.rs   # Core app: NSStatusItem, NSMenu with custom views, event loop
├── api.rs       # HTTP client for Anthropic OAuth usage API, ParsedUsage model
├── keychain.rs  # Retrieves OAuth token from macOS keychain via `security` CLI
├── icons.rs     # Generates tray icon (4 icon types) as raw RGBA pixels
├── settings.rs  # Persistent user preferences (~/.vibe-usage/settings.json)
├── history.rs   # Usage history recording for sparkline chart (~/.vibe-usage/history.jsonl)
└── bin/
    └── test_api.rs  # Standalone API connectivity test
```

### Threading Model

- **Main thread**: Runs the `tao` event loop, owns the NSStatusItem/NSMenu, polls for state changes every 500ms
- **Background thread**: Runs a tokio async runtime, re-reads OAuth token from keychain on each fetch, fetches usage at a configurable interval (1/2/5/10 min, default 5 min), with exponential backoff on repeated failures
- **Shared state**: `Arc<Mutex<AppState>>` passed between threads

### Data Flow

1. Background thread checks if refresh is needed (configurable interval or manual Cmd+R)
2. Re-reads OAuth token from keychain, then fetches usage from `https://api.anthropic.com/api/oauth/usage`
3. Parses JSON response into `ParsedUsage` (session/weekly/sonnet percentages, reset times, extra usage)
4. On success: records history entry for sparkline, checks notification thresholds
5. On failure: increments failure counter, applies exponential backoff (2^n seconds, capped at interval)
6. Main thread detects state change, rebuilds the NSMenu with updated custom views
7. Tray icon is regenerated via `generate_status_icon_raw()` based on current settings
8. Reset times are formatted as human-readable strings ("today at 3pm", "tomorrow at 2am")

### Menu Structure

The menu is a native NSMenu with custom NSView-based items (via `setView:` on NSMenuItem):

1. **Usage Sections** (each toggleable in settings):
   - **Session**: Title label + right-aligned %, graphical progress bar, reset time
   - **Weekly**: Same layout
   - **Sonnet**: Same layout
   - **Extra Usage**: Text status line
   - Separators between each section

2. **Sparkline Chart** (24h history):
   - Interpolated session + weekly usage trend lines, threshold-colored
   - Legend with color-coded dashes
   - Y-axis labels (min/max %)
   - Rendered as 400×120px RGBA image, displayed at ~60pt height

3. **Status Info**:
   - Version (from `claude --version`)
   - Model (from `~/.claude/settings.json`, mapped to full name)
   - Plan (subscription type from keychain)

4. **Settings Submenu**:
   - Icon Type (Dot / SignalBars / MiniBars / DotGrid)
   - Show Usage Categories (toggles for Session, Weekly, Sonnet, Extra)
   - Show Icon / Show Label / Show Number toggles (at least one must be on)
   - Color Palette (Default / Monochrome)
   - Icons Colored toggle
   - Usage Thresholds presets (Default 50/75/90, Conservative 30/60/90)
   - Auto Refresh Interval (1 / 2 / 5 / 10 min)
   - Notifications (enable + per-section thresholds for session and weekly)
   - Launch at Login (creates/removes LaunchAgent plist)

5. **Actions**:
   - Refresh (Cmd+R)
   - Quit (Cmd+Q)

Menu width: 260pt, progress bars: 228pt wide, 6pt tall with 3pt corner radius.

### Tray Icon

The tray icon is 44pt tall (rendered at 88px for retina) with dynamic width based on visible sections:

**Icon Types:**

| Type | Width | Description |
|------|-------|-------------|
| Dot | 18px | Single colored circle (session usage level) |
| SignalBars | 30px | 4 ascending bars (weekly usage quartiles) |
| MiniBars | 14px | Vertical fill bar per section |
| DotGrid | 54px | Label + 4-dot row (25% increments each) |

**Layout:** Each visible section (session/weekly) contributes an icon graphic + optional label + optional percentage text, separated by gaps. The icon is composable — show/hide icon, label, and number independently.

**Color:** Uses configurable palette (Default = green/yellow/orange/red, Monochrome = grayscale). Graphics can be independently colored or follow palette. Template mode (system-tinted) used when monochrome + uncolored.

The icon is created as raw RGBA pixels via the `image` crate, then converted to an NSImage using NSBitmapImageRep (half pixel dimensions for retina). Font: Geneva.ttf (static TTF compatible with `ab_glyph`).

### Settings

Stored as JSON in `~/.vibe-usage/settings.json`. Key settings:

- `icon_type`: Dot, SignalBars, MiniBars, DotGrid (default: Dot)
- `show_icon`, `show_label`, `show_number`: Toggles (at least one required)
- `show_session`, `show_weekly`, `show_sonnet`, `show_extra`: Section visibility
- `color_palette`: Default or Monochrome
- `color_thresholds`: warning/high/critical percentages
- `icons_colored`: Whether icon graphics use color or follow palette
- `refresh_interval`: 60/120/300/600 seconds (default: 300)
- `notify_enabled`: Enable desktop notifications
- `notify_session_threshold`, `notify_weekly_threshold`: Percent thresholds (default: 80)
- `launch_at_login`: Auto-start via LaunchAgent

Includes migration logic for old schema fields (`menubar_style`, `show_percent`, etc.).

### History & Sparkline

Usage snapshots stored in `~/.vibe-usage/history.jsonl` (JSONL format, one entry per line). Each entry has a Unix timestamp, session %, and weekly %. Entries older than 48 hours are trimmed on load. Duplicate entries (unchanged by >0.1%) are skipped. The sparkline chart renders the most recent 24 hours of data.

### API Integration

- **Endpoint**: `https://api.anthropic.com/api/oauth/usage`
- **Auth**: Bearer token extracted from macOS keychain (`Claude Code-credentials`)
- **Headers**: `anthropic-beta: oauth-2025-04-20`, `User-Agent: claude-code/2.1.31`
- **Response fields**: `five_hour`, `seven_day`, `seven_day_sonnet` (utilization windows), `extra_usage`
- **Token refresh**: Re-reads from keychain on every fetch (handles token rotation)

### Color Thresholds

Configurable via presets. Default thresholds used for both the tray icon and menu progress bar fills:

| Usage %  | Color  |
|----------|--------|
| 0-49%    | Green  |
| 50-74%   | Yellow |
| 75-89%   | Orange |
| 90-100%  | Red    |

Presets: Default (50/75/90), Conservative (30/60/90).

### Notifications

Desktop notifications via NSUserNotification when usage exceeds configurable thresholds:
- Separate thresholds for session and weekly (default: 80%)
- Sends once per threshold crossing — resets when usage drops below, re-fires on next crossing
- Plays "default" system sound

### macOS-Specific Details

- Hides dock icon via `NSApplicationActivationPolicyAccessory`
- Creates `NSStatusItem` directly via Cocoa APIs
- Uses `setMenu:` on the status item — macOS natively shows the menu on click (no click event detection needed)
- Custom menu item views use `NSTextField` labels + `NSView` layers for progress bars
- Keychain access via `security` CLI to avoid repeated system prompts
- Launch at Login via `~/Library/LaunchAgents/com.vibe-usage.launcher.plist`
- ObjC menu handler class registered at runtime for settings actions

## Key Dependencies

- `tao` - Event loop (provides `ControlFlow::WaitUntil` for periodic polling)
- `cocoa` / `objc` - Direct Objective-C bindings for NSStatusItem, NSMenu, NSView, NSFont, NSColor
- `reqwest` - HTTP client with rustls for API calls
- `tokio` - Async runtime for background API fetching
- `image` / `imageproc` / `ab_glyph` - Tray icon pixel rendering and text drawing
- `chrono` - Reset time formatting (UTC to local, human-readable)
- `serde` / `serde_json` - Settings and history persistence
- `tracing` / `tracing-subscriber` - Structured logging
- `anyhow` - Error handling
- `security-framework` - Keychain access (used by test_api binary only)

## App Icon

The app icon files are in the project root:

- `vibe-usage-icon.svg` — Vector source (1024×1024)
- `vibe-usage-icon.png` — Rasterized PNG (1024×1024)

The icon features "vu" in bold monospace text with a smile curve underneath, on a Claude orange gradient background (#E8956F → #D06845). The shape is an Apple-style squircle (superellipse with n=5, 120-point path) matching the macOS app icon silhouette. The SVG is the source of truth — edit it and re-export the PNG as needed.

## Notes

- The app requires an existing Claude Code OAuth token in the macOS keychain
- Run `claude` CLI and sign in first to populate credentials
- Settings stored at `~/.vibe-usage/settings.json`, history at `~/.vibe-usage/history.jsonl`
- `objc` crate macro warnings (`unexpected_cfgs`) during build are harmless — ignore them
- `cocoa` crate deprecation warnings are expected and fine
