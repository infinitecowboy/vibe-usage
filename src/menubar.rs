use crate::{
    api::{ParsedUsage, UsageClient},
    icons::{IconSet, UsageLevel},
    keychain, ui,
};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::TrayIconBuilder;

pub struct AppState {
    pub usage: Option<ParsedUsage>,
    pub client: Option<UsageClient>,
    pub needs_refresh: bool,
    pub last_update: Option<Instant>,
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
        Ok(Self {
            state: Arc::new(Mutex::new(AppState {
                usage: None,
                client,
                needs_refresh: true,
                last_update: None,
            })),
            icons,
        })
    }

    pub fn run(self) -> Result<()> {
        let icons = self.icons;
        let state = self.state;

        // Build event loop first - this initializes NSApplication on macOS
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

        // Create tray icon - keep it alive for the lifetime of the app
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(build_menu(None)))
            .with_tooltip("Vibe Usage - Loading...")
            .with_icon(icons.green.clone())
            .build()?;

        // Wrap in Arc<Mutex> for sharing
        let tray = Arc::new(Mutex::new(tray_icon));

        // Background thread: fetches data and updates AppState only
        let bg_state = state.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
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
                        if let Ok(r) = rt.block_on(c.fetch_usage()) {
                            let usage = ParsedUsage::from(r);
                            let mut s = bg_state.lock().unwrap();
                            s.usage = Some(usage);
                            s.needs_refresh = false;
                            s.last_update = Some(Instant::now());
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
                        _ => {}
                    }
                }

                // Update tray icon and menu from main thread if state changed
                let current_usage = state.lock().unwrap().usage.clone();
                if current_usage != last_rendered {
                    if let Some(ref usage) = current_usage {
                        let level = UsageLevel::from_percent(usage.max_percent);
                        let tooltip = format!(
                            "Session: {:.0}% | Weekly: {:.0}%",
                            usage.session_percent, usage.weekly_percent
                        );
                        // Show percentage next to the icon
                        let title = format!("{:.0}%", usage.max_percent);

                        if let Ok(t) = tray.lock() {
                            let _ = t.set_icon(Some(icons.get(level).clone()));
                            let _ = t.set_tooltip(Some(tooltip));
                            let _ = t.set_title(Some(title));
                            let _ = t.set_menu(Some(Box::new(build_menu(Some(usage)))));
                        }
                    }
                    last_rendered = current_usage;
                }
            }
        });
    }
}

fn build_menu(usage: Option<&ParsedUsage>) -> Menu {
    let menu = Menu::new();

    // Header
    let _ = menu.append(&MenuItem::new("Claude Code Usage", false, None));
    let _ = menu.append(&PredefinedMenuItem::separator());

    if let Some(u) = usage {
        // Session usage
        let session_bar = ui::render_progress_bar(u.session_percent, 20);
        let _ = menu.append(&MenuItem::new(
            format!("Session:  {} {:.0}%", session_bar, u.session_percent),
            false,
            None,
        ));
        if let Some(ref reset) = u.session_reset {
            let _ = menu.append(&MenuItem::new(
                format!("          Resets {}", format_reset_time(reset)),
                false,
                None,
            ));
        }

        let _ = menu.append(&PredefinedMenuItem::separator());

        // Weekly usage
        let weekly_bar = ui::render_progress_bar(u.weekly_percent, 20);
        let _ = menu.append(&MenuItem::new(
            format!("Weekly:   {} {:.0}%", weekly_bar, u.weekly_percent),
            false,
            None,
        ));
        if let Some(ref reset) = u.weekly_reset {
            let _ = menu.append(&MenuItem::new(
                format!("          Resets {}", format_reset_time(reset)),
                false,
                None,
            ));
        }
    } else {
        let _ = menu.append(&MenuItem::new("Loading...", false, None));
    }

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
            // Today - show time only
            local.format("today at %-I:%M%p").to_string().to_lowercase()
        } else if local.date_naive() == (now + chrono::Duration::days(1)).date_naive() {
            // Tomorrow
            local
                .format("tomorrow at %-I:%M%p")
                .to_string()
                .to_lowercase()
        } else {
            // Other day
            local.format("%b %-d at %-I:%M%p").to_string()
        }
    } else {
        iso.to_string()
    }
}
