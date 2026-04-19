use crate::settings::ColorThresholds;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UsageLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl UsageLevel {
    pub fn from_percent(p: f32, t: &ColorThresholds) -> Self {
        if p >= t.critical as f32 {
            Self::Critical
        } else if p >= t.high as f32 {
            Self::High
        } else if p >= t.warning as f32 {
            Self::Medium
        } else {
            Self::Low
        }
    }
}
