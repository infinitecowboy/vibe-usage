use crate::{
    api::{ParsedUsage, UsageClient},
    icons::{generate_status_icon_raw, RawIcon, UsageLevel},
    keychain,
};
use anyhow::Result;
use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};

struct SendId(id);
unsafe impl Send for SendId {}
unsafe impl Sync for SendId {}

static STATUS_ITEM: OnceLock<Mutex<SendId>> = OnceLock::new();
static STATUS_INFO: OnceLock<StatusInfo> = OnceLock::new();

struct StatusInfo {
    version: String,
    model_alias: String,
    model_full: String,
    plan: String,
    session_id: Option<String>,
}

struct AppState {
    usage: Option<ParsedUsage>,
    client: Option<UsageClient>,
    needs_refresh: bool,
    last_update: Option<Instant>,
}

pub struct MenubarApp {
    state: Arc<Mutex<AppState>>,
}

impl MenubarApp {
    pub fn new() -> Result<Self> {
        let client = keychain::get_oauth_token()
            .ok()
            .and_then(|t| UsageClient::new(t).ok());

        STATUS_INFO.get_or_init(|| gather_status_info());

        Ok(Self {
            state: Arc::new(Mutex::new(AppState {
                usage: None,
                client,
                needs_refresh: true,
                last_update: None,
            })),
        })
    }

    pub fn run(self) -> Result<()> {
        let state = self.state;
        let event_loop = EventLoopBuilder::new().build();

        unsafe {
            let app = NSApp();
            if app != nil {
                app.setActivationPolicy_(
                    NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
                );
            }

            // Create NSStatusItem
            let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
            let length: f64 = -1.0; // NSVariableStatusItemLength
            let status_item: id = msg_send![status_bar, statusItemWithLength: length];
            let () = msg_send![status_item, retain];

            let button: id = msg_send![status_item, button];

            // Set initial icon
            if let Ok(raw) = generate_status_icon_raw(UsageLevel::Green, None) {
                set_button_image(button, &raw);
            }

            // Build initial menu
            let menu = build_menu(None);
            let () = msg_send![status_item, setMenu: menu];

            STATUS_ITEM.get_or_init(|| Mutex::new(SendId(status_item)));
            tracing::info!("Status item created with menu");
        }

        // Background fetch thread
        let bg_state = state.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("Failed to create tokio runtime: {}", e);
                    return;
                }
            };

            loop {
                let should_fetch = {
                    let s = bg_state.lock().unwrap();
                    s.needs_refresh
                        || s.last_update
                            .map(|t| t.elapsed() > Duration::from_secs(300))
                            .unwrap_or(true)
                };

                if should_fetch {
                    let client = bg_state.lock().unwrap().client.clone();
                    if let Some(c) = client {
                        match rt.block_on(c.fetch_usage()) {
                            Ok(r) => {
                                let usage = ParsedUsage::from(r);
                                let mut s = bg_state.lock().unwrap();
                                s.usage = Some(usage);
                                s.needs_refresh = false;
                                s.last_update = Some(Instant::now());
                            }
                            Err(e) => {
                                tracing::warn!("Failed to fetch usage: {}", e);
                            }
                        }
                    }
                }

                std::thread::sleep(Duration::from_secs(1));
            }
        });

        let mut last_rendered: Option<ParsedUsage> = None;

        event_loop.run(move |event, _, cf| {
            *cf = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));

            if let Event::NewEvents(_) = event {
                let current_usage = {
                    let s = state.lock().unwrap();
                    s.usage.clone()
                };

                if current_usage != last_rendered {
                    if let Some(si) = STATUS_ITEM.get() {
                        let si = si.lock().unwrap();
                        unsafe {
                            if let Some(ref usage) = current_usage {
                                // Update icon
                                let level = UsageLevel::from_percent(usage.max_percent);
                                if let Ok(raw) = generate_status_icon_raw(level, Some(usage)) {
                                    let button: id = msg_send![si.0, button];
                                    set_button_image(button, &raw);
                                }
                            }

                            // Rebuild menu with current data
                            let menu = build_menu(current_usage.as_ref());
                            let () = msg_send![si.0, setMenu: menu];
                        }
                    }
                    last_rendered = current_usage;
                }
            }
        });
    }
}

// ── Menu construction ────────────────────────────────────────────────

const MENU_WIDTH: f64 = 260.0;
const BAR_WIDTH: f64 = MENU_WIDTH - 32.0; // 16px padding each side
const BAR_HEIGHT: f64 = 6.0;
const CORNER_RADIUS: f64 = 3.0;

/// Build an NSMenu displaying usage data with custom views.
unsafe fn build_menu(usage: Option<&ParsedUsage>) -> id {
    let menu: id = msg_send![class!(NSMenu), new];
    let () = msg_send![menu, setMinimumWidth: MENU_WIDTH];

    if let Some(u) = usage {
        // Session
        add_section(
            menu,
            "Session",
            u.session_percent,
            u.session_reset.as_deref(),
        );

        let () = msg_send![menu, addItem: separator()];

        // Weekly (all models)
        add_section(menu, "Weekly", u.weekly_percent, u.weekly_reset.as_deref());

        let () = msg_send![menu, addItem: separator()];

        // Sonnet only
        add_section(
            menu,
            "Sonnet only",
            u.sonnet_percent.unwrap_or(0.0),
            u.sonnet_reset.as_deref(),
        );

        let () = msg_send![menu, addItem: separator()];

        // Extra usage
        let extra = if u.extra_usage_enabled {
            u.extra_usage_percent
                .map(|p| format!("Extra usage: {:.0}% consumed", p))
                .unwrap_or_else(|| "Extra usage enabled".to_string())
        } else {
            "Extra usage not enabled".to_string()
        };
        let () = msg_send![menu, addItem: menu_item_with_view(extra_usage_view(&extra))];
    } else {
        let () =
            msg_send![menu, addItem: text_item("Loading\u{2026}", 12.0, false, secondary_color())];
    }

    // Status info
    if let Some(info) = STATUS_INFO.get() {
        let () = msg_send![menu, addItem: separator()];
        let () = msg_send![menu, addItem: menu_item_with_view(status_section_view(info))];
    }

    let () = msg_send![menu, addItem: separator()];
    let () = msg_send![menu, addItem: quit_item()];

    menu
}

/// Add a usage section with custom view: title + percentage, progress bar, reset time
unsafe fn add_section(menu: id, name: &str, pct: f32, reset: Option<&str>) {
    let view = section_view(name, pct, reset);
    let () = msg_send![menu, addItem: menu_item_with_view(view)];
}

/// Create a custom NSView for a usage section
unsafe fn section_view(name: &str, pct: f32, reset: Option<&str>) -> id {
    let total_h: f64 = if reset.is_some() { 52.0 } else { 40.0 };

    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, total_h),
    )];

    let mut y = total_h;

    // Title row: "Session" left, "29%" right
    y -= 18.0;
    let title = make_label(name, 13.0, true, primary_color());
    set_frame(title, 16.0, y, MENU_WIDTH - 80.0, 16.0);
    let () = msg_send![view, addSubview: title];

    let pct_label = make_label(&format!("{:.0}%", pct), 13.0, false, primary_color());
    set_frame(pct_label, MENU_WIDTH - 56.0, y, 40.0, 16.0);
    let () = msg_send![pct_label, setAlignment: 2u64]; // NSTextAlignmentRight
    let () = msg_send![view, addSubview: pct_label];

    // Progress bar
    y -= BAR_HEIGHT + 6.0;
    let bar = make_progress_bar(pct, BAR_WIDTH, BAR_HEIGHT);
    set_frame(bar, 16.0, y, BAR_WIDTH, BAR_HEIGHT);
    let () = msg_send![view, addSubview: bar];

    // Reset time
    if let Some(r) = reset {
        y -= 14.0;
        let reset_label = make_label(
            &format!("Resets {}", format_reset(r)),
            10.0,
            false,
            secondary_color(),
        );
        set_frame(reset_label, 16.0, y, BAR_WIDTH, 12.0);
        let () = msg_send![view, addSubview: reset_label];
    }

    view
}

/// Create a view for the extra usage line
unsafe fn extra_usage_view(text: &str) -> id {
    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, 24.0),
    )];

    let label = make_label(text, 11.0, false, secondary_color());
    set_frame(label, 16.0, 4.0, MENU_WIDTH - 32.0, 16.0);
    let () = msg_send![view, addSubview: label];

    view
}

/// Create a compact status info view with key-value rows.
unsafe fn status_section_view(info: &StatusInfo) -> id {
    let row_h: f64 = 16.0;
    let mut rows: Vec<(&str, String)> = vec![
        ("Version", info.version.clone()),
        (
            "Model",
            format!("{} ({})", info.model_alias, info.model_full),
        ),
        ("Plan", info.plan.clone()),
    ];
    // Session ID available via info.session_id if needed later
    let total_h = rows.len() as f64 * row_h + 8.0; // 4pt top/bottom padding

    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, total_h),
    )];

    for (i, (key, val)) in rows.iter().enumerate() {
        let y = total_h - 4.0 - (i as f64 + 1.0) * row_h;

        let key_label = make_label(key, 11.0, true, secondary_color());
        set_frame(key_label, 16.0, y, 60.0, row_h);
        let () = msg_send![view, addSubview: key_label];

        let val_label = make_label(val, 11.0, false, primary_color());
        set_frame(val_label, 80.0, y, MENU_WIDTH - 96.0, row_h);
        let () = msg_send![view, addSubview: val_label];
    }

    view
}

// ── Status info gathering ────────────────────────────────────────────

fn gather_status_info() -> StatusInfo {
    let version = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().replace(" (Claude Code)", ""))
        .unwrap_or_else(|| "unknown".into());

    let (model_alias, model_full) = read_model_setting();

    let plan = keychain::get_account_info()
        .ok()
        .and_then(|info| info.subscription_type)
        .map(|s| format_plan_name(&s))
        .unwrap_or_else(|| "Unknown".into());

    let session_id = read_latest_session_id();

    StatusInfo {
        version,
        model_alias,
        model_full,
        plan,
        session_id,
    }
}

fn read_latest_session_id() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = format!("{}/.claude/history.jsonl", home);
    let content = std::fs::read_to_string(&path).ok()?;
    let last_line = content.lines().rev().find(|l| !l.trim().is_empty())?;
    let v: serde_json::Value = serde_json::from_str(last_line).ok()?;
    v["sessionId"].as_str().map(|s| s.to_string())
}

fn read_model_setting() -> (String, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = format!("{}/.claude/settings.json", home);
    let alias = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["model"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "sonnet".into());

    let full = map_model_alias(&alias);
    (alias.clone(), full)
}

fn map_model_alias(alias: &str) -> String {
    match alias {
        "opus" => "claude-opus-4-6".into(),
        "sonnet" => "claude-sonnet-4-5".into(),
        "haiku" => "claude-haiku-4-5".into(),
        other => other.to_string(),
    }
}

fn format_plan_name(sub_type: &str) -> String {
    match sub_type {
        "max" => "Claude Max".into(),
        "max_5x" => "Claude Max (5x)".into(),
        "pro" => "Claude Pro".into(),
        "free" => "Free".into(),
        "team" => "Team".into(),
        "enterprise" => "Enterprise".into(),
        other => other.to_string(),
    }
}

// ── NSView helpers ───────────────────────────────────────────────────

unsafe fn make_label(text: &str, size: f64, medium: bool, color: id) -> id {
    let label: id = msg_send![class!(NSTextField), alloc];
    let label: id = msg_send![label, init];
    let s = NSString::alloc(nil).init_str(text);
    let () = msg_send![label, setStringValue: s];

    let font: id = if medium {
        msg_send![class!(NSFont), systemFontOfSize: size weight: 0.23f64]
    } else {
        msg_send![class!(NSFont), systemFontOfSize: size]
    };
    let () = msg_send![label, setFont: font];
    let () = msg_send![label, setTextColor: color];
    let () = msg_send![label, setBezeled: NO];
    let () = msg_send![label, setDrawsBackground: NO];
    let () = msg_send![label, setEditable: NO];
    let () = msg_send![label, setSelectable: NO];
    label
}

unsafe fn set_frame(view: id, x: f64, y: f64, w: f64, h: f64) {
    let () = msg_send![view, setFrame: NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))];
}

unsafe fn make_progress_bar(pct: f32, w: f64, h: f64) -> id {
    let container: id = msg_send![class!(NSView), alloc];
    let container: id = msg_send![container, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(w, h),
    )];
    let () = msg_send![container, setWantsLayer: YES];

    // Track (background)
    let layer: id = msg_send![container, layer];
    let track_color = NSColor_rgba(1.0, 1.0, 1.0, 0.08);
    let cg_track: id = msg_send![track_color, CGColor];
    let () = msg_send![layer, setBackgroundColor: cg_track];
    let () = msg_send![layer, setCornerRadius: CORNER_RADIUS];

    // Fill
    let fill_w = (pct as f64 / 100.0).min(1.0) * w;
    if fill_w > 0.5 {
        let fill: id = msg_send![class!(NSView), alloc];
        let fill: id = msg_send![fill, initWithFrame: NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(fill_w, h),
        )];
        let () = msg_send![fill, setWantsLayer: YES];
        let fl: id = msg_send![fill, layer];

        let fill_color = bar_fill_color(pct);
        let cg_fill: id = msg_send![fill_color, CGColor];
        let () = msg_send![fl, setBackgroundColor: cg_fill];
        let () = msg_send![fl, setCornerRadius: CORNER_RADIUS];

        let () = msg_send![container, addSubview: fill];
    }

    container
}

// ── Colors ───────────────────────────────────────────────────────────

unsafe fn primary_color() -> id {
    NSColor_rgba(0.92, 0.92, 0.94, 1.0)
}

unsafe fn secondary_color() -> id {
    NSColor_rgba(0.55, 0.55, 0.58, 1.0)
}

unsafe fn bar_fill_color(pct: f32) -> id {
    if pct >= 95.0 {
        NSColor_rgba(1.0, 0.23, 0.19, 1.0) // red
    } else if pct >= 80.0 {
        NSColor_rgba(1.0, 0.58, 0.0, 1.0) // orange
    } else if pct >= 50.0 {
        NSColor_rgba(1.0, 0.80, 0.0, 1.0) // yellow
    } else {
        NSColor_rgba(0.20, 0.78, 0.35, 1.0) // green
    }
}

#[allow(non_snake_case)]
unsafe fn NSColor_rgba(r: f64, g: f64, b: f64, a: f64) -> id {
    use cocoa::appkit::NSColor;
    NSColor::colorWithRed_green_blue_alpha_(nil, r, g, b, a)
}

// ── Menu item helpers ────────────────────────────────────────────────

unsafe fn menu_item_with_view(view: id) -> id {
    let item: id = msg_send![class!(NSMenuItem), alloc];
    let item: id = msg_send![item,
        initWithTitle: NSString::alloc(nil).init_str("")
        action: cocoa::base::nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![item, setView: view];
    item
}

unsafe fn text_item(text: &str, size: f64, medium: bool, color: id) -> id {
    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, 24.0),
    )];
    let label = make_label(text, size, medium, color);
    set_frame(label, 16.0, 4.0, MENU_WIDTH - 32.0, 16.0);
    let () = msg_send![view, addSubview: label];
    menu_item_with_view(view)
}

unsafe fn separator() -> id {
    msg_send![class!(NSMenuItem), separatorItem]
}

unsafe fn quit_item() -> id {
    let s = NSString::alloc(nil).init_str("Quit");
    let item: id = msg_send![class!(NSMenuItem), alloc];
    let item: id = msg_send![item,
        initWithTitle: s
        action: sel!(terminate:)
        keyEquivalent: NSString::alloc(nil).init_str("q")
    ];
    item
}

/// Create NSImage from raw RGBA pixel data and set on button.
unsafe fn set_button_image(button: id, raw: &RawIcon) {
    let _pool = NSAutoreleasePool::new(nil);

    let rep: id = msg_send![class!(NSBitmapImageRep), alloc];
    let planes_ptr = raw.rgba.as_ptr() as *mut u8;
    let mut planes = [planes_ptr];

    let rep: id = msg_send![rep,
        initWithBitmapDataPlanes: planes.as_mut_ptr()
        pixelsWide: raw.width as i64
        pixelsHigh: raw.height as i64
        bitsPerSample: 8i64
        samplesPerPixel: 4i64
        hasAlpha: true
        isPlanar: false
        colorSpaceName: NSString::alloc(nil).init_str("NSDeviceRGBColorSpace")
        bytesPerRow: (raw.width * 4) as i64
        bitsPerPixel: 32i64
    ];

    if rep == nil {
        return;
    }

    let img: id = msg_send![class!(NSImage), alloc];
    let logical = NSSize::new(raw.width as f64 / 2.0, raw.height as f64 / 2.0);
    let img: id = msg_send![img, initWithSize: logical];
    let () = msg_send![img, addRepresentation: rep];
    let () = msg_send![img, setTemplate: false];
    let () = msg_send![button, setImage: img];
}

fn format_reset(iso: &str) -> String {
    use chrono::{DateTime, Datelike, Local, Timelike, Utc};

    let Ok(dt) = iso.parse::<DateTime<Utc>>() else {
        return iso.to_string();
    };
    let local: DateTime<Local> = dt.into();
    let now = Local::now();

    let h12 = if local.hour() % 12 == 0 {
        12
    } else {
        local.hour() % 12
    };
    let ampm = if local.hour() >= 12 { "pm" } else { "am" };
    let time = if local.minute() == 0 {
        format!("{}{}", h12, ampm)
    } else {
        format!("{}:{:02}{}", h12, local.minute(), ampm)
    };

    if local.date_naive() == now.date_naive() {
        format!("today at {}", time)
    } else if local.date_naive() == (now + chrono::Duration::days(1)).date_naive() {
        format!("tomorrow at {}", time)
    } else {
        format!("{} {} at {}", local.format("%b"), local.day(), time)
    }
}
