use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("phosyncra=info")),
        )
        .init();

    info!("Phosyncra daemon starting");
    info!("No providers or lighting backends configured yet");

    tokio::signal::ctrl_c().await?;
    info!("Phosyncra daemon stopping");
    Ok(())
}
