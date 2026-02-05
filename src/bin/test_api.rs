use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Test keychain
    println!("=== Testing Keychain ===");
    let username = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    println!("Username: {}", username);

    let bytes =
        security_framework::passwords::get_generic_password("Claude Code-credentials", &username)?;
    println!("Got {} bytes from keychain", bytes.len());

    let json_str = String::from_utf8(bytes.to_vec())?;
    let creds: serde_json::Value = serde_json::from_str(&json_str)?;

    println!("Top-level keys:");
    if let Some(obj) = creds.as_object() {
        for key in obj.keys() {
            println!("  - {}", key);
        }
    }

    // Check nested structure
    if let Some(oauth) = creds.get("claudeAiOauth") {
        println!("claudeAiOauth keys:");
        if let Some(obj) = oauth.as_object() {
            for key in obj.keys() {
                println!("  - {}", key);
            }
        }
    }

    let token = creds["claudeAiOauth"]["accessToken"]
        .as_str()
        .or_else(|| creds["claudeAiOauth"]["access_token"].as_str())
        .or_else(|| creds["accessToken"].as_str())
        .or_else(|| creds["access_token"].as_str())
        .ok_or_else(|| anyhow::anyhow!("No token field found"))?;
    println!("Token length: {} chars", token.len());

    // Test API
    println!("\n=== Testing API ===");
    let client = reqwest::Client::builder()
        .user_agent("claude-code/2.1.31")
        .build()?;

    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {}", token))
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await?;

    println!("Status: {}", resp.status());
    let body = resp.text().await?;
    println!("Response: {}", body);

    Ok(())
}
