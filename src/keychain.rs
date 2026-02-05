use anyhow::{Context, Result};
use std::process::Command;

pub fn get_oauth_token() -> Result<String> {
    // Use security CLI to avoid repeated keychain prompts
    // (The security CLI uses existing keychain access grants)
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

    let creds: serde_json::Value = serde_json::from_str(&json_str).context("Invalid token JSON")?;

    creds["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(|s| s.to_string())
        .context("No accessToken in credentials")
}
