use crate::api::ParsedUsage;
use crate::settings::{ColorPalette, ColorThresholds};
use image::Rgba;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UsageLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl UsageLevel {
    pub fn from_percent(p: f32, thresholds: &ColorThresholds) -> Self {
        if p >= thresholds.critical as f32 {
            Self::Critical
        } else if p >= thresholds.high as f32 {
            Self::High
        } else if p >= thresholds.warning as f32 {
            Self::Medium
        } else {
            Self::Low
        }
    }

    pub fn color(&self, palette: ColorPalette) -> Rgba<u8> {
        match palette {
            ColorPalette::Default => match self {
                Self::Low => Rgba([52, 199, 89, 255]),      // green
                Self::Medium => Rgba([255, 204, 0, 255]),   // yellow
                Self::High => Rgba([255, 149, 0, 255]),     // orange
                Self::Critical => Rgba([255, 59, 48, 255]), // red
            },
            ColorPalette::Monochrome => match self {
                Self::Low => Rgba([200, 200, 200, 255]),
                Self::Medium => Rgba([160, 160, 160, 255]),
                Self::High => Rgba([120, 120, 120, 255]),
                Self::Critical => Rgba([80, 80, 80, 255]),
            },
        }
    }
}

/// Raw icon data for direct NSImage creation (used by sparkline).
#[derive(Debug, Clone)]
pub struct RawIcon {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Which sections are visible (mirrors settings toggles).
#[derive(Debug, Clone, Copy)]
pub struct SectionVisibility {
    pub session: bool,
    pub weekly: bool,
}

/// Per-section data exposed to the caller for building the pill indicator.
#[derive(Debug, Clone)]
pub struct SectionInfo {
    pub label: &'static str, // "S" or "W"
    pub pct: f32,
    pub level: UsageLevel,
}

/// Result of indicator generation: per-section metadata.
pub struct IndicatorResult {
    pub sections: Vec<SectionInfo>,
}

/// Per-section data for internal rendering.
struct SectionData {
    label: &'static str,
    pct: f32,
    level: UsageLevel,
}

/// Generate per-section metadata for the pill renderer.
pub fn generate_indicator(
    level: UsageLevel,
    usage: Option<&ParsedUsage>,
    thresholds: &ColorThresholds,
    vis: SectionVisibility,
) -> IndicatorResult {
    let sections = build_sections(level, usage, thresholds, vis);

    let section_infos = sections
        .iter()
        .map(|sec| SectionInfo {
            label: sec.label,
            pct: sec.pct,
            level: sec.level,
        })
        .collect();

    IndicatorResult {
        sections: section_infos,
    }
}

/// Build per-section data from usage.
fn build_sections(
    level: UsageLevel,
    usage: Option<&ParsedUsage>,
    thresholds: &ColorThresholds,
    vis: SectionVisibility,
) -> Vec<SectionData> {
    let mut sections = Vec::new();
    if vis.session {
        let pct = usage.map(|u| u.session_percent).unwrap_or(0.0);
        let lvl = usage
            .map(|u| UsageLevel::from_percent(u.session_percent, thresholds))
            .unwrap_or(level);
        sections.push(SectionData {
            label: "S",
            pct,
            level: lvl,
        });
    }
    if vis.weekly {
        let pct = usage.map(|u| u.weekly_percent).unwrap_or(0.0);
        let lvl = usage
            .map(|u| UsageLevel::from_percent(u.weekly_percent, thresholds))
            .unwrap_or(level);
        sections.push(SectionData {
            label: "W",
            pct,
            level: lvl,
        });
    }
    if sections.is_empty() {
        sections.push(SectionData {
            label: "S",
            pct: 0.0,
            level: UsageLevel::Low,
        });
    }
    sections
}
