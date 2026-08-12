pub mod bot;
pub mod command;
pub mod config;
pub mod napcat;
pub mod protocol;
pub mod server;
pub mod store;

use std::sync::Arc;

use bot::Bot;
use config::Config;
use server::AppState;
use store::FeatureStore;
use tracing_subscriber::EnvFilter;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error("invalid listen address: {0}")]
    ListenAddress(#[from] std::net::AddrParseError),
    #[error("failed to bind HTTP listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("HTTP server failed: {0}")]
    Server(#[source] std::io::Error),
}

pub async fn run() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::load_from_environment()?;
    let store = FeatureStore::connect(&config.database_url).await?;
    let enabled = store.load_enabled().await?;
    let bot = Arc::new(Bot::new(&config, store, enabled));
    let state = AppState::new(
        bot,
        config.webhook_token.as_bytes().to_vec(),
        config.max_body_bytes,
    );
    let address: std::net::SocketAddr = config.listen.parse()?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(Error::Bind)?;

    tracing::info!(%address, "cider-bot listening");
    axum::serve(listener, server::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(Error::Server)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
