use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static SETTINGS: OnceLock<Mutex<Settings>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorPalette {
    #[default]
    Default,
    Monochrome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorThresholds {
    pub warning: u32,
    pub high: u32,
    pub critical: u32,
}

impl Default for ColorThresholds {
    fn default() -> Self {
        Self {
            warning: 50,
            high: 75,
            critical: 90,
        }
    }
}

impl ColorThresholds {
    pub const PRESETS: [(&'static str, ColorThresholds); 2] = [
        (
            "Default (50/75/90)",
            ColorThresholds {
                warning: 50,
                high: 75,
                critical: 90,
            },
        ),
        (
            "Conservative (30/60/90)",
            ColorThresholds {
                warning: 30,
                high: 60,
                critical: 90,
            },
        ),
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshInterval(pub u64);

impl RefreshInterval {
    pub const OPTIONS: [(u64, &'static str); 4] = [
        (60, "1 min"),
        (120, "2 min"),
        (300, "5 min"),
        (600, "10 min"),
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub show_session: bool,
    #[serde(default = "default_true")]
    pub show_weekly: bool,
    #[serde(default)]
    pub show_sonnet: bool,
    #[serde(default)]
    pub show_extra: bool,

    #[serde(default = "default_true")]
    pub show_number: bool,

    #[serde(default)]
    pub color_palette: ColorPalette,
    #[serde(default)]
    pub color_thresholds: ColorThresholds,

    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: RefreshInterval,

    #[serde(default)]
    pub notify_enabled: bool,
    #[serde(default = "default_notify_threshold")]
    pub notify_session_threshold: u32,
    #[serde(default = "default_notify_threshold")]
    pub notify_weekly_threshold: u32,

    #[serde(default = "default_true")]
    pub launch_at_login: bool,
    #[serde(default)]
    pub show_in_dock: bool,
}

fn default_true() -> bool {
    true
}
fn default_notify_threshold() -> u32 {
    80
}
fn default_refresh_interval() -> RefreshInterval {
    RefreshInterval(300)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_session: true,
            show_weekly: true,
            show_sonnet: false,
            show_extra: false,
            show_number: true,
            color_palette: ColorPalette::Default,
            color_thresholds: ColorThresholds::default(),
            refresh_interval: RefreshInterval(300),
            notify_enabled: false,
            notify_session_threshold: 80,
            notify_weekly_threshold: 80,
            launch_at_login: true,
            show_in_dock: false,
        }
    }
}

fn settings_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = PathBuf::from(home).join(".vibe-usage");
    std::fs::create_dir_all(&dir).ok();
    dir.join("settings.json")
}

fn load() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .unwrap_or_default()
}

fn save(settings: &Settings) {
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        std::fs::write(settings_path(), json).ok();
    }
}

pub fn init() {
    SETTINGS.get_or_init(|| Mutex::new(load()));
}

pub fn get() -> Settings {
    SETTINGS
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}

pub fn update(f: impl FnOnce(&mut Settings)) {
    if let Some(m) = SETTINGS.get() {
        let mut s = m.lock().unwrap();
        f(&mut s);
        save(&s);
    }
}
