use crate::{
    api::{ParsedUsage, UsageClient},
    history,
    icons::UsageLevel,
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

// ── Layout constants ────────────────────────────────────────────────

const MENU_WIDTH: f64 = 280.0;
const MENU_PAD_X: f64 = 16.0;
const BAR_WIDTH: f64 = MENU_WIDTH - MENU_PAD_X * 2.0;

const SEG_COUNT: usize = 10;
const SEG_GAP: f64 = 3.0;
const SEG_HEIGHT: f64 = 10.0;
const SEG_RADIUS: f64 = 2.0;

const RING_DIAM: f64 = 14.0;
const RING_STROKE: f64 = 2.2;
const RING_GAP: f64 = 6.0;
const MENUBAR_PAD: f64 = 3.0;

// ── Statics ─────────────────────────────────────────────────────────

struct SendId(id);
unsafe impl Send for SendId {}
unsafe impl Sync for SendId {}

static STATUS_ITEM: OnceLock<Mutex<SendId>> = OnceLock::new();
static STATUS_INFO: OnceLock<StatusInfo> = OnceLock::new();
static MENU_HANDLER: OnceLock<SendId> = OnceLock::new();
static REFRESH_REQUESTED: AtomicBool = AtomicBool::new(false);
static SETTINGS_CHANGED: AtomicBool = AtomicBool::new(false);

// ── App ─────────────────────────────────────────────────────────────

struct StatusInfo {
    version: String,
    model_alias: String,
    model_full: String,
    plan: String,
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

        STATUS_INFO.get_or_init(gather_status_info);

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
                let cfg = settings::get();
                let policy = if cfg.show_in_dock {
                    NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular
                } else {
                    NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory
                };
                app.setActivationPolicy_(policy);

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

            let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
            let length: f64 = -1.0; // NSVariableStatusItemLength
            let status_item: id = msg_send![status_bar, statusItemWithLength: length];
            let () = msg_send![status_item, retain];

            let button: id = msg_send![status_item, button];
            update_menubar_icon(button, None);

            let menu = build_menu(None);
            let () = msg_send![status_item, setMenu: menu];

            STATUS_ITEM.get_or_init(|| Mutex::new(SendId(status_item)));
            MENU_HANDLER.get_or_init(|| SendId(register_menu_handler()));
            tracing::info!("Status item created");
        }

        // Background fetcher
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

                let backoff = if failures > 0 {
                    Duration::from_secs((2u64.saturating_pow(failures)).min(interval_secs))
                } else {
                    Duration::ZERO
                };

                if should_fetch {
                    let client = bg_state.lock().unwrap().client.clone();
                    if let Some(c) = client {
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
                                    tracing::warn!("Fetch failed: {}", e);
                                    let mut s = bg_state.lock().unwrap();
                                    s.needs_refresh = false;
                                    s.last_update = Some(Instant::now());
                                    s.consecutive_failures =
                                        s.consecutive_failures.saturating_add(1);
                                }
                            },
                            Err(e) => {
                                tracing::warn!("Keychain read failed: {}", e);
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
        let mut last_dark: bool = unsafe { is_dark_appearance() };

        event_loop.run(move |event, _, cf| {
            *cf = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));

            if let Event::NewEvents(_) = event {
                if REFRESH_REQUESTED.swap(false, Ordering::Relaxed) {
                    let mut s = state.lock().unwrap();
                    s.needs_refresh = true;
                    s.consecutive_failures = 0;
                }

                let settings_dirty = SETTINGS_CHANGED.swap(false, Ordering::Relaxed);

                let cur_dark = unsafe { is_dark_appearance() };
                let appearance_changed = cur_dark != last_dark;
                last_dark = cur_dark;

                let current_usage = {
                    let s = state.lock().unwrap();
                    s.usage.clone()
                };

                if current_usage != last_rendered || settings_dirty || appearance_changed {
                    if let Some(si) = STATUS_ITEM.get() {
                        let si = si.lock().unwrap();
                        unsafe {
                            let button: id = msg_send![si.0, button];
                            update_menubar_icon(button, current_usage.as_ref());
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

// ── ObjC menu action handler ────────────────────────────────────────

unsafe fn register_menu_handler() -> id {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};

    let superclass = Class::get("NSObject").unwrap();
    let mut decl = ClassDecl::new("MenuHandler", superclass).unwrap();

    extern "C" fn refresh_action(_this: &Object, _cmd: Sel, _sender: id) {
        REFRESH_REQUESTED.store(true, Ordering::Relaxed);
    }

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

    extern "C" fn toggle_number_action(_this: &Object, _cmd: Sel, _sender: id) {
        settings::update(|s| s.show_number = !s.show_number);
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

    extern "C" fn set_refresh_interval_action(_this: &Object, _cmd: Sel, sender: id) {
        let tag: isize = unsafe { msg_send![sender, tag] };
        settings::update(|s| s.refresh_interval = settings::RefreshInterval(tag as u64));
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

    extern "C" fn toggle_launch_at_login_action(_this: &Object, _cmd: Sel, _sender: id) {
        let new_val = !settings::get().launch_at_login;
        settings::update(|s| s.launch_at_login = new_val);
        set_launch_at_login(new_val);
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

    decl.add_method(
        sel!(refreshAction:),
        refresh_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleSectionAction:),
        toggle_section_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleNumberAction:),
        toggle_number_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleMonochromeAction:),
        toggle_monochrome_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(setThresholdsAction:),
        set_thresholds_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(setRefreshIntervalAction:),
        set_refresh_interval_action as extern "C" fn(&Object, Sel, id),
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
        sel!(toggleLaunchAtLoginAction:),
        toggle_launch_at_login_action as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
        sel!(toggleShowInDockAction:),
        toggle_show_in_dock_action as extern "C" fn(&Object, Sel, id),
    );

    decl.register();

    let cls = Class::get("MenuHandler").unwrap();
    let obj: id = msg_send![cls, new];
    let () = msg_send![obj, retain];
    obj
}

// ── Menubar icon (ring) ─────────────────────────────────────────────

unsafe fn update_menubar_icon(button: id, usage: Option<&ParsedUsage>) {
    let _pool = NSAutoreleasePool::new(nil);
    let cfg = settings::get();

    let image = render_ring_image(usage, &cfg);
    let () = msg_send![button, setImage: image];
    let () = msg_send![button, setImagePosition: 1i64]; // NSImageOnly
    let empty: id = msg_send![class!(NSMutableAttributedString), new];
    let () = msg_send![button, setAttributedTitle: empty];
}

/// Render the menubar image: small rings for each visible section, optional %.
unsafe fn render_ring_image(usage: Option<&ParsedUsage>, cfg: &settings::Settings) -> id {
    let sections = visible_menubar_sections(usage, cfg);

    let font = menubar_font(11.0);
    let text_color = menubar_text_color();

    let font_key = NSString::alloc(nil).init_str("NSFont");
    let color_key = NSString::alloc(nil).init_str("NSColor");

    // Measure text per section
    let measure_keys: [id; 2] = [font_key, color_key];
    let measure_vals: [id; 2] = [font, text_color];
    let measure_attrs: id = msg_send![class!(NSDictionary),
        dictionaryWithObjects: measure_vals.as_ptr()
        forKeys: measure_keys.as_ptr()
        count: 2usize
    ];

    struct MenubarSec {
        pct: f32,
        level: UsageLevel,
        text: Option<id>,
        text_w: f64,
    }

    let mut ms_secs: Vec<MenubarSec> = Vec::new();
    for sec in &sections {
        let (text, text_w) = if cfg.show_number {
            let s = NSString::alloc(nil).init_str(&format!("{:.0}%", sec.pct));
            let sz: NSSize = msg_send![s, sizeWithAttributes: measure_attrs];
            (Some(s), sz.width)
        } else {
            (None, 0.0)
        };
        ms_secs.push(MenubarSec {
            pct: sec.pct,
            level: sec.level,
            text,
            text_w,
        });
    }

    // Layout: for each section: ring + (gap + text)?, separator gap between sections
    let img_h: f64 = 22.0;
    let mut total_w: f64 = MENUBAR_PAD;
    for (i, ms) in ms_secs.iter().enumerate() {
        if i > 0 {
            total_w += RING_GAP + 2.0;
        }
        total_w += RING_DIAM;
        if ms.text.is_some() {
            total_w += 4.0 + ms.text_w;
        }
    }
    total_w += MENUBAR_PAD;

    if ms_secs.is_empty() {
        // Empty placeholder ring
        total_w = RING_DIAM + MENUBAR_PAD * 2.0;
    }

    let img: id = msg_send![class!(NSImage), alloc];
    let img: id = msg_send![img, initWithSize: NSSize::new(total_w, img_h)];
    let () = msg_send![img, lockFocus];

    if ms_secs.is_empty() {
        // Draw a single dim ring as placeholder (no data yet)
        let cx = MENUBAR_PAD + RING_DIAM / 2.0;
        let cy = img_h / 2.0;
        draw_ring(cx, cy, 0.0, track_color(), track_color());
    } else {
        let mut x = MENUBAR_PAD;
        for (i, ms) in ms_secs.iter().enumerate() {
            if i > 0 {
                x += RING_GAP + 2.0;
            }
            let cx = x + RING_DIAM / 2.0;
            let cy = img_h / 2.0;
            let progress_c = level_nscolor(ms.level, cfg.color_palette);
            draw_ring(cx, cy, ms.pct, track_color(), progress_c);
            x += RING_DIAM;

            if let Some(text) = ms.text {
                let text_x = x + 4.0;
                let sz: NSSize = msg_send![text, sizeWithAttributes: measure_attrs];
                let text_y = (img_h - sz.height) / 2.0;

                let draw_keys: [id; 2] = [font_key, color_key];
                let draw_vals: [id; 2] = [font, text_color];
                let draw_attrs: id = msg_send![class!(NSDictionary),
                    dictionaryWithObjects: draw_vals.as_ptr()
                    forKeys: draw_keys.as_ptr()
                    count: 2usize
                ];
                let pt = NSPoint::new(text_x, text_y);
                let () = msg_send![text, drawAtPoint: pt withAttributes: draw_attrs];
                x += 4.0 + ms.text_w;
            }
        }
    }

    let () = msg_send![img, unlockFocus];
    let () = msg_send![img, setTemplate: NO];
    img
}

/// Draw a single ring (track + progress arc) centered at (cx, cy).
unsafe fn draw_ring(cx: f64, cy: f64, pct: f32, track: id, progress: id) {
    let radius = RING_DIAM / 2.0 - RING_STROKE / 2.0;
    let rect = NSRect::new(
        NSPoint::new(cx - radius, cy - radius),
        NSSize::new(radius * 2.0, radius * 2.0),
    );

    // Track (full circle)
    let () = msg_send![track, setStroke];
    let track_path: id = msg_send![class!(NSBezierPath), bezierPathWithOvalInRect: rect];
    let () = msg_send![track_path, setLineWidth: RING_STROKE];
    let () = msg_send![track_path, stroke];

    // Progress arc (clockwise from top)
    let p = pct.clamp(0.0, 100.0) as f64;
    if p > 0.0 {
        let sweep = (p / 100.0) * 360.0;
        let start_angle = 90.0_f64;
        let end_angle = start_angle - sweep;
        let center = NSPoint::new(cx, cy);

        let arc: id = msg_send![class!(NSBezierPath), bezierPath];
        let () = msg_send![arc,
            appendBezierPathWithArcWithCenter: center
            radius: radius
            startAngle: start_angle
            endAngle: end_angle
            clockwise: YES
        ];
        let () = msg_send![arc, setLineWidth: RING_STROKE];
        let () = msg_send![arc, setLineCapStyle: 1i64]; // NSLineCapStyleRound
        let () = msg_send![progress, setStroke];
        let () = msg_send![arc, stroke];
    }
}

#[derive(Clone, Copy)]
struct MenubarSection {
    pct: f32,
    level: UsageLevel,
}

fn visible_menubar_sections(
    usage: Option<&ParsedUsage>,
    cfg: &settings::Settings,
) -> Vec<MenubarSection> {
    let mut v = Vec::new();
    let thresholds = &cfg.color_thresholds;

    if cfg.show_session {
        let pct = usage.map(|u| u.session_percent).unwrap_or(0.0);
        v.push(MenubarSection {
            pct,
            level: UsageLevel::from_percent(pct, thresholds),
        });
    }
    if cfg.show_weekly {
        let pct = usage.map(|u| u.weekly_percent).unwrap_or(0.0);
        v.push(MenubarSection {
            pct,
            level: UsageLevel::from_percent(pct, thresholds),
        });
    }
    v
}

// ── Menu ─────────────────────────────────────────────────────────────

unsafe fn build_menu(usage: Option<&ParsedUsage>) -> id {
    let menu: id = msg_send![class!(NSMenu), new];
    let () = msg_send![menu, setMinimumWidth: MENU_WIDTH];
    let cfg = settings::get();
    let mut has_section = false;

    if let Some(u) = usage {
        if cfg.show_session {
            add_usage_row(menu, "Session", u.session_percent, u.session_reset.as_deref());
            has_section = true;
        }
        if cfg.show_weekly {
            if has_section {
                let () = msg_send![menu, addItem: separator()];
            }
            add_usage_row(menu, "Weekly", u.weekly_percent, u.weekly_reset.as_deref());
            has_section = true;
        }
        if cfg.show_sonnet {
            if has_section {
                let () = msg_send![menu, addItem: separator()];
            }
            add_usage_row(
                menu,
                "Sonnet",
                u.sonnet_percent.unwrap_or(0.0),
                u.sonnet_reset.as_deref(),
            );
            has_section = true;
        }
        if cfg.show_extra {
            if has_section {
                let () = msg_send![menu, addItem: separator()];
            }
            let extra = if u.extra_usage_enabled {
                u.extra_usage_percent
                    .map(|p| format!("Extra usage · {:.0}%", p))
                    .unwrap_or_else(|| "Extra usage enabled".into())
            } else {
                "Extra usage not enabled".into()
            };
            let () = msg_send![menu, addItem: menu_item_with_view(extra_row_view(&extra))];
            has_section = true;
        }
    } else {
        let () = msg_send![menu, addItem: menu_item_with_view(loading_view())];
        has_section = true;
    }

    if let Some(info) = STATUS_INFO.get() {
        if has_section {
            let () = msg_send![menu, addItem: separator()];
        }
        let () = msg_send![menu, addItem: menu_item_with_view(status_row_view(info))];
    }

    let () = msg_send![menu, addItem: separator()];
    let () = msg_send![menu, addItem: settings_submenu_item(&cfg)];
    let () = msg_send![menu, addItem: refresh_item()];
    let () = msg_send![menu, addItem: quit_item()];

    menu
}

/// Usage row: title + percent, segmented bar, reset time.
unsafe fn add_usage_row(menu: id, name: &str, pct: f32, reset: Option<&str>) {
    let total_h: f64 = if reset.is_some() { 52.0 } else { 38.0 };

    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, total_h),
    )];

    let mut y = total_h;

    // Title row
    y -= 18.0;
    let title = make_label(name, 13.0, true, primary_color());
    set_frame(title, MENU_PAD_X, y, MENU_WIDTH - 80.0, 16.0);
    let () = msg_send![view, addSubview: title];

    let pct_label = make_label(&format!("{:.0}%", pct), 13.0, false, secondary_color());
    set_frame(pct_label, MENU_WIDTH - MENU_PAD_X - 48.0, y, 48.0, 16.0);
    let () = msg_send![pct_label, setAlignment: 2u64]; // NSTextAlignmentRight
    let () = msg_send![view, addSubview: pct_label];

    // Segmented bar
    y -= SEG_HEIGHT + 4.0;
    let cfg = settings::get();
    let bar = make_segmented_bar(pct, BAR_WIDTH, &cfg);
    set_frame(bar, MENU_PAD_X, y, BAR_WIDTH, SEG_HEIGHT);
    let () = msg_send![view, addSubview: bar];

    // Reset time
    if let Some(r) = reset {
        y -= 14.0;
        let reset_label = make_label(
            &format!("Resets {}", format_reset(r)),
            10.0,
            false,
            tertiary_color(),
        );
        set_frame(reset_label, MENU_PAD_X, y, BAR_WIDTH, 12.0);
        let () = msg_send![view, addSubview: reset_label];
    }

    let () = msg_send![menu, addItem: menu_item_with_view(view)];
}

/// Segmented progress bar: N discrete cells, filled based on percent.
unsafe fn make_segmented_bar(pct: f32, w: f64, cfg: &settings::Settings) -> id {
    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(w, SEG_HEIGHT),
    )];
    let () = msg_send![view, setWantsLayer: YES];

    let total_gap = SEG_GAP * (SEG_COUNT as f64 - 1.0);
    let seg_w = (w - total_gap) / SEG_COUNT as f64;

    let p = pct.clamp(0.0, 100.0);
    let level = UsageLevel::from_percent(p, &cfg.color_thresholds);
    let fill_color = level_nscolor(level, cfg.color_palette);
    let track = track_color();
    let filled_cells = ((p / 100.0) * SEG_COUNT as f32).round() as usize;

    for i in 0..SEG_COUNT {
        let x = i as f64 * (seg_w + SEG_GAP);
        let seg: id = msg_send![class!(NSView), alloc];
        let seg: id = msg_send![seg, initWithFrame: NSRect::new(
            NSPoint::new(x, 0.0),
            NSSize::new(seg_w, SEG_HEIGHT),
        )];
        let () = msg_send![seg, setWantsLayer: YES];
        let layer: id = msg_send![seg, layer];
        let color = if i < filled_cells { fill_color } else { track };
        let cg: id = msg_send![color, CGColor];
        let () = msg_send![layer, setBackgroundColor: cg];
        let () = msg_send![layer, setCornerRadius: SEG_RADIUS];
        let () = msg_send![view, addSubview: seg];
    }

    view
}

unsafe fn loading_view() -> id {
    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, 24.0),
    )];
    let label = make_label("Loading\u{2026}", 12.0, false, secondary_color());
    set_frame(label, MENU_PAD_X, 4.0, BAR_WIDTH, 16.0);
    let () = msg_send![view, addSubview: label];
    view
}

unsafe fn extra_row_view(text: &str) -> id {
    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, 24.0),
    )];
    let label = make_label(text, 11.0, false, secondary_color());
    set_frame(label, MENU_PAD_X, 4.0, BAR_WIDTH, 16.0);
    let () = msg_send![view, addSubview: label];
    view
}

unsafe fn status_row_view(info: &StatusInfo) -> id {
    let row_h: f64 = 16.0;
    let rows: Vec<(&str, String)> = vec![
        ("Version", info.version.clone()),
        (
            "Model",
            format!("{} ({})", info.model_alias, info.model_full),
        ),
        ("Plan", info.plan.clone()),
    ];
    let total_h = rows.len() as f64 * row_h + 8.0;

    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(MENU_WIDTH, total_h),
    )];

    for (i, (key, val)) in rows.iter().enumerate() {
        let y = total_h - 4.0 - (i as f64 + 1.0) * row_h;

        let key_label = make_label(key, 11.0, true, tertiary_color());
        set_frame(key_label, MENU_PAD_X, y, 60.0, row_h);
        let () = msg_send![view, addSubview: key_label];

        let val_label = make_label(val, 11.0, false, secondary_color());
        set_frame(val_label, MENU_PAD_X + 64.0, y, MENU_WIDTH - MENU_PAD_X * 2.0 - 64.0, row_h);
        let () = msg_send![view, addSubview: val_label];
    }

    view
}

// ── Settings submenu ─────────────────────────────────────────────────

unsafe fn settings_submenu_item(cfg: &settings::Settings) -> id {
    let handler = MENU_HANDLER.get().map(|h| h.0).unwrap_or(nil);
    let sub: id = msg_send![class!(NSMenu), new];

    add_header(sub, "Show");
    for (label, enabled, tag) in [
        ("Session", cfg.show_session, 0isize),
        ("Weekly", cfg.show_weekly, 1),
        ("Sonnet", cfg.show_sonnet, 2),
        ("Extra Usage", cfg.show_extra, 3),
    ] {
        add_toggle(sub, handler, label, enabled, sel!(toggleSectionAction:), tag);
    }

    let () = msg_send![sub, addItem: separator()];

    add_toggle(
        sub,
        handler,
        "Show Percent",
        cfg.show_number,
        sel!(toggleNumberAction:),
        0,
    );
    add_toggle(
        sub,
        handler,
        "Monochrome",
        cfg.color_palette == settings::ColorPalette::Monochrome,
        sel!(toggleMonochromeAction:),
        0,
    );

    let () = msg_send![sub, addItem: separator()];

    add_header(sub, "Thresholds");
    for (i, (label, preset)) in settings::ColorThresholds::PRESETS.iter().enumerate() {
        add_toggle(
            sub,
            handler,
            label,
            cfg.color_thresholds == *preset,
            sel!(setThresholdsAction:),
            i as isize,
        );
    }

    let () = msg_send![sub, addItem: separator()];

    add_header(sub, "Auto Refresh");
    for (secs, label) in &settings::RefreshInterval::OPTIONS {
        add_toggle(
            sub,
            handler,
            label,
            cfg.refresh_interval.0 == *secs,
            sel!(setRefreshIntervalAction:),
            *secs as isize,
        );
    }

    let () = msg_send![sub, addItem: separator()];

    add_toggle(
        sub,
        handler,
        "Notifications",
        cfg.notify_enabled,
        sel!(toggleNotifyAction:),
        0,
    );

    if cfg.notify_enabled {
        add_threshold_submenu(
            sub,
            handler,
            &format!("Session alert at {}%", cfg.notify_session_threshold),
            cfg.notify_session_threshold,
            sel!(setSessionThresholdAction:),
        );
        add_threshold_submenu(
            sub,
            handler,
            &format!("Weekly alert at {}%", cfg.notify_weekly_threshold),
            cfg.notify_weekly_threshold,
            sel!(setWeeklyThresholdAction:),
        );
    }

    let () = msg_send![sub, addItem: separator()];

    add_toggle(
        sub,
        handler,
        "Launch at Login",
        cfg.launch_at_login,
        sel!(toggleLaunchAtLoginAction:),
        0,
    );
    add_toggle(
        sub,
        handler,
        "Show in Dock",
        cfg.show_in_dock,
        sel!(toggleShowInDockAction:),
        0,
    );

    let parent: id = msg_send![class!(NSMenuItem), alloc];
    let parent: id = msg_send![parent,
        initWithTitle: NSString::alloc(nil).init_str("Settings")
        action: nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let gear_name = NSString::alloc(nil).init_str("gearshape");
    let gear_img: id = msg_send![class!(NSImage),
        imageWithSystemSymbolName: gear_name
        accessibilityDescription: nil
    ];
    if gear_img != nil {
        let () = msg_send![parent, setImage: gear_img];
    }
    let () = msg_send![parent, setSubmenu: sub];
    parent
}

unsafe fn add_header(menu: id, title: &str) {
    let item: id = msg_send![class!(NSMenuItem), alloc];
    let item: id = msg_send![item,
        initWithTitle: NSString::alloc(nil).init_str(title)
        action: nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![item, setEnabled: NO];
    let () = msg_send![menu, addItem: item];
}

unsafe fn add_toggle(
    menu: id,
    handler: id,
    title: &str,
    enabled: bool,
    action: objc::runtime::Sel,
    tag: isize,
) {
    let item: id = msg_send![class!(NSMenuItem), alloc];
    let item: id = msg_send![item,
        initWithTitle: NSString::alloc(nil).init_str(title)
        action: action
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![item, setTarget: handler];
    let () = msg_send![item, setTag: tag];
    if enabled {
        let () = msg_send![item, setState: 1i64];
    }
    let () = msg_send![menu, addItem: item];
}

unsafe fn add_threshold_submenu(
    parent_menu: id,
    handler: id,
    title: &str,
    current: u32,
    action: objc::runtime::Sel,
) {
    let sub: id = msg_send![class!(NSMenu), new];
    for t in &[50u32, 70, 80, 90, 95] {
        let item: id = msg_send![class!(NSMenuItem), alloc];
        let label = format!("{}%", t);
        let item: id = msg_send![item,
            initWithTitle: NSString::alloc(nil).init_str(&label)
            action: action
            keyEquivalent: NSString::alloc(nil).init_str("")
        ];
        let () = msg_send![item, setTarget: handler];
        let () = msg_send![item, setTag: *t as isize];
        if *t == current {
            let () = msg_send![item, setState: 1i64];
        }
        let () = msg_send![sub, addItem: item];
    }
    let wrapper: id = msg_send![class!(NSMenuItem), alloc];
    let wrapper: id = msg_send![wrapper,
        initWithTitle: NSString::alloc(nil).init_str(title)
        action: nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![wrapper, setSubmenu: sub];
    let () = msg_send![parent_menu, addItem: wrapper];
}

unsafe fn refresh_item() -> id {
    let item: id = msg_send![class!(NSMenuItem), alloc];
    let item: id = msg_send![item,
        initWithTitle: NSString::alloc(nil).init_str("Refresh")
        action: sel!(refreshAction:)
        keyEquivalent: NSString::alloc(nil).init_str("r")
    ];
    if let Some(handler) = MENU_HANDLER.get() {
        let () = msg_send![item, setTarget: handler.0];
    }
    item
}

unsafe fn quit_item() -> id {
    let item: id = msg_send![class!(NSMenuItem), alloc];
    let item: id = msg_send![item,
        initWithTitle: NSString::alloc(nil).init_str("Quit")
        action: sel!(terminate:)
        keyEquivalent: NSString::alloc(nil).init_str("q")
    ];
    item
}

unsafe fn menu_item_with_view(view: id) -> id {
    let item: id = msg_send![class!(NSMenuItem), alloc];
    let item: id = msg_send![item,
        initWithTitle: NSString::alloc(nil).init_str("")
        action: nil
        keyEquivalent: NSString::alloc(nil).init_str("")
    ];
    let () = msg_send![item, setView: view];
    item
}

unsafe fn separator() -> id {
    msg_send![class!(NSMenuItem), separatorItem]
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

// ── Colors ───────────────────────────────────────────────────────────

unsafe fn primary_color() -> id {
    msg_send![class!(NSColor), labelColor]
}

unsafe fn secondary_color() -> id {
    msg_send![class!(NSColor), secondaryLabelColor]
}

unsafe fn tertiary_color() -> id {
    msg_send![class!(NSColor), tertiaryLabelColor]
}

unsafe fn track_color() -> id {
    // Dim neutral — adapts so it reads on both dark and light menubars/menus.
    use cocoa::appkit::NSColor;
    if is_dark_appearance() {
        NSColor::colorWithRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.22)
    } else {
        NSColor::colorWithRed_green_blue_alpha_(nil, 0.0, 0.0, 0.0, 0.13)
    }
}

unsafe fn menubar_text_color() -> id {
    // labelColor bakes to black at lockFocus time regardless of menubar appearance.
    // Choose explicitly so the % stays readable in both modes.
    use cocoa::appkit::NSColor;
    if is_dark_appearance() {
        NSColor::colorWithRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.92)
    } else {
        NSColor::colorWithRed_green_blue_alpha_(nil, 0.0, 0.0, 0.0, 0.87)
    }
}

unsafe fn is_dark_appearance() -> bool {
    let app = NSApp();
    if app == nil {
        return true;
    }
    let appearance: id = msg_send![app, effectiveAppearance];
    if appearance == nil {
        return true;
    }
    let dark_name = NSString::alloc(nil).init_str("NSAppearanceNameDarkAqua");
    let aqua_name = NSString::alloc(nil).init_str("NSAppearanceNameAqua");
    let names: id = msg_send![class!(NSArray),
        arrayWithObjects: [dark_name, aqua_name].as_ptr()
        count: 2usize
    ];
    let matched: id = msg_send![appearance, bestMatchFromAppearancesWithNames: names];
    if matched == nil {
        return true;
    }
    let is_dark: bool = msg_send![matched, isEqualToString: dark_name];
    is_dark
}

unsafe fn menubar_font(size: f64) -> id {
    msg_send![class!(NSFont), monospacedDigitSystemFontOfSize: size weight: 0.0f64]
}

unsafe fn level_nscolor(level: UsageLevel, palette: settings::ColorPalette) -> id {
    match palette {
        settings::ColorPalette::Monochrome => msg_send![class!(NSColor), labelColor],
        settings::ColorPalette::Default => match level {
            UsageLevel::Low => msg_send![class!(NSColor), systemGreenColor],
            UsageLevel::Medium => msg_send![class!(NSColor), systemYellowColor],
            UsageLevel::High => msg_send![class!(NSColor), systemOrangeColor],
            UsageLevel::Critical => msg_send![class!(NSColor), systemRedColor],
        },
    }
}

// ── Status info ──────────────────────────────────────────────────────

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

    StatusInfo {
        version,
        model_alias,
        model_full,
        plan,
    }
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
    (alias, full)
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

// ── Notifications ────────────────────────────────────────────────────

static NOTIFIED_SESSION: AtomicBool = AtomicBool::new(false);
static NOTIFIED_WEEKLY: AtomicBool = AtomicBool::new(false);

fn check_and_notify(usage: &ParsedUsage) {
    let cfg = settings::get();
    if !cfg.notify_enabled {
        return;
    }

    let session_t = cfg.notify_session_threshold as f32;
    if usage.session_percent >= session_t {
        if !NOTIFIED_SESSION.swap(true, Ordering::Relaxed) {
            let body = usage
                .session_reset
                .as_deref()
                .map(|r| format!("Resets {}", format_reset(r)))
                .unwrap_or_default();
            send_notification(
                &format!("Session usage at {:.0}%", usage.session_percent),
                &body,
            );
        }
    } else {
        NOTIFIED_SESSION.store(false, Ordering::Relaxed);
    }

    let weekly_t = cfg.notify_weekly_threshold as f32;
    if usage.weekly_percent >= weekly_t {
        if !NOTIFIED_WEEKLY.swap(true, Ordering::Relaxed) {
            let body = usage
                .weekly_reset
                .as_deref()
                .map(|r| format!("Resets {}", format_reset(r)))
                .unwrap_or_default();
            send_notification(
                &format!("Weekly usage at {:.0}%", usage.weekly_percent),
                &body,
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

fn resolve_app_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    if parent.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = parent.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
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

// ── Date formatting ──────────────────────────────────────────────────

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
