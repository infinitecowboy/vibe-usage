use crate::api::ParsedUsage;
use crate::settings::{ColorPalette, ColorThresholds, IconType};
use image::{Rgba, RgbaImage};

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

fn gray_color() -> Rgba<u8> {
    Rgba([120, 120, 125, 180])
}

/// Whether the icon should be set as a template image (adapts to light/dark).
/// When icons_colored is true, we have mixed colors so template mode won't work.
pub fn is_template(palette: ColorPalette, icons_colored: bool) -> bool {
    matches!(palette, ColorPalette::Monochrome) && !icons_colored
}

const ICON_H: u32 = 44;

/// Raw icon data for direct NSImage creation.
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

/// Per-section data exposed to the caller for building native text.
#[derive(Debug, Clone)]
pub struct SectionInfo {
    pub label: &'static str, // "S" or "W"
    pub pct: f32,
    pub level: UsageLevel,
    pub icon: RawIcon,
}

/// Result of indicator generation: per-section icons + metadata.
pub struct IndicatorResult {
    pub sections: Vec<SectionInfo>,
}

// ── Composable indicator generator ───────────────────────────────────

/// Per-section data for internal rendering.
struct SectionData {
    label: &'static str,
    pct: f32,
    level: UsageLevel,
}

/// Generate per-section indicator images (no text). Text is rendered natively
/// by the caller via NSAttributedString with inline image attachments.
pub fn generate_indicator_image(
    level: UsageLevel,
    usage: Option<&ParsedUsage>,
    icon_type: IconType,
    palette: ColorPalette,
    thresholds: &ColorThresholds,
    vis: SectionVisibility,
    icons_colored: bool,
) -> anyhow::Result<IndicatorResult> {
    let graphic_palette = if icons_colored {
        ColorPalette::Default
    } else {
        palette
    };

    let sections = build_sections(level, usage, thresholds, vis);

    let section_infos = sections
        .iter()
        .map(|sec| {
            let icon = render_section_icon(icon_type, sec, graphic_palette);
            SectionInfo {
                label: sec.label,
                pct: sec.pct,
                level: sec.level,
                icon,
            }
        })
        .collect();

    Ok(IndicatorResult {
        sections: section_infos,
    })
}

/// Render a single section's indicator graphic as a standalone image.
fn render_section_icon(icon_type: IconType, sec: &SectionData, palette: ColorPalette) -> RawIcon {
    let w = section_icon_width(icon_type, sec);
    let mut img = RgbaImage::new(w, ICON_H);
    draw_section_icon(&mut img, icon_type, sec, 0, palette);
    RawIcon {
        rgba: img.into_raw(),
        width: w,
        height: ICON_H,
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

/// Width of a single section's icon graphic.
fn section_icon_width(icon_type: IconType, _sec: &SectionData) -> u32 {
    match icon_type {
        IconType::Dot => 18,
        IconType::SignalBars => 30,
        IconType::MiniBars => 14,
        IconType::DotGrid => 40, // 4 dots only (no label text)
    }
}

/// Draw a single section's icon at the given x position.
fn draw_section_icon(
    img: &mut RgbaImage,
    icon_type: IconType,
    sec: &SectionData,
    x: u32,
    palette: ColorPalette,
) {
    match icon_type {
        IconType::Dot => {
            let color = sec.level.color(palette);
            let cx = x as f32 + 9.0;
            let cy = img.height() as f32 / 2.0;
            draw_filled_circle(img, cx, cy, 8.0, color);
        }
        IconType::SignalBars => {
            draw_signal_bars_at(img, sec, x, palette);
        }
        IconType::MiniBars => {
            draw_mini_bar_single(img, sec, x, palette);
        }
        IconType::DotGrid => {
            draw_dot_row(img, sec, x, palette);
        }
    }
}

// ── Per-section drawing helpers ──────────────────────────────────────

/// Draw signal bars for a single section at the given x offset.
fn draw_signal_bars_at(
    img: &mut RgbaImage,
    sec: &SectionData,
    start_x: u32,
    palette: ColorPalette,
) {
    let bar_color = sec.level.color(palette);
    let gray = gray_color();

    let filled_bars = if sec.pct >= 75.0 {
        4
    } else if sec.pct >= 50.0 {
        3
    } else if sec.pct >= 25.0 {
        2
    } else if sec.pct > 0.0 {
        1
    } else {
        0
    };

    let bar_width = 6u32;
    let bar_gap = 2u32;
    let bar_heights = [12u32, 20u32, 28u32, 38u32];
    let base_y = 42u32;

    for (i, &bar_h) in bar_heights.iter().enumerate() {
        let bx = start_x + (i as u32) * (bar_width + bar_gap);
        let by = base_y - bar_h;
        let is_filled = (i as i32) < filled_bars;
        let color = if is_filled { bar_color } else { gray };
        draw_rounded_bar(img, bx, by, bar_width, bar_h, color, is_filled);
    }
}

/// Draw a single vertical fill bar for a section at the given x offset.
fn draw_mini_bar_single(
    img: &mut RgbaImage,
    sec: &SectionData,
    start_x: u32,
    palette: ColorPalette,
) {
    let track = Rgba([120, 120, 125, 60]);
    let bar_w = 8u32;
    let bar_h = 38u32;
    let top_y = 3u32;
    let r = 3.0;

    let bx = start_x + 3;
    draw_filled_rounded_rect(img, bx, top_y, bar_w, bar_h, r, track);
    if sec.pct > 0.5 {
        let fill_h = ((sec.pct / 100.0).min(1.0) * bar_h as f32) as u32;
        if fill_h > 0 {
            let fill_y = top_y + bar_h - fill_h;
            draw_filled_rounded_rect(
                img,
                bx,
                fill_y,
                bar_w,
                fill_h,
                r.min(fill_h as f32 / 2.0),
                sec.level.color(palette),
            );
        }
    }
}

/// Draw a row of 4 dots for a section at the given x offset (no text label).
fn draw_dot_row(img: &mut RgbaImage, sec: &SectionData, start_x: u32, palette: ColorPalette) {
    let track = Rgba([120, 120, 125, 60]);
    let dot_r = 5.0f32;
    let cols = 4i32;
    let gap_x = 10.0f32;

    let row_y = img.height() as f32 / 2.0;
    let filled = ((sec.pct / 25.0).ceil() as i32).min(4).max(0);

    for col in 0..cols {
        let cx = start_x as f32 + col as f32 * gap_x;
        let color = if col < filled {
            sec.level.color(palette)
        } else {
            track
        };
        draw_filled_circle(img, cx, row_y, dot_r, color);
    }
}

// ── Primitive drawing ────────────────────────────────────────────────

fn draw_rounded_bar(
    img: &mut RgbaImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: Rgba<u8>,
    filled: bool,
) {
    let r = 2.0f32;
    for dy in 0..h {
        for dx in 0..w {
            if !in_rounded_rect(dx as f32, dy as f32, w as f32, h as f32, r) {
                continue;
            }
            let px = x + dx;
            let py = y + dy;
            if px >= img.width() || py >= img.height() {
                continue;
            }
            if filled {
                img.put_pixel(px, py, color);
            } else {
                let border = dx <= 1 || dx >= w - 2 || dy <= 1 || dy >= h - 2;
                if border {
                    img.put_pixel(px, py, color);
                }
            }
        }
    }
}

fn in_rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    if x < r && y < r {
        return (r - x).powi(2) + (r - y).powi(2) <= r * r;
    }
    if x >= w - r && y < r {
        return (x - (w - r)).powi(2) + (r - y).powi(2) <= r * r;
    }
    if x < r && y >= h - r {
        return (r - x).powi(2) + (y - (h - r)).powi(2) <= r * r;
    }
    if x >= w - r && y >= h - r {
        return (x - (w - r)).powi(2) + (y - (h - r)).powi(2) <= r * r;
    }
    true
}

fn alpha_blend(dst: Rgba<u8>, src: Rgba<u8>) -> Rgba<u8> {
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

fn draw_filled_circle(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, color: Rgba<u8>) {
    let x0 = (cx - r - 2.0).max(0.0) as u32;
    let x1 = ((cx + r + 2.0) as u32).min(img.width());
    let y0 = (cy - r - 2.0).max(0.0) as u32;
    let y1 = ((cy + r + 2.0) as u32).min(img.height());

    for py in y0..y1 {
        for px in x0..x1 {
            let dist = ((px as f32 - cx).powi(2) + (py as f32 - cy).powi(2)).sqrt();
            if dist <= r + 0.5 {
                let a = if dist <= r - 0.5 {
                    color[3]
                } else {
                    ((r + 0.5 - dist) * color[3] as f32) as u8
                };
                let src = Rgba([color[0], color[1], color[2], a]);
                let dst = *img.get_pixel(px, py);
                img.put_pixel(px, py, alpha_blend(dst, src));
            }
        }
    }
}

fn draw_filled_rounded_rect(
    img: &mut RgbaImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    r: f32,
    color: Rgba<u8>,
) {
    for dy in 0..h {
        for dx in 0..w {
            if !in_rounded_rect(dx as f32, dy as f32, w as f32, h as f32, r) {
                continue;
            }
            let px = x + dx;
            let py = y + dy;
            if px < img.width() && py < img.height() {
                let dst = *img.get_pixel(px, py);
                img.put_pixel(px, py, alpha_blend(dst, color));
            }
        }
    }
}
