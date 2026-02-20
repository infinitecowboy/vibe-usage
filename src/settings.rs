use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static SETTINGS: OnceLock<Mutex<Settings>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconType {
    Pill,
    // Legacy variants kept for serde deserialization of old settings
    #[serde(other)]
    _Legacy,
}

impl Default for IconType {
    fn default() -> Self {
        Self::Pill
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorPalette {
    Default,
    Monochrome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorThresholds {
    pub warning: u32,  // yellow/medium threshold
    pub high: u32,     // orange/high threshold
    pub critical: u32, // red/critical threshold
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

    pub fn label(&self) -> &'static str {
        Self::OPTIONS
            .iter()
            .find(|(v, _)| *v == self.0)
            .map(|(_, l)| *l)
            .unwrap_or("5 min")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_icon_type")]
    pub icon_type: IconType,
    #[serde(default = "default_true")]
    pub show_icon: bool,
    #[serde(default = "default_true")]
    pub show_number: bool,
    pub show_session: bool,
    pub show_weekly: bool,
    pub show_sonnet: bool,
    pub show_extra: bool,
    #[serde(default = "default_notify_enabled")]
    pub notify_enabled: bool,
    #[serde(default = "default_notify_threshold")]
    pub notify_session_threshold: u32,
    #[serde(default = "default_notify_threshold")]
    pub notify_weekly_threshold: u32,
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: RefreshInterval,
    #[serde(default)]
    pub launch_at_login: bool,
    #[serde(default = "default_color_palette")]
    pub color_palette: ColorPalette,
    #[serde(default)]
    pub color_thresholds: ColorThresholds,
    #[serde(default = "default_true")]
    pub icons_colored: bool,
    #[serde(default)]
    pub neutral_text: bool,
    #[serde(default)]
    pub pill_outline: bool,
    #[serde(default)]
    pub show_in_dock: bool,
}

fn default_icon_type() -> IconType {
    IconType::Pill
}
fn default_notify_enabled() -> bool {
    false
}
fn default_notify_threshold() -> u32 {
    80
}
fn default_refresh_interval() -> RefreshInterval {
    RefreshInterval(300)
}
fn default_color_palette() -> ColorPalette {
    ColorPalette::Default
}
fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            icon_type: IconType::Pill,
            show_icon: true,
            show_number: true,
            show_session: true,
            show_weekly: true,
            show_sonnet: false,
            show_extra: false,
            notify_enabled: false,
            notify_session_threshold: 80,
            notify_weekly_threshold: 80,
            refresh_interval: RefreshInterval(120),
            launch_at_login: true,
            color_palette: ColorPalette::Default,
            color_thresholds: ColorThresholds {
                warning: 30,
                high: 60,
                critical: 90,
            },
            icons_colored: true,
            neutral_text: true,
            pill_outline: false,
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

pub fn load() -> Settings {
    let path = settings_path();
    let json_str = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Settings::default(),
    };

    // Try parsing directly with new format
    if let Ok(s) = serde_json::from_str::<Settings>(&json_str) {
        return s;
    }

    // Migration: detect old fields and convert
    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&json_str) {
        let mut changed = false;
        if let Some(old_style) = val
            .get("menubar_style")
            .and_then(|v| v.as_str())
            .map(String::from)
        {
            let (icon_type, show_icon, show_num) = match old_style.as_str() {
                "detailed" => ("pill", true, true),
                "compact" => ("pill", true, false),
                "text_only" => ("pill", false, true),
                "icon_only" => ("pill", true, false),
                _ => ("pill", true, true),
            };
            if let Some(obj) = val.as_object_mut() {
                obj.remove("menubar_style");
                obj.insert(
                    "icon_type".into(),
                    serde_json::Value::String(icon_type.into()),
                );
                obj.insert("show_icon".into(), serde_json::Value::Bool(show_icon));
                obj.insert("show_number".into(), serde_json::Value::Bool(show_num));
            }
            changed = true;
        }
        // Migrate old show_percent -> show_number
        if let Some(obj) = val.as_object_mut() {
            if let Some(sp) = obj.remove("show_percent") {
                obj.insert("show_number".into(), sp);
                changed = true;
            }
        }
        // Remove old fields that no longer exist
        if let Some(obj) = val.as_object_mut() {
            for key in &["hotkey_enabled", "chart_style", "show_label"] {
                if obj.remove(*key).is_some() {
                    changed = true;
                }
            }
            // Migrate any old icon_type -> "pill"
            if let Some(it) = obj.get("icon_type").and_then(|v| v.as_str()).map(String::from) {
                if it != "pill" {
                    obj.insert("icon_type".into(), serde_json::Value::String("pill".into()));
                    changed = true;
                }
            }
            // Migrate colorblind palette -> default
            if obj.get("color_palette").and_then(|v| v.as_str()) == Some("colorblind") {
                obj.insert(
                    "color_palette".into(),
                    serde_json::Value::String("default".into()),
                );
                changed = true;
            }
        }
        if changed {
            if let Ok(migrated) = serde_json::from_value::<Settings>(val) {
                save(&migrated);
                return migrated;
            }
        }
    }

    Settings::default()
}

fn save(settings: &Settings) {
    let path = settings_path();
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        std::fs::write(&path, json).ok();
    }
}

/// Initialize the global settings. Call once at startup.
pub fn init() {
    SETTINGS.get_or_init(|| Mutex::new(load()));
}

/// Get a clone of the current settings.
pub fn get() -> Settings {
    SETTINGS
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}

/// Update settings with a closure, then persist to disk.
pub fn update(f: impl FnOnce(&mut Settings)) {
    if let Some(m) = SETTINGS.get() {
        let mut s = m.lock().unwrap();
        f(&mut s);
        save(&s);
    }
}
