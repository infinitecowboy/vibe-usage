use image::{Rgba, RgbaImage};
use tray_icon::Icon;

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
            Self::Green => Rgba([52, 199, 89, 255]),  // Apple green
            Self::Yellow => Rgba([255, 204, 0, 255]), // Apple yellow
            Self::Orange => Rgba([255, 149, 0, 255]), // Apple orange
            Self::Red => Rgba([255, 59, 48, 255]),    // Apple red
        }
    }
}

pub fn generate_icon(level: UsageLevel) -> anyhow::Result<Icon> {
    // 44x44 canvas for retina, but smaller circle inside
    let size = 44u32;
    let color = level.color();
    let center = size as f32 / 2.0;

    // Smaller circle - about 50% of canvas size
    let outer_radius = 10.0;
    let border_width = 2.0;
    let inner_radius = outer_radius - border_width;
    let aa_width = 1.0; // Anti-aliasing width

    let border_color = Rgba([255, 255, 255, 230]);

    let mut img = RgbaImage::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();

            if d <= inner_radius - aa_width {
                // Solid inner circle
                img.put_pixel(x, y, color);
            } else if d <= inner_radius {
                // Anti-aliased edge of inner circle
                let alpha = ((inner_radius - d) / aa_width * 255.0).clamp(0.0, 255.0) as u8;
                img.put_pixel(x, y, Rgba([color[0], color[1], color[2], alpha]));
            } else if d <= outer_radius - aa_width {
                // Solid border ring
                img.put_pixel(x, y, border_color);
            } else if d <= outer_radius {
                // Anti-aliased outer edge
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

pub struct IconSet {
    pub green: Icon,
    pub yellow: Icon,
    pub orange: Icon,
    pub red: Icon,
}

impl IconSet {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            green: generate_icon(UsageLevel::Green)?,
            yellow: generate_icon(UsageLevel::Yellow)?,
            orange: generate_icon(UsageLevel::Orange)?,
            red: generate_icon(UsageLevel::Red)?,
        })
    }
    pub fn get(&self, l: UsageLevel) -> &Icon {
        match l {
            UsageLevel::Green => &self.green,
            UsageLevel::Yellow => &self.yellow,
            UsageLevel::Orange => &self.orange,
            UsageLevel::Red => &self.red,
        }
    }
}
