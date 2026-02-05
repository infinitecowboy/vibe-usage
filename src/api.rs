use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

#[derive(Clone)]
pub struct UsageClient {
    client: Client,
    token: String,
}

#[derive(Debug, Deserialize)]
pub struct UsageResponse {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    pub seven_day_sonnet: Option<UsageWindow>,
    pub extra_usage: Option<ExtraUsage>,
}

#[derive(Debug, Deserialize)]
pub struct UsageWindow {
    #[serde(default)]
    pub utilization: f32,
    pub resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f32>,
    pub used_credits: Option<f32>,
    pub utilization: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUsage {
    pub session_percent: f32,
    pub session_reset: Option<String>,
    pub weekly_percent: f32,
    pub weekly_reset: Option<String>,
    pub sonnet_percent: Option<f32>,
    pub sonnet_reset: Option<String>,
    pub extra_usage_enabled: bool,
    pub extra_usage_percent: Option<f32>,
    pub max_percent: f32,
}

impl UsageClient {
    pub fn new(token: String) -> Result<Self> {
        Ok(Self {
            client: Client::builder().user_agent("claude-code/2.1.31").build()?,
            token,
        })
    }

    pub async fn fetch_usage(&self) -> Result<UsageResponse> {
        let response = self
            .client
            .get("https://api.anthropic.com/api/oauth/usage")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await
            .context("Request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("API error {}: {}", status, body);
        }

        response.json().await.context("Parse failed")
    }
}

impl From<UsageResponse> for ParsedUsage {
    fn from(r: UsageResponse) -> Self {
        let session_percent = r.five_hour.as_ref().map(|w| w.utilization).unwrap_or(0.0);
        let weekly_percent = r.seven_day.as_ref().map(|w| w.utilization).unwrap_or(0.0);
        let sonnet_percent = r.seven_day_sonnet.as_ref().map(|w| w.utilization);

        let extra_usage_enabled = r
            .extra_usage
            .as_ref()
            .map(|e| e.is_enabled)
            .unwrap_or(false);
        let extra_usage_percent = r.extra_usage.as_ref().and_then(|e| e.utilization);

        // Max percent for icon color (only count enabled/active limits)
        let max_percent = session_percent
            .max(weekly_percent)
            .max(sonnet_percent.unwrap_or(0.0));

        Self {
            session_percent,
            session_reset: r.five_hour.and_then(|w| w.resets_at),
            weekly_percent,
            weekly_reset: r.seven_day.and_then(|w| w.resets_at),
            sonnet_percent,
            sonnet_reset: r.seven_day_sonnet.and_then(|w| w.resets_at),
            extra_usage_enabled,
            extra_usage_percent,
            max_percent,
        }
    }
}
