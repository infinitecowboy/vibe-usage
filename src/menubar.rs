use crate::{
    api::{ParsedUsage, UsageClient},
    history,
    icons::{generate_indicator, RawIcon, SectionInfo, SectionVisibility, UsageLevel},
    keychain, settings,
};
use anyhow::Result;
use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};

struct SendId(id);
unsafe impl Send for SendId {}
unsafe impl Sync for SendId {}

static STATUS_ITEM: OnceLock<Mutex<SendId>> = OnceLock::new();
static STATUS_INFO: OnceLock<StatusInfo> = OnceLock::new();
static REFRESH_REQUESTED: AtomicBool = AtomicBool::new(false);
static SETTINGS_CHANGED: AtomicBool = AtomicBool::new(false);

/// Register an ObjC class to handle menu actions (refresh, settings, copy).
unsafe fn register_menu_handler() -> id {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};

    let superclass = Class::get("NSObject").unwrap();
    let mut decl = ClassDecl::new("MenuHandler", superclass).unwrap();

    extern "C" fn refresh_action(_this: &Object, _cmd: Sel, _sender: id) {
        REFRESH_REQUESTED.store(true, Ordering::Relaxed);
    }

    // Toggle show_icon (at least one of icon/number must remain on)
    extern "C" fn toggle_icon_action(_this: &Object, _cmd: Sel, _sender: id) {
        settings::update(|s| {
            let new_val = !s.show_icon;
            if new_val || s.show_number {
                s.show_icon = new_val;
            }
        });
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    // Toggle show_number (at least one of icon/number must remain on)
    extern "C" fn toggle_number_action(_this: &Object, _cmd: Sel, _sender: id) {
        settings::update(|s| {
            let new_val = !s.show_number;
            if new_val || s.show_icon {
                s.show_number = new_val;
            }
        });
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    // Toggle section visibility: tag encodes which section (0=session, 1=weekly, 2=sonnet, 3=extra)
    extern "C" fn toggle_section_action(_this: &Object, _cmd: Sel, sender: id) {
        let tag: isize = unsafe { msg_send![sender, tag] };
        settings::update(|s| match tag {
            0 => s.show_session = !s.show_session,
            1 => s.show_weekly = !s.show_weekly,
            2 => s.show_sonnet = !s.show_sonnet,
            3 => s.show_extra = !s.show_extra,
            _ => {}
        });
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn toggle_notify_action(_this: &Object, _cmd: Sel, _sender: id) {
        settings::update(|s| s.notify_enabled = !s.notify_enabled);
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn set_session_threshold_action(_this: &Object, _cmd: Sel, sender: id) {
        let tag: isize = unsafe { msg_send![sender, tag] };
        settings::update(|s| s.notify_session_threshold = tag as u32);
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn set_weekly_threshold_action(_this: &Object, _cmd: Sel, sender: id) {
        let tag: isize = unsafe { msg_send![sender, tag] };
        settings::update(|s| s.notify_weekly_threshold = tag as u32);
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn set_refresh_interval_action(_this: &Object, _cmd: Sel, sender: id) {
        let tag: isize = unsafe { msg_send![sender, tag] };
        settings::update(|s| s.refresh_interval = settings::RefreshInterval(tag as u64));
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn toggle_launch_at_login_action(_this: &Object, _cmd: Sel, _sender: id) {
        let new_val = {
            let cfg = settings::get();
            !cfg.launch_at_login
        };
        settings::update(|s| s.launch_at_login = new_val);
        set_launch_at_login(new_val);
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn toggle_icons_colored_action(_this: &Object, _cmd: Sel, _sender: id) {
        settings::update(|s| s.icons_colored = !s.icons_colored);
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn toggle_neutral_text_action(_this: &Object, _cmd: Sel, _sender: id) {
        settings::update(|s| s.neutral_text = !s.neutral_text);
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn toggle_show_in_dock_action(_this: &Object, _cmd: Sel, _sender: id) {
        settings::update(|s| s.show_in_dock = !s.show_in_dock);
        let show = settings::get().show_in_dock;
        unsafe {
            let app = NSApp();
            if show {
                app.setActivationPolicy_(
                    NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular,
                );
                // Refresh dock tile to show custom icon
                let dock_tile: id = msg_send![app, dockTile];
                let () = msg_send![dock_tile, display];
            } else {
                app.setActivationPolicy_(
                    NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
                );
            }
        }
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn toggle_monochrome_action(_this: &Object, _cmd: Sel, _sender: id) {
        settings::update(|s| {
            s.color_palette = match s.color_palette {
                settings::ColorPalette::Monochrome => settings::ColorPalette::Default,
                _ => settings::ColorPalette::Monochrome,
            };
        });
        SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    }

    extern "C" fn set_thresholds_action(_this: &Object, _cmd: Sel, sender: id) {
        let tag: isize = unsafe { msg_send![sender, tag] };
        if let Some((_, preset)) = settings::ColorThresholds::PRESETS.get(tag as usize) {
            settings::update(|s| s.color_thresholds = *preset);
            SETTINGS_CHANGED.store(true, Ordering::Relaxed);
        }
    }

    decl.add_method(
        sel!(refreshAction:),
        refresh_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleIconAction:),
        toggle_icon_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleNumberAction:),
        toggle_number_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleSectionAction:),
        toggle_section_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleNotifyAction:),
        toggle_notify_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(setSessionThresholdAction:),
        set_session_threshold_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(setWeeklyThresholdAction:),
        set_weekly_threshold_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(setRefreshIntervalAction:),
        set_refresh_interval_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleLaunchAtLoginAction:),
        toggle_launch_at_login_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleIconsColoredAction:),
        toggle_icons_colored_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleNeutralTextAction:),
        toggle_neutral_text_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleShowInDockAction:),
        toggle_show_in_dock_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleMonochromeAction:),
        toggle_monochrome_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(setThresholdsAction:),
        set_thresholds_action as extern "C" fn(&Object, Sel, id),
    );

    decl.register();

    let cls = Class::get("MenuHandler").unwrap();
    let obj: id = msg_send![cls, new];
    let () = msg_send![obj, retain];
    obj
}

static MENU_HANDLER: OnceLock<SendId> = OnceLock::new();

struct StatusInfo {
    version: String,
    model_alias: String,
    model_full: String,
    plan: String,
    #[allow(dead_code)]
    session_id: Option<String>,
}

struct AppState {
    usage: Option<ParsedUsage>,
    client: Option<UsageClient>,
    needs_refresh: bool,
    last_update: Option<Instant>,
    consecutive_failures: u32,
}

pub struct MenubarApp {
    state: Arc<Mutex<AppState>>,
}

impl MenubarApp {
    pub fn new() -> Result<Self> {
        settings::init();
        history::init();

        let client = UsageClient::new().ok();

        STATUS_INFO.get_or_init(|| gather_status_info());

        Ok(Self {
            state: Arc::new(Mutex::new(AppState {
                usage: None,
                client,
                needs_refresh: true,
                last_update: None,
                consecutive_failures: 0,
            })),
        })
    }

    pub fn run(self) -> Result<()> {
        let state = self.state;
        let event_loop = EventLoopBuilder::new().build();

        unsafe {
            let app = NSApp();
            if app != nil {
                let cfg_startup = settings::get();
                if cfg_startup.show_in_dock {
                    app.setActivationPolicy_(
                        NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular,
                    );
                } else {
                    app.setActivationPolicy_(
                        NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
                    );
                }

                // Set app icon from embedded PNG
                let icon_bytes: &[u8] = include_bytes!("../vibe-usage-icon.png");
                let ns_data: id = msg_send![class!(NSData),
                    dataWithBytes: icon_bytes.as_ptr()
                    length: icon_bytes.len()
                ];
                let icon_image: id = msg_send![class!(NSImage), alloc];
                let icon_image: id = msg_send![icon_image, initWithData: ns_data];
                if icon_image != nil {
                    let () = msg_send![app, setApplicationIconImage: icon_image];
                }
            }

            // Create NSStatusItem
            let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
            let length: f64 = -1.0; // NSVariableStatusItemLength
            let status_item: id = msg_send![status_bar, statusItemWithLength: length];
            let () = msg_send![status_item, retain];

            let button: id = msg_send![status_item, button];

            // Set initial icon
            let cfg = settings::get();
            let vis = SectionVisibility {
                session: cfg.show_session,
                weekly: cfg.show_weekly,
            };
            {
                let result = generate_indicator(
                    UsageLevel::Low,
                    None,
                    &cfg.color_thresholds,
                    vis,
                );
                update_status_button(button, &result.sections, &cfg);
            }

            // Build initial menu
            let menu = build_menu(None);
            let () = msg_send![status_item, setMenu: menu];

            STATUS_ITEM.get_or_init(|| Mutex::new(SendId(status_item)));
            MENU_HANDLER.get_or_init(|| SendId(register_menu_handler()));
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
                let (interval_secs, should_fetch, failures) = {
                    let s = bg_state.lock().unwrap();
                    let interval = settings::get().refresh_interval.0;
                    let fetch = s.needs_refresh
                        || s.last_update
                            .map(|t| t.elapsed() > Duration::from_secs(interval))
                            .unwrap_or(true);
                    (interval, fetch, s.consecutive_failures)
                };

                // Backoff: on repeated failures, wait longer before retrying
                // (2^failures seconds, capped at the configured interval)
                let backoff = if failures > 0 {
                    Duration::from_secs((2u64.saturating_pow(failures)).min(interval_secs))
                } else {
                    Duration::ZERO
                };

                if should_fetch {
                    let client = bg_state.lock().unwrap().client.clone();
                    if let Some(c) = client {
                        // Re-read token from keychain each time (tokens rotate)
                        let token = keychain::get_oauth_token();
                        match token {
                            Ok(tok) => match rt.block_on(c.fetch_usage(&tok)) {
                                Ok(r) => {
                                    let usage = ParsedUsage::from(r);
                                    check_and_notify(&usage);
                                    history::record(&usage);
                                    let mut s = bg_state.lock().unwrap();
                                    s.usage = Some(usage);
                                    s.needs_refresh = false;
                                    s.last_update = Some(Instant::now());
                                    s.consecutive_failures = 0;
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to fetch usage: {}", e);
                                    let mut s = bg_state.lock().unwrap();
                                    s.needs_refresh = false;
                                    s.last_update = Some(Instant::now());
                                    s.consecutive_failures =
                                        s.consecutive_failures.saturating_add(1);
                                }
                            },
                            Err(e) => {
                                tracing::warn!("Failed to read token from keychain: {}", e);
                                let mut s = bg_state.lock().unwrap();
                                s.needs_refresh = false;
                                s.last_update = Some(Instant::now());
                                s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                            }
                        }
                    }
                }

                std::thread::sleep(Duration::from_secs(1).max(backoff));
            }
        });

        let mut last_rendered: Option<ParsedUsage> = None;

        event_loop.run(move |event, _, cf| {
            *cf = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));

            if let Event::NewEvents(_) = event {
                // Check if refresh was requested from menu
                if REFRESH_REQUESTED.swap(false, Ordering::Relaxed) {
                    let mut s = state.lock().unwrap();
                    s.needs_refresh = true;
                    s.consecutive_failures = 0; // reset backoff on manual refresh
                }

                // Force re-render on settings change
                let settings_dirty = SETTINGS_CHANGED.swap(false, Ordering::Relaxed);

                let current_usage = {
                    let s = state.lock().unwrap();
                    s.usage.clone()
                };

                if current_usage != last_rendered || settings_dirty {
                    if let Some(si) = STATUS_ITEM.get() {
                        let si = si.lock().unwrap();
                        unsafe {
                            if let Some(ref usage) = current_usage {
                                // Update icon based on style
                                let cfg = settings::get();
                                let level = UsageLevel::from_percent(
                                    usage.max_percent,
                                    &cfg.color_thresholds,
                                );
                                let vis = SectionVisibility {
                                    session: cfg.show_session,
                                    weekly: cfg.show_weekly,
                                };
                                {
                                    let result = generate_indicator(
                                        level,
                                        Some(usage),
                                        &cfg.color_thresholds,
                                        vis,
                                    );
                                    let button: id = msg_send![si.0, button];
                                    update_status_button(
                                        button,
                                        &result.sections,
                                        &cfg,
                                    );
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
    let cfg = settings::get();
    let mut has_section = false;

    if let Some(u) = usage {
        // Session
        if cfg.show_session {
            add_section(
                menu,
                "Session",
                u.session_percent,
                u.session_reset.as_deref(),
            );
            has_section = true;
        }

        // Weekly (all models)
        if cfg.show_weekly {
            if has_section {
                let () = msg_send![menu, addItem: separator()];
            }
            add_section(menu, "Weekly", u.weekly_percent, u.weekly_reset.as_deref());
            has_section = true;
        }

        // Sonnet only
        if cfg.show_sonnet {
            if has_section {
                let () = msg_send![menu, addItem: separator()];
            }
            add_section(
                menu,
                "Sonnet only",
                u.sonnet_percent.unwrap_or(0.0),
                u.sonnet_reset.as_deref(),
            );
            has_section = true;
        }

        // Extra usage
        if cfg.show_extra {
            if has_section {
                let () = msg_send![menu, addItem: separator()];
            }
            let extra = if u.extra_usage_enabled {
                u.extra_usage_percent
                    .map(|p| format!("Extra usage: {:.0}% consumed", p))
                    .unwrap_or_else(|| "Extra usage enabled".to_string())
            } else {
                "Extra usage not enabled".to_string()
            };
            let () = msg_send![menu, addItem: menu_item_with_view(extra_usage_view(&extra))];
            has_section = true;
        }
    } else {
        let () =
            msg_send![menu, addItem: text_item("Loading\u{2026}", 12.0, false, secondary_color())];
        has_section = true;
    }

    // Sparkline
    if let Some(spark_view) = sparkline_section_view() {
        if has_section {
            let () = msg_send![menu, addItem: separator()];
        }
        let () = msg_send![menu, addItem: menu_item_with_view(spark_view)];
        has_section = true;
    }

    // Status info
    if let Some(info) = STATUS_INFO.get() {
        if has_section {
            let () = msg_send![menu, addItem: separator()];
        }
        let () = msg_send![menu, addItem: menu_item_with_view(status_section_view(info))];
    }

    let () = msg_send![menu, addItem: separator()];

    // Settings submenu
    let () = msg_send![menu, addItem: settings_submenu_item(&cfg)];

    // Refresh / Quit
    let () = msg_send![menu, addItem: refresh_item()];
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
    let rows: Vec<(&str, String)> = vec![
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

// ── Sparkline ────────────────────────────────────────────────────────

const SPARKLINE_W: u32 = 400; // 200pt @ 2x retina
const SPARKLINE_H: u32 = 120; // 60pt @ 2x retina
const SPARKLINE_PT_H: f64 = 60.0;

/// Render a chart from history data and wrap in an NSView menu item.
unsafe fn sparkline_section_view() -> Option<id> {
    let data = history::get_history();
    if data.len() < 2 {
        return None;
    }

    let cfg = settings::get();
    let cutoff = chrono::Utc::now().timestamp() - 24 * 3600;

    let filtered: Vec<&history::HistoryEntry> = data.iter().filter(|e| e.ts >= cutoff).collect();
    if filtered.len() < 2 {
        return None;
    }

    let session_points: Vec<(i64, f32)> = filtered.iter().map(|e| (e.ts, e.session)).collect();
    let weekly_points: Vec<(i64, f32)> = filtered.iter().map(|e| (e.ts, e.weekly)).collect();

    let show_s = cfg.show_session;
    let show_w = cfg.show_weekly;

    let (raw, y_min_val, y_max_val) = render_sparkline(
        &session_points,
        &weekly_points,
        show_s,
        show_w,
        cfg.color_palette,
        &cfg.color_thresholds,
    )?;
    let title = "Usage (24h)";

    // title (14pt) + legend (12pt) + sparkline (60pt) + padding = total
    let total_h: f64 = 100.0;
    let chart_x: f64 = 40.0; // left margin for y-axis labels
    let chart_w: f64 = MENU_WIDTH - chart_x - 16.0; // right padding

    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, total_h),
    )];

    // Title label
    let label = make_label(title, 10.0, true, secondary_color());
    set_frame(label, 16.0, total_h - 16.0, BAR_WIDTH, 14.0);
    let () = msg_send![view, addSubview: label];

    // Legend: conditionally show "— Session" and/or "— Weekly" with threshold colors
    let legend_y = total_h - 28.0;
    let mut legend_x = 16.0f64;

    if show_s {
        let s_pct = filtered.last().map(|e| e.session).unwrap_or(0.0);
        let s_rgb = threshold_rgb(s_pct, cfg.color_palette, &cfg.color_thresholds);
        let s_color = NSColor_rgba(
            s_rgb[0] as f64 / 255.0,
            s_rgb[1] as f64 / 255.0,
            s_rgb[2] as f64 / 255.0,
            1.0,
        );
        let s_dash = make_label("—", 9.0, true, s_color);
        set_frame(s_dash, legend_x, legend_y, 14.0, 12.0);
        let () = msg_send![view, addSubview: s_dash];
        let s_label = make_label("Session", 9.0, false, secondary_color());
        set_frame(s_label, legend_x + 14.0, legend_y, 46.0, 12.0);
        let () = msg_send![view, addSubview: s_label];
        legend_x += 64.0;
    }

    if show_w {
        let w_pct = filtered.last().map(|e| e.weekly).unwrap_or(0.0);
        let w_rgb = threshold_rgb(w_pct, cfg.color_palette, &cfg.color_thresholds);
        let w_color = NSColor_rgba(
            w_rgb[0] as f64 / 255.0,
            w_rgb[1] as f64 / 255.0,
            w_rgb[2] as f64 / 255.0,
            1.0,
        );
        let w_dash = make_label("—", 9.0, true, w_color);
        set_frame(w_dash, legend_x, legend_y, 14.0, 12.0);
        let () = msg_send![view, addSubview: w_dash];
        let w_label = make_label("Weekly", 9.0, false, secondary_color());
        set_frame(w_label, legend_x + 14.0, legend_y, 46.0, 12.0);
        let () = msg_send![view, addSubview: w_label];
    }

    // Y-axis labels (max at top, min at bottom of chart area)
    let chart_bottom: f64 = 4.0;
    let max_label = make_label(&format!("{:.0}%", y_max_val), 9.0, false, secondary_color());
    set_frame(
        max_label,
        16.0,
        chart_bottom + SPARKLINE_PT_H - 12.0,
        30.0,
        12.0,
    );
    let () = msg_send![view, addSubview: max_label];

    let min_label = make_label(&format!("{:.0}%", y_min_val), 9.0, false, secondary_color());
    set_frame(min_label, 16.0, chart_bottom, 30.0, 12.0);
    let () = msg_send![view, addSubview: min_label];

    // NSImage from raw pixels
    let ns_image = raw_to_nsimage(&raw);
    if ns_image == nil {
        return None;
    }

    let image_view: id = msg_send![class!(NSImageView), alloc];
    let image_view: id = msg_send![image_view, initWithFrame: NSRect::new(
        NSPoint::new(chart_x, chart_bottom),
        NSSize::new(chart_w, SPARKLINE_PT_H),
    )];
    let () = msg_send![image_view, setImage: ns_image];
    let () = msg_send![image_view, setImageScaling: 3i64]; // NSImageScaleProportionallyUpOrDown
    let () = msg_send![view, addSubview: image_view];

    Some(view)
}

/// Interpolate a series of (timestamp, value) points at a given pixel x position.
fn interpolate_series(points: &[(i64, f32)], px: u32, w: u32, min_t: f64, t_range: f64) -> f64 {
    let t_frac = px as f64 / (w - 1) as f64;
    let t_at_px = min_t + t_frac * t_range;

    let mut val = points[0].1 as f64;
    for i in 0..points.len().saturating_sub(1) {
        let (t0, v0) = (points[i].0 as f64, points[i].1 as f64);
        let (t1, v1) = (points[i + 1].0 as f64, points[i + 1].1 as f64);
        if t_at_px <= t1 {
            let frac = ((t_at_px - t0) / (t1 - t0).max(1.0)).clamp(0.0, 1.0);
            val = v0 + frac * (v1 - v0);
            break;
        }
        val = v1;
    }
    val
}

/// Simple alpha-over blend for chart pixels.
fn alpha_blend_chart(dst: image::Rgba<u8>, src: image::Rgba<u8>) -> image::Rgba<u8> {
    use image::Rgba;
    let sa = src[3] as f32 / 255.0;
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a < 0.001 {
        return Rgba([0, 0, 0, 0]);
    }
    let r = (src[0] as f32 * sa + dst[0] as f32 * da * (1.0 - sa)) / out_a;
    let g = (src[1] as f32 * sa + dst[1] as f32 * da * (1.0 - sa)) / out_a;
    let b = (src[2] as f32 * sa + dst[2] as f32 * da * (1.0 - sa)) / out_a;
    Rgba([r as u8, g as u8, b as u8, (out_a * 255.0) as u8])
}

/// Get threshold color as [r, g, b] for a given percentage value.
fn threshold_rgb(
    pct: f32,
    palette: settings::ColorPalette,
    thresholds: &settings::ColorThresholds,
) -> [u8; 3] {
    let level = UsageLevel::from_percent(pct, thresholds);
    let c = level.color(palette);
    [c[0], c[1], c[2]]
}

/// Draw a threshold-colored line with area fill. Color changes per-pixel based on value.
fn draw_threshold_line(
    img: &mut image::RgbaImage,
    values: &[f64], // raw percentage values per pixel column
    line_y: &[f64], // y pixel positions per column
    thickness: f64,
    fill_alpha: f64,
    bottom_y: f64,
    palette: settings::ColorPalette,
    thresholds: &settings::ColorThresholds,
) {
    use image::Rgba;
    let w = img.width();
    let h = img.height();

    for px in 0..w {
        let cy = line_y[px as usize];
        let color = threshold_rgb(values[px as usize] as f32, palette, thresholds);

        for py in 0..h {
            let fy = py as f64;

            let dist = (fy - cy).abs();
            if dist < thickness {
                let src = Rgba([color[0], color[1], color[2], 255]);
                let dst = *img.get_pixel(px, py);
                img.put_pixel(px, py, alpha_blend_chart(dst, src));
            } else if dist < thickness + 1.0 {
                let a = ((thickness + 1.0 - dist) * 255.0) as u8;
                let src = Rgba([color[0], color[1], color[2], a]);
                let dst = *img.get_pixel(px, py);
                img.put_pixel(px, py, alpha_blend_chart(dst, src));
            } else if fill_alpha > 0.0 && fy > cy + thickness && fy <= bottom_y {
                let fill_span = bottom_y - (cy + thickness);
                if fill_span > 0.0 {
                    let t = (fy - (cy + thickness)) / fill_span;
                    let a = ((1.0 - t) * fill_alpha) as u8;
                    if a > 0 {
                        let src = Rgba([color[0], color[1], color[2], a]);
                        let dst = *img.get_pixel(px, py);
                        img.put_pixel(px, py, alpha_blend_chart(dst, src));
                    }
                }
            }
        }
    }
}

/// Render sparkline as raw RGBA pixels with threshold-based colors.
fn render_sparkline(
    session_pts: &[(i64, f32)],
    weekly_pts: &[(i64, f32)],
    show_session: bool,
    show_weekly: bool,
    palette: settings::ColorPalette,
    thresholds: &settings::ColorThresholds,
) -> Option<(RawIcon, f32, f32)> {
    use image::RgbaImage;

    let w = SPARKLINE_W;
    let h = SPARKLINE_H;
    let mut img = RgbaImage::new(w, h);

    let min_t = session_pts.first()?.0 as f64;
    let max_t = session_pts.last()?.0 as f64;
    let t_range = (max_t - min_t).max(1.0);

    let y_range = 100.0f64;
    let margin = 4u32;
    let draw_h = (h - margin * 2) as f64;
    let bottom_y = (h - margin) as f64;
    let line_thickness = 3.0f64;

    let compute = |points: &[(i64, f32)]| -> (Vec<f64>, Vec<f64>) {
        let values: Vec<f64> = (0..w)
            .map(|px| interpolate_series(points, px, w, min_t, t_range))
            .collect();
        let ys: Vec<f64> = values
            .iter()
            .map(|&val| {
                let y_frac = (val / y_range).clamp(0.0, 1.0);
                margin as f64 + draw_h * (1.0 - y_frac)
            })
            .collect();
        (values, ys)
    };

    // Draw weekly first (behind), then session on top
    if show_weekly {
        let (vals, ys) = compute(weekly_pts);
        draw_threshold_line(
            &mut img,
            &vals,
            &ys,
            line_thickness,
            30.0,
            bottom_y,
            palette,
            thresholds,
        );
    }
    if show_session {
        let (vals, ys) = compute(session_pts);
        draw_threshold_line(
            &mut img,
            &vals,
            &ys,
            line_thickness,
            50.0,
            bottom_y,
            palette,
            thresholds,
        );
    }

    Some((
        RawIcon {
            rgba: img.into_raw(),
            width: w,
            height: h,
        },
        0.0,
        100.0,
    ))
}

/// Convert a RawIcon to an NSImage (retina, 2x).
unsafe fn raw_to_nsimage(raw: &RawIcon) -> id {
    let _pool = NSAutoreleasePool::new(nil);

    let rep: id = msg_send![class!(NSBitmapImageRep), alloc];
    let rep: id = msg_send![rep,
        initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>()
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
        return nil;
    }

    // Copy pixel data into the rep's own buffer
    let rep_data: *mut u8 = msg_send![rep, bitmapData];
    if !rep_data.is_null() {
        std::ptr::copy_nonoverlapping(raw.rgba.as_ptr(), rep_data, raw.rgba.len());
    }

    let img: id = msg_send![class!(NSImage), alloc];
    let logical = NSSize::new(raw.width as f64 / 2.0, raw.height as f64 / 2.0);
    let img: id = msg_send![img, initWithSize: logical];
    let () = msg_send![img, addRepresentation: rep];
    img
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

        let cfg = settings::get();
        let fill_color = bar_fill_color(pct, cfg.color_palette, &cfg.color_thresholds);
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

unsafe fn bar_fill_color(
    pct: f32,
    palette: settings::ColorPalette,
    thresholds: &settings::ColorThresholds,
) -> id {
    let level = UsageLevel::from_percent(pct, thresholds);
    match palette {
        settings::ColorPalette::Default => match level {
            UsageLevel::Low => NSColor_rgba(0.20, 0.78, 0.35, 1.0), // green
            UsageLevel::Medium => NSColor_rgba(1.0, 0.80, 0.0, 1.0), // yellow
            UsageLevel::High => NSColor_rgba(1.0, 0.58, 0.0, 1.0),  // orange
            UsageLevel::Critical => NSColor_rgba(1.0, 0.23, 0.19, 1.0), // red
        },
        settings::ColorPalette::Monochrome => match level {
            UsageLevel::Low => NSColor_rgba(0.55, 0.55, 0.55, 1.0),
            UsageLevel::Medium => NSColor_rgba(0.65, 0.65, 0.65, 1.0),
            UsageLevel::High => NSColor_rgba(0.75, 0.75, 0.75, 1.0),
            UsageLevel::Critical => NSColor_rgba(0.85, 0.85, 0.85, 1.0),
        },
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

/// Build the Settings submenu with style and section visibility options.
unsafe fn settings_submenu_item(cfg: &settings::Settings) -> id {
    let handler = MENU_HANDLER.get().map(|h| h.0).unwrap_or(nil);

    let sub: id = msg_send![class!(NSMenu), new];

    // ── Show Usage Categories toggles ──
    let sections_header: id = msg_send![class!(NSMenuItem), alloc];
    let sections_header: id = msg_send![sections_header,
        initWithTitle: NSString::alloc(nil).init_str("Show Usage Categories")
        action: cocoa::base::nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![sections_header, setEnabled: NO];
    let () = msg_send![sub, addItem: sections_header];

    let sections = [
        ("Session", cfg.show_session, 0isize),
        ("Weekly", cfg.show_weekly, 1),
        ("Sonnet", cfg.show_sonnet, 2),
        ("Extra Usage", cfg.show_extra, 3),
    ];

    for (label, enabled, tag) in &sections {
        let item: id = msg_send![class!(NSMenuItem), alloc];
        let item: id = msg_send![item,
            initWithTitle: NSString::alloc(nil).init_str(label)
            action: sel!(toggleSectionAction:)
            keyEquivalent: NSString::alloc(nil).init_str("")
        ];
        let () = msg_send![item, setTarget: handler];
        let () = msg_send![item, setTag: *tag];
        if *enabled {
            let () = msg_send![item, setState: 1i64]; // NSControlStateValueOn
        }
        let () = msg_send![sub, addItem: item];
    }

    let () = msg_send![sub, addItem: separator()];

    // ── Show Icon toggle ──
    let icon_toggle: id = msg_send![class!(NSMenuItem), alloc];
    let icon_toggle: id = msg_send![icon_toggle,
        initWithTitle: NSString::alloc(nil).init_str("Show Icon")
        action: sel!(toggleIconAction:)
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![icon_toggle, setTarget: handler];
    if cfg.show_icon {
        let () = msg_send![icon_toggle, setState: 1i64];
    }
    let () = msg_send![sub, addItem: icon_toggle];

    // ── Show Number toggle ──
    let num_toggle: id = msg_send![class!(NSMenuItem), alloc];
    let num_toggle: id = msg_send![num_toggle,
        initWithTitle: NSString::alloc(nil).init_str("Show Number")
        action: sel!(toggleNumberAction:)
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![num_toggle, setTarget: handler];
    if cfg.show_number {
        let () = msg_send![num_toggle, setState: 1i64];
    }
    let () = msg_send![sub, addItem: num_toggle];

    let () = msg_send![sub, addItem: separator()];

    // ── Monochrome toggle ──
    let mono_item: id = msg_send![class!(NSMenuItem), alloc];
    let mono_item: id = msg_send![mono_item,
        initWithTitle: NSString::alloc(nil).init_str("Monochrome")
        action: sel!(toggleMonochromeAction:)
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![mono_item, setTarget: handler];
    if cfg.color_palette == settings::ColorPalette::Monochrome {
        let () = msg_send![mono_item, setState: 1i64];
    }
    let () = msg_send![sub, addItem: mono_item];

    // ── Icons Colored toggle ──
    let icons_colored_item: id = msg_send![class!(NSMenuItem), alloc];
    let icons_colored_item: id = msg_send![icons_colored_item,
        initWithTitle: NSString::alloc(nil).init_str("Icons Colored")
        action: sel!(toggleIconsColoredAction:)
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![icons_colored_item, setTarget: handler];
    if cfg.icons_colored {
        let () = msg_send![icons_colored_item, setState: 1i64];
    }
    let () = msg_send![sub, addItem: icons_colored_item];

    // ── Neutral Text toggle ──
    let neutral_text_item: id = msg_send![class!(NSMenuItem), alloc];
    let neutral_text_item: id = msg_send![neutral_text_item,
        initWithTitle: NSString::alloc(nil).init_str("Neutral Text")
        action: sel!(toggleNeutralTextAction:)
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![neutral_text_item, setTarget: handler];
    if cfg.neutral_text {
        let () = msg_send![neutral_text_item, setState: 1i64];
    }
    let () = msg_send![sub, addItem: neutral_text_item];

    let () = msg_send![sub, addItem: separator()];

    // ── Usage Thresholds preset group ──
    let thresh_header: id = msg_send![class!(NSMenuItem), alloc];
    let thresh_header: id = msg_send![thresh_header,
        initWithTitle: NSString::alloc(nil).init_str("Usage Thresholds")
        action: cocoa::base::nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![thresh_header, setEnabled: NO];
    let () = msg_send![sub, addItem: thresh_header];

    for (i, (label, preset)) in settings::ColorThresholds::PRESETS.iter().enumerate() {
        let item: id = msg_send![class!(NSMenuItem), alloc];
        let item: id = msg_send![item,
            initWithTitle: NSString::alloc(nil).init_str(label)
            action: sel!(setThresholdsAction:)
            keyEquivalent: NSString::alloc(nil).init_str("")
        ];
        let () = msg_send![item, setTarget: handler];
        let () = msg_send![item, setTag: i as isize];
        if cfg.color_thresholds == *preset {
            let () = msg_send![item, setState: 1i64];
        }
        let () = msg_send![sub, addItem: item];
    }

    let () = msg_send![sub, addItem: separator()];

    // ── Auto Refresh Interval radio group ──
    let interval_header: id = msg_send![class!(NSMenuItem), alloc];
    let interval_header: id = msg_send![interval_header,
        initWithTitle: NSString::alloc(nil).init_str("Auto Refresh Interval")
        action: cocoa::base::nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![interval_header, setEnabled: NO];
    let () = msg_send![sub, addItem: interval_header];

    for (secs, label) in &settings::RefreshInterval::OPTIONS {
        let item: id = msg_send![class!(NSMenuItem), alloc];
        let item: id = msg_send![item,
            initWithTitle: NSString::alloc(nil).init_str(label)
            action: sel!(setRefreshIntervalAction:)
            keyEquivalent: NSString::alloc(nil).init_str("")
        ];
        let () = msg_send![item, setTarget: handler];
        let () = msg_send![item, setTag: *secs as isize];
        if cfg.refresh_interval.0 == *secs {
            let () = msg_send![item, setState: 1i64];
        }
        let () = msg_send![sub, addItem: item];
    }

    let () = msg_send![sub, addItem: separator()];

    // ── Notifications ──
    let notify_item: id = msg_send![class!(NSMenuItem), alloc];
    let notify_item: id = msg_send![notify_item,
        initWithTitle: NSString::alloc(nil).init_str("Notifications")
        action: sel!(toggleNotifyAction:)
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![notify_item, setTarget: handler];
    if cfg.notify_enabled {
        let () = msg_send![notify_item, setState: 1i64];
    }
    let () = msg_send![sub, addItem: notify_item];

    // Per-section notification threshold submenus
    if cfg.notify_enabled {
        // Session threshold
        let session_sub: id = msg_send![class!(NSMenu), new];
        for threshold in &[50u32, 70, 80, 90, 95] {
            let item: id = msg_send![class!(NSMenuItem), alloc];
            let label = format!("{}%", threshold);
            let item: id = msg_send![item,
                initWithTitle: NSString::alloc(nil).init_str(&label)
                action: sel!(setSessionThresholdAction:)
                keyEquivalent: NSString::alloc(nil).init_str("")
            ];
            let () = msg_send![item, setTarget: handler];
            let () = msg_send![item, setTag: *threshold as isize];
            if cfg.notify_session_threshold == *threshold {
                let () = msg_send![item, setState: 1i64];
            }
            let () = msg_send![session_sub, addItem: item];
        }
        let session_parent: id = msg_send![class!(NSMenuItem), alloc];
        let session_parent: id = msg_send![session_parent,
            initWithTitle: NSString::alloc(nil).init_str(&format!("Session alert at {}%", cfg.notify_session_threshold))
            action: cocoa::base::nil
            keyEquivalent: NSString::alloc(nil).init_str("")
        ];
        let () = msg_send![session_parent, setSubmenu: session_sub];
        let () = msg_send![sub, addItem: session_parent];

        // Weekly threshold
        let weekly_sub: id = msg_send![class!(NSMenu), new];
        for threshold in &[50u32, 70, 80, 90, 95] {
            let item: id = msg_send![class!(NSMenuItem), alloc];
            let label = format!("{}%", threshold);
            let item: id = msg_send![item,
                initWithTitle: NSString::alloc(nil).init_str(&label)
                action: sel!(setWeeklyThresholdAction:)
                keyEquivalent: NSString::alloc(nil).init_str("")
            ];
            let () = msg_send![item, setTarget: handler];
            let () = msg_send![item, setTag: *threshold as isize];
            if cfg.notify_weekly_threshold == *threshold {
                let () = msg_send![item, setState: 1i64];
            }
            let () = msg_send![weekly_sub, addItem: item];
        }
        let weekly_parent: id = msg_send![class!(NSMenuItem), alloc];
        let weekly_parent: id = msg_send![weekly_parent,
            initWithTitle: NSString::alloc(nil).init_str(&format!("Weekly alert at {}%", cfg.notify_weekly_threshold))
            action: cocoa::base::nil
            keyEquivalent: NSString::alloc(nil).init_str("")
        ];
        let () = msg_send![weekly_parent, setSubmenu: weekly_sub];
        let () = msg_send![sub, addItem: weekly_parent];
    }

    let () = msg_send![sub, addItem: separator()];

    // ── Launch at Login ──
    let login_item: id = msg_send![class!(NSMenuItem), alloc];
    let login_item: id = msg_send![login_item,
        initWithTitle: NSString::alloc(nil).init_str("Launch at Login")
        action: sel!(toggleLaunchAtLoginAction:)
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![login_item, setTarget: handler];
    if cfg.launch_at_login {
        let () = msg_send![login_item, setState: 1i64];
    }
    let () = msg_send![sub, addItem: login_item];

    // ── Show in Dock toggle ──
    let dock_item: id = msg_send![class!(NSMenuItem), alloc];
    let dock_item: id = msg_send![dock_item,
        initWithTitle: NSString::alloc(nil).init_str("Show in Dock")
        action: sel!(toggleShowInDockAction:)
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![dock_item, setTarget: handler];
    if cfg.show_in_dock {
        let () = msg_send![dock_item, setState: 1i64];
    }
    let () = msg_send![sub, addItem: dock_item];

    // ── Create the parent item with gear icon ──
    let parent: id = msg_send![class!(NSMenuItem), alloc];
    let parent: id = msg_send![parent,
        initWithTitle: NSString::alloc(nil).init_str("Settings")
        action: cocoa::base::nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];

    // SF Symbol gear icon (macOS 11+)
    let gear_name = NSString::alloc(nil).init_str("gearshape");
    let gear_img: id = msg_send![class!(NSImage), imageWithSystemSymbolName: gear_name accessibilityDescription: nil];
    if gear_img != nil {
        let () = msg_send![parent, setImage: gear_img];
    }

    let () = msg_send![parent, setSubmenu: sub];
    parent
}

unsafe fn refresh_item() -> id {
    let s = NSString::alloc(nil).init_str("Refresh");
    let item: id = msg_send![class!(NSMenuItem), alloc];
    let item: id = msg_send![item,
        initWithTitle: s
        action: sel!(refreshAction:)
        keyEquivalent: NSString::alloc(nil).init_str("r")
    ];
    if let Some(handler) = MENU_HANDLER.get() {
        let () = msg_send![item, setTarget: handler.0];
    }
    item
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

/// Load Berkeley Mono font, with fallbacks to BerkeleyMono-Regular then system monospace.
unsafe fn berkeley_mono(size: f64) -> id {
    let name1 = NSString::alloc(nil).init_str("Berkeley Mono");
    let font: id = msg_send![class!(NSFont), fontWithName: name1 size: size];
    if font != nil {
        return font;
    }
    let name2 = NSString::alloc(nil).init_str("BerkeleyMono-Regular");
    let font: id = msg_send![class!(NSFont), fontWithName: name2 size: size];
    if font != nil {
        return font;
    }
    msg_send![class!(NSFont), monospacedSystemFontOfSize: size weight: 0.0f64]
}

/// Return index of the section with the highest usage percentage.
/// Session wins ties.
fn active_section_index(sections: &[SectionInfo]) -> usize {
    let mut best = 0usize;
    let mut best_pct = f32::NEG_INFINITY;
    for (i, sec) in sections.iter().enumerate() {
        if sec.pct > best_pct {
            best_pct = sec.pct;
            best = i;
        }
    }
    best
}

/// Render a pill-style menubar image using native NSFont drawing.
/// The active section (highest usage) gets a white pill background with dark text.
/// Other sections are dimmed. Small colored dots indicate usage level.
unsafe fn render_pill_image(sections: &[SectionInfo], cfg: &settings::Settings) -> id {
    let _pool = NSAutoreleasePool::new(nil);

    const PILL_SPACING: f64 = 6.0;
    const PILL_PAD_H: f64 = 6.0;
    const PILL_PAD_V: f64 = 2.0;
    const PILL_CORNER_RADIUS: f64 = 4.0;
    const DOT_DIAMETER: f64 = 5.0;
    const DOT_TEXT_GAP: f64 = 3.0;
    const FONT_SIZE: f64 = 12.0;

    if sections.is_empty() {
        // No data placeholder
        let font = berkeley_mono(FONT_SIZE);
        let dimmed = NSColor_rgba(1.0, 1.0, 1.0, 0.5);
        let font_key = NSString::alloc(nil).init_str("NSFont");
        let color_key = NSString::alloc(nil).init_str("NSColor");
        let keys: [id; 2] = [font_key, color_key];
        let vals: [id; 2] = [font, dimmed];
        let attrs: id = msg_send![class!(NSDictionary),
            dictionaryWithObjects: vals.as_ptr()
            forKeys: keys.as_ptr()
            count: 2usize
        ];
        let placeholder = NSString::alloc(nil).init_str("...");
        let size: NSSize = msg_send![placeholder, sizeWithAttributes: attrs];
        let img_w = size.width + PILL_PAD_H * 2.0;
        let img_h = 22.0f64;
        let img: id = msg_send![class!(NSImage), alloc];
        let img: id = msg_send![img, initWithSize: NSSize::new(img_w, img_h)];
        let () = msg_send![img, lockFocus];
        let pt = NSPoint::new(PILL_PAD_H, (img_h - size.height) / 2.0);
        let () = msg_send![placeholder, drawAtPoint: pt withAttributes: attrs];
        let () = msg_send![img, unlockFocus];
        let () = msg_send![img, setTemplate: false];
        return img;
    }

    let active = active_section_index(sections);
    let font = berkeley_mono(FONT_SIZE);

    // Build label strings and measure them
    let font_key = NSString::alloc(nil).init_str("NSFont");
    let color_key = NSString::alloc(nil).init_str("NSColor");

    // Use a neutral color just for measuring
    let measure_color: id = msg_send![class!(NSColor), whiteColor];
    let measure_keys: [id; 2] = [font_key, color_key];
    let measure_vals: [id; 2] = [font, measure_color];
    let measure_attrs: id = msg_send![class!(NSDictionary),
        dictionaryWithObjects: measure_vals.as_ptr()
        forKeys: measure_keys.as_ptr()
        count: 2usize
    ];

    struct PillSection {
        label_ns: id,
        label_size: NSSize,
        has_dot: bool,
        has_text: bool,
    }

    let mut pill_sections: Vec<PillSection> = Vec::new();

    for sec in sections.iter() {
        let text = if cfg.show_number {
            format!("{} {:.0}%", sec.label, sec.pct)
        } else {
            sec.label.to_string()
        };
        let ns_str = NSString::alloc(nil).init_str(&text);
        let size: NSSize = msg_send![ns_str, sizeWithAttributes: measure_attrs];
        pill_sections.push(PillSection {
            label_ns: ns_str,
            label_size: size,
            has_dot: cfg.show_icon,
            has_text: true,
        });
    }

    // Calculate total image width
    let img_h = 22.0f64;
    let mut total_w = 0.0f64;
    for (i, ps) in pill_sections.iter().enumerate() {
        if i > 0 {
            total_w += PILL_SPACING;
        }
        let mut sec_w = 0.0;
        if ps.has_dot {
            sec_w += DOT_DIAMETER + DOT_TEXT_GAP;
        }
        if ps.has_text {
            sec_w += ps.label_size.width;
        }
        sec_w += PILL_PAD_H * 2.0;
        total_w += sec_w;
    }

    let img: id = msg_send![class!(NSImage), alloc];
    let img: id = msg_send![img, initWithSize: NSSize::new(total_w, img_h)];
    let () = msg_send![img, lockFocus];

    let mut x = 0.0f64;
    for (i, ps) in pill_sections.iter().enumerate() {
        if i > 0 {
            x += PILL_SPACING;
        }

        let mut sec_w = PILL_PAD_H * 2.0;
        if ps.has_dot {
            sec_w += DOT_DIAMETER + DOT_TEXT_GAP;
        }
        if ps.has_text {
            sec_w += ps.label_size.width;
        }

        let is_active = i == active;

        // Draw pill background for active section
        if is_active {
            let pill_rect = NSRect::new(
                NSPoint::new(x, (img_h - ps.label_size.height - PILL_PAD_V * 2.0) / 2.0),
                NSSize::new(sec_w, ps.label_size.height + PILL_PAD_V * 2.0),
            );
            let pill_color = NSColor_rgba(1.0, 1.0, 1.0, 0.9);
            let () = msg_send![pill_color, setFill];
            let path: id =
                msg_send![class!(NSBezierPath), bezierPathWithRoundedRect: pill_rect xRadius: PILL_CORNER_RADIUS yRadius: PILL_CORNER_RADIUS];
            let () = msg_send![path, fill];
        }

        let mut inner_x = x + PILL_PAD_H;

        // Draw colored dot
        if ps.has_dot {
            let dot_color = level_to_nscolor(sections[i].level, cfg.color_palette);
            let () = msg_send![dot_color, setFill];
            let dot_y = (img_h - DOT_DIAMETER) / 2.0;
            let dot_rect = NSRect::new(
                NSPoint::new(inner_x, dot_y),
                NSSize::new(DOT_DIAMETER, DOT_DIAMETER),
            );
            let oval: id = msg_send![class!(NSBezierPath), bezierPathWithOvalInRect: dot_rect];
            let () = msg_send![oval, fill];
            inner_x += DOT_DIAMETER + DOT_TEXT_GAP;
        }

        // Draw text
        if ps.has_text {
            let text_color = if is_active {
                NSColor_rgba(0.0, 0.0, 0.0, 0.85)
            } else {
                NSColor_rgba(1.0, 1.0, 1.0, 0.5)
            };
            let draw_keys: [id; 2] = [font_key, color_key];
            let draw_vals: [id; 2] = [font, text_color];
            let draw_attrs: id = msg_send![class!(NSDictionary),
                dictionaryWithObjects: draw_vals.as_ptr()
                forKeys: draw_keys.as_ptr()
                count: 2usize
            ];
            let text_y = (img_h - ps.label_size.height) / 2.0;
            let pt = NSPoint::new(inner_x, text_y);
            let () = msg_send![ps.label_ns, drawAtPoint: pt withAttributes: draw_attrs];
        }

        x += sec_w;
    }

    let () = msg_send![img, unlockFocus];
    let () = msg_send![img, setTemplate: false];
    img
}

/// Update the status bar button with pill-style image.
unsafe fn update_status_button(
    button: id,
    sections: &[SectionInfo],
    cfg: &settings::Settings,
) {
    let _pool = NSAutoreleasePool::new(nil);

    let pill_image = render_pill_image(sections, cfg);
    let () = msg_send![button, setImage: pill_image];
    let () = msg_send![button, setImagePosition: 1i64]; // NSImageOnly
    let empty: id = msg_send![class!(NSMutableAttributedString), alloc];
    let empty: id = msg_send![empty, init];
    let () = msg_send![button, setAttributedTitle: empty];
}

/// Map a UsageLevel to an NSColor for the pill dot.
unsafe fn level_to_nscolor(level: UsageLevel, palette: settings::ColorPalette) -> id {
    match palette {
        settings::ColorPalette::Monochrome => {
            msg_send![class!(NSColor), labelColor]
        }
        settings::ColorPalette::Default => match level {
            UsageLevel::Low => msg_send![class!(NSColor), systemGreenColor],
            UsageLevel::Medium => msg_send![class!(NSColor), systemYellowColor],
            UsageLevel::High => msg_send![class!(NSColor), systemOrangeColor],
            UsageLevel::Critical => msg_send![class!(NSColor), systemRedColor],
        },
    }
}

// ── Notifications ────────────────────────────────────────────────────

/// Track whether we already sent a notification for the current usage window
/// to avoid spamming. Reset when usage drops below threshold.
static NOTIFIED_SESSION: AtomicBool = AtomicBool::new(false);
static NOTIFIED_WEEKLY: AtomicBool = AtomicBool::new(false);

fn check_and_notify(usage: &ParsedUsage) {
    let cfg = settings::get();
    if !cfg.notify_enabled {
        return;
    }

    // Session (independent threshold)
    let session_threshold = cfg.notify_session_threshold as f32;
    if usage.session_percent >= session_threshold {
        if !NOTIFIED_SESSION.swap(true, Ordering::Relaxed) {
            send_notification(
                &format!("Session usage at {:.0}%", usage.session_percent),
                &format!(
                    "{}",
                    usage
                        .session_reset
                        .as_deref()
                        .map(|r| format!("Resets {}", format_reset(r)))
                        .unwrap_or_default()
                ),
            );
        }
    } else {
        NOTIFIED_SESSION.store(false, Ordering::Relaxed);
    }

    // Weekly (independent threshold)
    let weekly_threshold = cfg.notify_weekly_threshold as f32;
    if usage.weekly_percent >= weekly_threshold {
        if !NOTIFIED_WEEKLY.swap(true, Ordering::Relaxed) {
            send_notification(
                &format!("Weekly usage at {:.0}%", usage.weekly_percent),
                &format!(
                    "{}",
                    usage
                        .weekly_reset
                        .as_deref()
                        .map(|r| format!("Resets {}", format_reset(r)))
                        .unwrap_or_default()
                ),
            );
        }
    } else {
        NOTIFIED_WEEKLY.store(false, Ordering::Relaxed);
    }
}

fn send_notification(title: &str, body: &str) {
    unsafe {
        let center: id = msg_send![
            class!(NSUserNotificationCenter),
            defaultUserNotificationCenter
        ];
        let notif: id = msg_send![class!(NSUserNotification), new];
        let () = msg_send![notif, setTitle: NSString::alloc(nil).init_str(title)];
        let () = msg_send![notif, setInformativeText: NSString::alloc(nil).init_str(body)];
        let () = msg_send![notif, setSoundName: NSString::alloc(nil).init_str("default")];
        let () = msg_send![center, deliverNotification: notif];
    }
    tracing::info!("Notification sent: {}", title);
}

// ── Launch at Login ──────────────────────────────────────────────────

const LAUNCH_AGENT_LABEL: &str = "com.vibe-usage.launcher";

fn launch_agent_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", LAUNCH_AGENT_LABEL))
}

/// Resolve the .app bundle path if the binary is inside one (Contents/MacOS/),
/// otherwise fall back to the bare binary path.
fn resolve_app_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // Check if we're inside a .app bundle: <name>.app/Contents/MacOS/<binary>
    let parent = exe.parent()?; // MacOS/
    if parent.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = parent.parent()?; // Contents/
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?; // <name>.app
    if bundle.extension()?.to_str()? != "app" {
        return None;
    }
    Some(bundle.to_path_buf())
}

fn set_launch_at_login(enable: bool) {
    let path = launch_agent_path();
    if enable {
        let xml_escape = |s: &str| -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;")
        };

        // Prefer launching via `open <bundle>.app` so macOS reads the icon/Info.plist.
        // Fall back to bare binary if not running from a bundle.
        let plist = if let Some(bundle) = resolve_app_bundle() {
            let bundle_path = xml_escape(&bundle.display().to_string());
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>{bundle}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>"#,
                label = LAUNCH_AGENT_LABEL,
                bundle = bundle_path
            )
        } else {
            let exe = std::env::current_exe().unwrap_or_default();
            let exe_escaped = xml_escape(&exe.display().to_string());
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>"#,
                label = LAUNCH_AGENT_LABEL,
                exe = exe_escaped
            )
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, plist).ok();
        tracing::info!("Launch agent installed at {:?}", path);
    } else {
        std::fs::remove_file(&path).ok();
        tracing::info!("Launch agent removed from {:?}", path);
    }
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
