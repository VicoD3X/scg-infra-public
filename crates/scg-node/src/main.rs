#![forbid(unsafe_code)]

mod config;

use std::{env, error::Error, path::Path, sync::Arc};

use config::NodeConfig;
use scg_core::{Runtime, RuntimeConfig};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("scg_node=info,scg_api=info,tower_http=info")),
        )
        .compact()
        .init();

    let config_path = env::args().nth(1);
    let config = NodeConfig::load(config_path.as_deref().map(Path::new))?;
    let mut runtime_config = RuntimeConfig::new(config.realm.clone(), &config.database_path);
    runtime_config.event_retention = config.event_retention;
    runtime_config.snapshot_retention = config.snapshot_retention;

    let runtime = Arc::new(Runtime::open(runtime_config)?);
    let snapshot = runtime.start()?;
    let listener = TcpListener::bind(config.bind).await?;
    info!(
        bind = %config.bind,
        realm = %config.realm,
        revision = snapshot.revision,
        "SCG node is ready"
    );

    let server = axum::serve(listener, scg_api::router(Arc::clone(&runtime)))
        .with_graceful_shutdown(shutdown_signal())
        .await;

    if let Err(error) = runtime.stop() {
        error!(%error, "runtime shutdown checkpoint failed");
    }
    server?;
    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!(%error, "failed to install shutdown signal handler");
    }
}
