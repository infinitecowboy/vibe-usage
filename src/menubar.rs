use crate::{
    api::{ParsedUsage, UsageClient},
    icons::{IconSet, IconStyle, UsageLevel},
    keychain, ui,
};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::TrayIconBuilder;

pub struct AppState {
    pub usage: Option<ParsedUsage>,
    pub client: Option<UsageClient>,
    pub needs_refresh: bool,
    pub last_update: Option<Instant>,
    pub icon_style: IconStyle,
}

pub struct MenubarApp {
    state: Arc<Mutex<AppState>>,
    icons: IconSet,
}

impl MenubarApp {
    pub fn new() -> Result<Self> {
        let icons = IconSet::new()?;
        let client = keychain::get_oauth_token()
            .ok()
            .and_then(|t| UsageClient::new(t).ok());

        // Load saved preference or default to Dot
        let icon_style = load_icon_style_preference();

        Ok(Self {
            state: Arc::new(Mutex::new(AppState {
                usage: None,
                client,
                needs_refresh: true,
                last_update: None,
                icon_style,
            })),
            icons,
        })
    }

    pub fn run(self) -> Result<()> {
        let icons = self.icons;
        let state = self.state;

        let event_loop = EventLoopBuilder::new().build();

        // Set activation policy to hide dock icon
        #[cfg(target_os = "macos")]
        {
            use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
            use cocoa::base::nil;
            unsafe {
                let app = NSApp();
                if app != nil {
                    app.setActivationPolicy_(
                        NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
                    );
                }
            }
        }

        let initial_style = state.lock().unwrap().icon_style;

        // Create tray icon
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(build_menu(None, initial_style)))
            .with_tooltip("Vibe Usage - Loading...")
            .with_icon(icons.green.clone())
            .build()?;

        let tray = Arc::new(Mutex::new(tray_icon));

        // Background thread: fetches data and updates AppState
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
        let mut last_style: Option<IconStyle> = None;

        event_loop.run(move |event, _, cf| {
            *cf = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));

            if let Event::NewEvents(_) = event {
                // Check for menu events
                if let Ok(e) = MenuEvent::receiver().try_recv() {
                    match e.id.0.as_str() {
                        "quit" => {
                            *cf = ControlFlow::Exit;
                            return;
                        }
                        "refresh" => {
                            state.lock().unwrap().needs_refresh = true;
                        }
                        "style_dot" => {
                            let mut s = state.lock().unwrap();
                            s.icon_style = IconStyle::Dot;
                            save_icon_style_preference(IconStyle::Dot);
                        }
                        "style_bars" => {
                            let mut s = state.lock().unwrap();
                            s.icon_style = IconStyle::Bars;
                            save_icon_style_preference(IconStyle::Bars);
                        }
                        _ => {}
                    }
                }

                // Get current state
                let (current_usage, current_style) = {
                    let s = state.lock().unwrap();
                    (s.usage.clone(), s.icon_style)
                };

                // Update if usage or style changed
                let style_changed = last_style != Some(current_style);
                let usage_changed = current_usage != last_rendered;

                if usage_changed || style_changed {
                    if let Some(ref usage) = current_usage {
                        let level = UsageLevel::from_percent(usage.max_percent);
                        let tooltip = format!(
                            "Session: {:.0}% | Weekly: {:.0}%",
                            usage.session_percent, usage.weekly_percent
                        );
                        let title = format!("{:.0}%", usage.max_percent);

                        if let Ok(t) = tray.lock() {
                            // Get appropriate icon based on style
                            let icon = match current_style {
                                IconStyle::Dot => icons.get(level).clone(),
                                IconStyle::Bars => icons.get_bars(usage).clone(),
                            };
                            let _ = t.set_icon(Some(icon));
                            let _ = t.set_tooltip(Some(tooltip));
                            t.set_title(Some(title));
                            t.set_menu(Some(Box::new(build_menu(Some(usage), current_style))));
                        }
                    }
                    last_rendered = current_usage;
                    last_style = Some(current_style);
                }
            }
        });
    }
}

fn build_menu(usage: Option<&ParsedUsage>, icon_style: IconStyle) -> Menu {
    let menu = Menu::new();

    // Header
    let _ = menu.append(&MenuItem::new("Claude Code Usage", true, None));
    let _ = menu.append(&PredefinedMenuItem::separator());

    if let Some(u) = usage {
        // Session usage
        let session_bar = ui::render_progress_bar(u.session_percent, 20);
        let _ = menu.append(&MenuItem::new(
            format!("Session:      {} {:.0}%", session_bar, u.session_percent),
            true,
            None,
        ));
        if let Some(ref reset) = u.session_reset {
            let _ = menu.append(&MenuItem::new(
                format!("              Resets {}", format_reset_time(reset)),
                true,
                None,
            ));
        }

        let _ = menu.append(&PredefinedMenuItem::separator());

        // Weekly usage (all models)
        let weekly_bar = ui::render_progress_bar(u.weekly_percent, 20);
        let _ = menu.append(&MenuItem::new(
            format!("Weekly (all): {} {:.0}%", weekly_bar, u.weekly_percent),
            true,
            None,
        ));
        if let Some(ref reset) = u.weekly_reset {
            let _ = menu.append(&MenuItem::new(
                format!("              Resets {}", format_reset_time(reset)),
                true,
                None,
            ));
        }

        // Sonnet-only usage (if available)
        if let Some(sonnet_pct) = u.sonnet_percent {
            let _ = menu.append(&PredefinedMenuItem::separator());
            let sonnet_bar = ui::render_progress_bar(sonnet_pct, 20);
            let _ = menu.append(&MenuItem::new(
                format!("Sonnet only:  {} {:.0}%", sonnet_bar, sonnet_pct),
                true,
                None,
            ));
            if let Some(ref reset) = u.sonnet_reset {
                let _ = menu.append(&MenuItem::new(
                    format!("              Resets {}", format_reset_time(reset)),
                    true,
                    None,
                ));
            }
        }

        // Extra usage section
        let _ = menu.append(&PredefinedMenuItem::separator());
        if u.extra_usage_enabled {
            if let Some(extra_pct) = u.extra_usage_percent {
                let extra_bar = ui::render_progress_bar(extra_pct, 20);
                let _ = menu.append(&MenuItem::new(
                    format!("Extra usage:  {} {:.0}%", extra_bar, extra_pct),
                    true,
                    None,
                ));
            } else {
                let _ = menu.append(&MenuItem::new("Extra usage:  enabled", true, None));
            }
        } else {
            let _ = menu.append(&MenuItem::new("Extra usage:  not enabled", true, None));
        }
    } else {
        let _ = menu.append(&MenuItem::new("Loading...", false, None));
    }

    let _ = menu.append(&PredefinedMenuItem::separator());

    // Options submenu
    let options = Submenu::new("Options", true);
    let dot_item = CheckMenuItem::with_id(
        "style_dot",
        "● Dot icon",
        true,
        icon_style == IconStyle::Dot,
        None,
    );
    let bars_item = CheckMenuItem::with_id(
        "style_bars",
        "▐ Bars icon",
        true,
        icon_style == IconStyle::Bars,
        None,
    );
    let _ = options.append(&dot_item);
    let _ = options.append(&bars_item);
    let _ = menu.append(&options);

    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("refresh", "↻ Refresh", true, None));
    let _ = menu.append(&MenuItem::with_id("quit", "Quit", true, None));

    menu
}

fn format_reset_time(iso: &str) -> String {
    use chrono::{DateTime, Local, Utc};

    if let Ok(dt) = iso.parse::<DateTime<Utc>>() {
        let local: DateTime<Local> = dt.into();
        let now = Local::now();

        if local.date_naive() == now.date_naive() {
            local.format("today at %-I:%M%p").to_string().to_lowercase()
        } else if local.date_naive() == (now + chrono::Duration::days(1)).date_naive() {
            local
                .format("tomorrow at %-I:%M%p")
                .to_string()
                .to_lowercase()
        } else {
            local.format("%b %-d at %-I:%M%p").to_string()
        }
    } else {
        iso.to_string()
    }
}

fn load_icon_style_preference() -> IconStyle {
    // Try to load from ~/.config/vibe-usage/config
    if let Some(home) = std::env::var_os("HOME") {
        let config_path = std::path::Path::new(&home)
            .join(".config")
            .join("vibe-usage")
            .join("style");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            match content.trim() {
                "bars" => return IconStyle::Bars,
                _ => return IconStyle::Dot,
            }
        }
    }
    IconStyle::Dot
}

fn save_icon_style_preference(style: IconStyle) {
    if let Some(home) = std::env::var_os("HOME") {
        let config_dir = std::path::Path::new(&home)
            .join(".config")
            .join("vibe-usage");
        let _ = std::fs::create_dir_all(&config_dir);
        let config_path = config_dir.join("style");
        let content = match style {
            IconStyle::Dot => "dot",
            IconStyle::Bars => "bars",
        };
        let _ = std::fs::write(config_path, content);
    }
}
