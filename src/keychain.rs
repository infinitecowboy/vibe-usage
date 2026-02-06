use anyhow::{Context, Result};
use std::process::Command;

pub struct AccountInfo {
    pub subscription_type: Option<String>,
}

fn read_credentials_json() -> Result<serde_json::Value> {
    let username = std::env::var("USER").unwrap_or_default();

    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-a",
            &username,
            "-w",
        ])
        .output()
        .context("Failed to run security command")?;

    if !output.status.success() {
        anyhow::bail!("No OAuth token found. Run 'claude' and sign in first.");
    }

    let json_str = String::from_utf8(output.stdout)
        .context("Invalid token encoding")?
        .trim()
        .to_string();

    serde_json::from_str(&json_str).context("Invalid token JSON")
}

pub fn get_oauth_token() -> Result<String> {
    let creds = read_credentials_json()?;
    creds["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(|s| s.to_string())
        .context("No accessToken in credentials")
}

pub fn get_account_info() -> Result<AccountInfo> {
    let creds = read_credentials_json()?;
    let oauth = &creds["claudeAiOauth"];
    Ok(AccountInfo {
        subscription_type: oauth["subscriptionType"].as_str().map(|s| s.to_string()),
    })
}
