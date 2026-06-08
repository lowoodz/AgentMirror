use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use smr_core::{run_app, SharedApp, DEFAULT_CONFIG_YAML};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "smr", about = "LLM-SafeRoute — lightweight LLM proxy with routing and guardrails", version)]
struct Cli {
    /// Path to YAML config (default: ~/.config/securemodelroute/smr.yaml)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Open management UI in browser on start
    #[arg(long)]
    open: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path = cli
        .config
        .unwrap_or_else(smr_core::paths::default_config_path);

    let (app, path) = SharedApp::load_or_create(&config_path, DEFAULT_CONFIG_YAML)
        .with_context(|| format!("initialize config at {}", config_path.display()))?;

    let listen = app.config().server.listen.clone();
    tracing::info!(config = %path.display(), listen = %listen, "LLM-SafeRoute starting");

    if cli.open {
        let ui = format!("http://{listen}/ui");
        if let Err(e) = open::that(&ui) {
            tracing::warn!(error = %e, "failed to open browser, visit {ui} manually");
        }
    }

    run_with_shutdown(app).await
}

async fn run_with_shutdown(app: Arc<SharedApp>) -> anyhow::Result<()> {
    tokio::select! {
        res = run_app(app) => res,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("LLM-SafeRoute shutting down");
            Ok(())
        }
    }
}
