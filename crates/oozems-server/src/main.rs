#![forbid(unsafe_code)]

mod api;
mod app;
mod config;
mod content;
mod database;

use anyhow::Context;
use config::Config;
use content::ContentCatalog;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::from_env()?;
    std::fs::create_dir_all(&config.data_dir).with_context(|| {
        format!(
            "failed to create data directory {}",
            config.data_dir.display()
        )
    })?;

    let catalog =
        ContentCatalog::load_with_wz(&config.content_dir, &config.asset_dir, &config.wz_dir)?;
    let database = database::open_surreal_kv(&config.data_dir.join("surrealkv")).await?;
    let router = app::router(database, catalog, &config.public_dir, &config.asset_dir);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;

    info!(address = %config.bind, "oozems server ready");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("oozems_server=info,tower_http=info"));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal handler");
    }
}
