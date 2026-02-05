# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

vibe-usage is a macOS native menubar application written in Rust that displays Claude Code API usage statistics. It shows session (5-hour) and weekly (7-day) usage limits with color-coded tray icons (green/yellow/orange/red based on utilization percentage).

## Build Commands

```bash
cargo build --release    # Build the application
cargo run --release      # Run the application
cargo run --bin test_api --release  # Test API connectivity
cargo fmt                # Format code
cargo clippy --release   # Lint
```

## Architecture

**Threading Model:**
- Main thread runs the UI event loop (tao/tray-icon)
- Background thread runs Tokio async runtime for API calls
- Shared state via `Arc<Mutex<AppState>>`

**Module Structure:**
- `main.rs` - Entry point, initializes logger and MenubarApp
- `menubar.rs` - Core application logic, event loop, menu management
- `api.rs` - HTTP client for Anthropic OAuth usage API
- `keychain.rs` - Retrieves OAuth token from macOS keychain via `security` CLI
- `icons.rs` - Generates colored circular tray icons (22x22px)
- `ui.rs` - Progress bar rendering for menu display

**API Integration:**
- Endpoint: `https://api.anthropic.com/api/oauth/usage`
- Auth: Bearer token from keychain (searches for "Claude Code-credentials")
- Beta header: `anthropic-beta: oauth-2025-04-20`
- User-Agent: `claude-code/2.1.31`

**Data Flow:**
1. Background thread fetches usage data every 5 minutes (or on manual refresh)
2. API response parsed into `ParsedUsage` struct with session/weekly percentages
3. Main thread updates tray icon color and menu content
4. Reset times formatted as human-readable strings ("today at 3:45pm")

**macOS-specific:**
- Hides dock icon via `NSApplicationActivationPolicyAccessory`
- Uses `cocoa`/`objc` crates for Objective-C bindings
