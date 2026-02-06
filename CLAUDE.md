# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## Project Overview

**vibe-usage** is a macOS native menubar application written in Rust that displays Claude Code API usage statistics. Clicking the menubar icon opens a native NSMenu with graphical progress bars showing session (5-hour), weekly (7-day), and Sonnet-only usage limits, plus extra usage status. The tray icon itself shows a color-coded dot + percentage text + signal bars that update based on utilization.

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
├── icons.rs     # Generates tray icon (dot + text + bars) as raw RGBA pixels
└── bin/
    └── test_api.rs  # Standalone API connectivity test
```

### Threading Model

- **Main thread**: Runs the `tao` event loop, owns the NSStatusItem/NSMenu, polls for state changes every 500ms
- **Background thread**: Runs a tokio async runtime, fetches usage from the API every 5 minutes
- **Shared state**: `Arc<Mutex<AppState>>` passed between threads

### Data Flow

1. Background thread checks if refresh is needed (every 5 min or on first launch)
2. Fetches usage from `https://api.anthropic.com/api/oauth/usage`
3. Parses JSON response into `ParsedUsage` (session/weekly/sonnet percentages, reset times, extra usage)
4. Main thread detects state change, rebuilds the NSMenu with updated custom views
5. Tray icon is regenerated with color-coded dot and bar indicators via `generate_status_icon_raw()`
6. Reset times are formatted as human-readable strings ("today at 3pm", "tomorrow at 2am")

### Menu Display

The menu is a native NSMenu with custom NSView-based items (via `setView:` on NSMenuItem):

- **Section views**: Title label + right-aligned percentage, graphical progress bar (NSView with rounded corners, color-filled), and reset time in secondary text
- **Sections shown**: Session, Weekly, Sonnet only, Extra usage status
- **Separators** between each section
- **Quit** item with Cmd+Q shortcut (uses `terminate:` action)
- Menu width: 260pt, progress bars: 228pt wide, 6pt tall with 3pt corner radius

### Tray Icon

The tray icon is 200x44 pixels (rendered at 2x for retina, displayed at 100x22pt):

- **Left**: Color-coded dot (session usage level)
- **Center-left**: Session percentage text
- **Center-right**: Signal bars (4 bars representing weekly usage quartiles)
- **Right**: Weekly percentage text

The icon is created as raw RGBA pixels via the `image` crate, then converted to an NSImage using NSBitmapImageRep (half pixel dimensions for retina).

### API Integration

- **Endpoint**: `https://api.anthropic.com/api/oauth/usage`
- **Auth**: Bearer token extracted from macOS keychain (`Claude Code-credentials`)
- **Headers**: `anthropic-beta: oauth-2025-04-20`, `User-Agent: claude-code/2.1.31`
- **Response fields**: `five_hour`, `seven_day`, `seven_day_sonnet` (utilization windows), `extra_usage`

### Color Thresholds

Used for both the tray icon and menu progress bar fills:

| Usage %  | Color  |
|----------|--------|
| 0-49%    | Green  |
| 50-79%   | Yellow |
| 80-94%   | Orange |
| 95-100%  | Red    |

### macOS-Specific Details

- Hides dock icon via `NSApplicationActivationPolicyAccessory`
- Creates `NSStatusItem` directly via Cocoa APIs
- Uses `setMenu:` on the status item — macOS natively shows the menu on click (no click event detection needed)
- Custom menu item views use `NSTextField` labels + `NSView` layers for progress bars
- Keychain access via `security` CLI to avoid repeated system prompts

## Key Dependencies

- `tao` - Event loop (provides `ControlFlow::WaitUntil` for periodic polling)
- `cocoa` / `objc` - Direct Objective-C bindings for NSStatusItem, NSMenu, NSView, NSFont, NSColor
- `reqwest` - HTTP client with rustls for API calls
- `tokio` - Async runtime for background API fetching
- `image` / `imageproc` / `ab_glyph` - Tray icon pixel rendering and text drawing
- `chrono` - Reset time formatting (UTC to local, human-readable)
- `security-framework` - Keychain access (used by test_api binary only)

## Notes

- The app requires an existing Claude Code OAuth token in the macOS keychain
- Run `claude` CLI and sign in first to populate credentials
- `objc` crate macro warnings (`unexpected_cfgs`) during build are harmless — ignore them
- `cocoa` crate deprecation warnings are expected and fine
