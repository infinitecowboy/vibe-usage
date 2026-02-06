mod api;
mod icons;
mod keychain;
mod menubar;

use anyhow::Result;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("vibe_usage=debug,warn")
        .init();

    tracing::info!("Starting vibe-usage");

    let app = menubar::MenubarApp::new().map_err(|e| {
        tracing::error!("Failed to create app: {:?}", e);
        e
    })?;

    tracing::info!("App created, starting event loop");
    app.run()
}
