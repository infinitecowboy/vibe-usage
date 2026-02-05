use image::{Rgba, RgbaImage};
use tray_icon::Icon;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UsageLevel { Green, Yellow, Orange, Red }

impl UsageLevel {
    pub fn from_percent(p: f32) -> Self {
        if p >= 95.0 { Self::Red } else if p >= 80.0 { Self::Orange } else if p >= 50.0 { Self::Yellow } else { Self::Green }
    }
    fn color(&self) -> Rgba<u8> {
        match self {
            Self::Green => Rgba([139, 148, 186, 255]),
            Self::Yellow => Rgba([169, 147, 227, 255]),
            Self::Orange => Rgba([255, 170, 100, 255]),
            Self::Red => Rgba([255, 107, 107, 255]),
        }
    }
}

pub fn generate_icon(level: UsageLevel) -> anyhow::Result<Icon> {
    let (size, color) = (22u32, level.color());
    let (center, radius) = (size as f32 / 2.0, size as f32 / 2.0 - 2.0);
    let mut img = RgbaImage::new(size, size);
    for y in 0..size { for x in 0..size {
        let d = (((x as f32 - center).powi(2) + (y as f32 - center).powi(2))).sqrt();
        if d <= radius { img.put_pixel(x, y, Rgba([color[0], color[1], color[2], if d > radius - 1.0 { ((radius - d) * 255.0) as u8 } else { 255 }])); }
    }}
    Icon::from_rgba(img.into_raw(), size, size).map_err(|e| anyhow::anyhow!("{}", e))
}

pub struct IconSet { pub green: Icon, pub yellow: Icon, pub orange: Icon, pub red: Icon }
impl IconSet {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self { green: generate_icon(UsageLevel::Green)?, yellow: generate_icon(UsageLevel::Yellow)?, orange: generate_icon(UsageLevel::Orange)?, red: generate_icon(UsageLevel::Red)? })
    }
    pub fn get(&self, l: UsageLevel) -> &Icon { match l { UsageLevel::Green => &self.green, UsageLevel::Yellow => &self.yellow, UsageLevel::Orange => &self.orange, UsageLevel::Red => &self.red } }
}
