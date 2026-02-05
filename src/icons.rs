use crate::api::ParsedUsage;
use image::{Rgba, RgbaImage};
use tray_icon::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconStyle {
    Dot,
    Bars,
}

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

/// Generate a dot icon with the given color level
pub fn generate_dot_icon(level: UsageLevel) -> anyhow::Result<Icon> {
    let size = 44u32;
    let color = level.color();
    let center = size as f32 / 2.0;

    let outer_radius = 10.0;
    let border_width = 2.0;
    let inner_radius = outer_radius - border_width;
    let aa_width = 1.0;

    let border_color = Rgba([255, 255, 255, 230]);

    let mut img = RgbaImage::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();

            if d <= inner_radius - aa_width {
                img.put_pixel(x, y, color);
            } else if d <= inner_radius {
                let alpha = ((inner_radius - d) / aa_width * 255.0).clamp(0.0, 255.0) as u8;
                img.put_pixel(x, y, Rgba([color[0], color[1], color[2], alpha]));
            } else if d <= outer_radius - aa_width {
                img.put_pixel(x, y, border_color);
            } else if d <= outer_radius {
                let alpha = ((outer_radius - d) / aa_width * 230.0).clamp(0.0, 230.0) as u8;
                img.put_pixel(
                    x,
                    y,
                    Rgba([border_color[0], border_color[1], border_color[2], alpha]),
                );
            }
        }
    }
    Icon::from_rgba(img.into_raw(), size, size).map_err(|e| anyhow::anyhow!("{}", e))
}

/// Generate a stacking bars icon showing session and weekly usage
pub fn generate_bars_icon(usage: &ParsedUsage) -> anyhow::Result<Icon> {
    let size = 44u32;
    let mut img = RgbaImage::new(size, size);

    // Bar dimensions
    let bar_width = 8u32;
    let bar_height = 32u32;
    let gap = 4u32;
    let start_y = (size - bar_height) / 2;

    // Two bars: session (left) and weekly (right)
    let bar1_x = (size - 2 * bar_width - gap) / 2;
    let bar2_x = bar1_x + bar_width + gap;

    // Background color (dark gray)
    let bg_color = Rgba([80, 80, 85, 255]);

    // Draw session bar (left)
    let session_level = UsageLevel::from_percent(usage.session_percent);
    let session_color = session_level.color();
    let session_fill = ((usage.session_percent / 100.0) * bar_height as f32).round() as u32;
    draw_bar(
        &mut img,
        bar1_x,
        start_y,
        bar_width,
        bar_height,
        session_fill,
        session_color,
        bg_color,
    );

    // Draw weekly bar (right)
    let weekly_level = UsageLevel::from_percent(usage.weekly_percent);
    let weekly_color = weekly_level.color();
    let weekly_fill = ((usage.weekly_percent / 100.0) * bar_height as f32).round() as u32;
    draw_bar(
        &mut img,
        bar2_x,
        start_y,
        bar_width,
        bar_height,
        weekly_fill,
        weekly_color,
        bg_color,
    );

    Icon::from_rgba(img.into_raw(), size, size).map_err(|e| anyhow::anyhow!("{}", e))
}

fn draw_bar(
    img: &mut RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    fill_height: u32,
    fill_color: Rgba<u8>,
    bg_color: Rgba<u8>,
) {
    let corner_radius = 2.0f32;

    for dy in 0..height {
        for dx in 0..width {
            let px = x + dx;
            let py = y + dy;

            // Check if within rounded corners
            let in_bar = is_in_rounded_rect(
                dx as f32,
                dy as f32,
                width as f32,
                height as f32,
                corner_radius,
            );

            if in_bar {
                // Fill from bottom up
                let fill_start = height.saturating_sub(fill_height);
                if dy >= fill_start {
                    img.put_pixel(px, py, fill_color);
                } else {
                    img.put_pixel(px, py, bg_color);
                }
            }
        }
    }
}

fn is_in_rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    // Check corners
    if x < r && y < r {
        // Top-left corner
        let dx = r - x;
        let dy = r - y;
        return dx * dx + dy * dy <= r * r;
    }
    if x >= w - r && y < r {
        // Top-right corner
        let dx = x - (w - r);
        let dy = r - y;
        return dx * dx + dy * dy <= r * r;
    }
    if x < r && y >= h - r {
        // Bottom-left corner
        let dx = r - x;
        let dy = y - (h - r);
        return dx * dx + dy * dy <= r * r;
    }
    if x >= w - r && y >= h - r {
        // Bottom-right corner
        let dx = x - (w - r);
        let dy = y - (h - r);
        return dx * dx + dy * dy <= r * r;
    }
    true
}

pub struct IconSet {
    pub green: Icon,
    pub yellow: Icon,
    pub orange: Icon,
    pub red: Icon,
}

impl IconSet {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            green: generate_dot_icon(UsageLevel::Green)?,
            yellow: generate_dot_icon(UsageLevel::Yellow)?,
            orange: generate_dot_icon(UsageLevel::Orange)?,
            red: generate_dot_icon(UsageLevel::Red)?,
        })
    }

    pub fn get(&self, level: UsageLevel) -> &Icon {
        match level {
            UsageLevel::Green => &self.green,
            UsageLevel::Yellow => &self.yellow,
            UsageLevel::Orange => &self.orange,
            UsageLevel::Red => &self.red,
        }
    }

    pub fn get_bars(&self, usage: &ParsedUsage) -> Icon {
        generate_bars_icon(usage).unwrap_or_else(|_| self.green.clone())
    }
}
