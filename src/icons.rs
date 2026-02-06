use crate::api::ParsedUsage;
use ab_glyph::{FontRef, PxScale};
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UsageLevel {
    Green,
    Yellow,
    Orange,
    Red,
}

impl UsageLevel {
    pub fn from_percent(p: f32) -> Self {
        if p >= 95.0 {
            Self::Red
        } else if p >= 80.0 {
            Self::Orange
        } else if p >= 50.0 {
            Self::Yellow
        } else {
            Self::Green
        }
    }

    fn color(&self) -> Rgba<u8> {
        match self {
            Self::Green => Rgba([52, 199, 89, 255]),
            Self::Yellow => Rgba([255, 204, 0, 255]),
            Self::Orange => Rgba([255, 149, 0, 255]),
            Self::Red => Rgba([255, 59, 48, 255]),
        }
    }
}

fn gray_color() -> Rgba<u8> {
    Rgba([120, 120, 125, 180])
}

fn text_color() -> Rgba<u8> {
    Rgba([255, 255, 255, 255])
}

const FONT_DATA: &[u8] = include_bytes!("/System/Library/Fonts/Helvetica.ttc");

/// Raw icon data for direct NSImage creation.
pub struct RawIcon {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

const ICON_WIDTH: u32 = 200;
const ICON_HEIGHT: u32 = 44;

/// Generate status bar icon (dot + text + bars) as raw RGBA pixels.
pub fn generate_status_icon_raw(
    level: UsageLevel,
    usage: Option<&ParsedUsage>,
) -> anyhow::Result<RawIcon> {
    let mut img = RgbaImage::new(ICON_WIDTH, ICON_HEIGHT);
    let center_y = ICON_HEIGHT / 2;

    let font =
        FontRef::try_from_slice(FONT_DATA).map_err(|e| anyhow::anyhow!("Font error: {}", e))?;
    let scale = PxScale::from(28.0);
    let text_col = text_color();
    let text_y = 18i32;

    // 1. Draw dot (session indicator)
    let session_level = usage
        .map(|u| UsageLevel::from_percent(u.session_percent))
        .unwrap_or(level);
    let dot_color = session_level.color();
    let dot_cx = 10.0f32;
    let dot_cy = center_y as f32;
    let dot_r = 8.0f32;

    for y in 0..ICON_HEIGHT {
        for x in 0..22 {
            let d = ((x as f32 - dot_cx).powi(2) + (y as f32 - dot_cy).powi(2)).sqrt();
            if d <= dot_r {
                let alpha = if d <= dot_r - 1.5 {
                    255
                } else {
                    ((dot_r - d) / 1.5 * 255.0) as u8
                };
                img.put_pixel(
                    x,
                    y,
                    Rgba([dot_color[0], dot_color[1], dot_color[2], alpha]),
                );
            }
        }
    }

    // 2. Session percentage text
    let session_pct = usage.map(|u| u.session_percent).unwrap_or(0.0);
    let session_text = format!("{:.0}%", session_pct);
    draw_text_mut(&mut img, text_col, 24, text_y, scale, &font, &session_text);

    // 3. Signal bars (weekly indicator)
    let weekly_section_start = 100u32;
    let weekly_level = usage
        .map(|u| UsageLevel::from_percent(u.weekly_percent))
        .unwrap_or(level);
    let bar_color = weekly_level.color();
    let gray = gray_color();

    let weekly_pct = usage.map(|u| u.weekly_percent).unwrap_or(0.0);
    let filled_bars = if weekly_pct >= 75.0 {
        4
    } else if weekly_pct >= 50.0 {
        3
    } else if weekly_pct >= 25.0 {
        2
    } else if weekly_pct > 0.0 {
        1
    } else {
        0
    };

    let bar_width = 10u32;
    let bar_gap = 3u32;
    let bar_heights = [16u32, 24u32, 32u32, 40u32];
    let base_y = 42u32;

    for (i, &bar_h) in bar_heights.iter().enumerate() {
        let bx = weekly_section_start + (i as u32) * (bar_width + bar_gap);
        let by = base_y - bar_h;
        let is_filled = (i as i32) < filled_bars;
        let color = if is_filled { bar_color } else { gray };

        draw_bar(&mut img, bx, by, bar_width, bar_h, color, is_filled);
    }

    // 4. Weekly percentage text
    let weekly_text = format!("{:.0}%", weekly_pct);
    draw_text_mut(&mut img, text_col, 156, text_y, scale, &font, &weekly_text);

    Ok(RawIcon {
        rgba: img.into_raw(),
        width: ICON_WIDTH,
        height: ICON_HEIGHT,
    })
}

fn draw_bar(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: Rgba<u8>, filled: bool) {
    let r = 2.0f32;
    for dy in 0..h {
        for dx in 0..w {
            if !in_rounded_rect(dx as f32, dy as f32, w as f32, h as f32, r) {
                continue;
            }
            let px = x + dx;
            let py = y + dy;
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
