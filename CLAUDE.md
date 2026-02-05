# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## Project Overview

**vibe-usage** is a macOS native menubar application written in Rust that displays Claude Code API usage statistics. It shows session (5-hour) and weekly (7-day) usage limits with color-coded tray icons based on utilization percentage.

## Build Commands

```bash
cargo build --release          # Build the application
cargo run --release --bin vibe-usage   # Run the application
cargo run --release --bin test_api     # Test API connectivity
cargo fmt                      # Format code
cargo clippy --release         # Lint (expect cocoa deprecation warnings)
```

## Architecture

### Module Structure

```
src/
├── main.rs      # Entry point, initializes logger and MenubarApp
├── menubar.rs   # Core app: event loop, tray icon, menu management
├── api.rs       # HTTP client for Anthropic OAuth usage API
├── keychain.rs  # Retrieves OAuth token from macOS keychain
├── icons.rs     # Generates colored circular tray icons (44x44 retina)
├── ui.rs        # Progress bar rendering for menu display
└── bin/
    └── test_api.rs  # Standalone API connectivity test
```

### Threading Model

- **Main thread**: Runs the tao event loop for UI (tray icon, menu)
- **Background thread**: Runs tokio async runtime for API calls every 5 minutes
- **Shared state**: `Arc<Mutex<AppState>>` for thread-safe usage data sharing

### Data Flow

1. Background thread checks if refresh needed (every 5 min or manual)
2. Fetches usage from `https://api.anthropic.com/api/oauth/usage`
3. Parses response into `ParsedUsage` with session/weekly percentages
4. Main thread polls state, updates tray icon color and menu content
5. Reset times formatted as human-readable ("tomorrow at 2:00am")

### API Integration

- **Endpoint**: `https://api.anthropic.com/api/oauth/usage`
- **Auth**: Bearer token from macOS keychain ("Claude Code-credentials")
- **Headers**: `anthropic-beta: oauth-2025-04-20`, `User-Agent: claude-code/2.1.31`
- **Response**: JSON with `five_hour` and `seven_day` utilization windows

### Color Thresholds

| Usage %  | Color  | Icon |
|----------|--------|------|
| 0-49%    | Green  | 🟢   |
| 50-79%   | Yellow | 🟡   |
| 80-94%   | Orange | 🟠   |
| 95-100%  | Red    | 🔴   |

### macOS-specific

- Hides dock icon via `NSApplicationActivationPolicyAccessory`
- Uses `cocoa` crate for Objective-C bindings (deprecated but functional)
- Keychain access via `security` CLI to avoid repeated prompts

## Key Dependencies

- `tao` - Cross-platform window/event loop (used for menubar apps)
- `tray-icon` - System tray icon management
- `reqwest` - HTTP client with rustls for API calls
- `tokio` - Async runtime for background fetching
- `image` - Icon generation with anti-aliasing
- `cocoa` - macOS AppKit bindings

## Notes

- The app requires an existing Claude Code OAuth token in keychain
- Run `claude` CLI and sign in first to populate credentials
- Tray icon is 44x44 pixels (retina 2x) with 10px radius circle
- Menu uses native macOS styling (no colored text support)
