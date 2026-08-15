use reqwest::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid listen address: {0}")]
    ListenAddress(#[from] std::net::AddrParseError),
    #[error("failed to bind HTTP listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("HTTP server failed: {0}")]
    Server(#[source] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read configuration file {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse TOML file {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("configuration value {0} must not be empty")]
    Empty(&'static str),
    #[error("bot.owners contains an invalid QQ number: {0}")]
    InvalidOwner(String),
    #[error("database.url must start with sqlite: or postgres:/postgresql:")]
    UnsupportedDatabase,
    #[error("napcat.base_url must start with http:// or https://")]
    InvalidNapcatUrl,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestParseError {
    #[error("failed to parse TOML manifest: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("nested manifest key is not supported: {0}")]
    NestedKey(String),
}

#[derive(Debug, thiserror::Error)]
pub enum NapcatError {
    #[error("failed to construct NapCat HTTP client: {0}")]
    Build(#[source] reqwest::Error),
    #[error("NapCat request failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("NapCat returned HTTP {0}")]
    Http(StatusCode),
    #[error("NapCat rejected the operation with retcode {retcode}: {message}")]
    Rejected { retcode: i64, message: String },
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("failed to serialize feature manifest: {0}")]
    ManifestSerialization(#[from] toml::ser::Error),
    #[error("system clock is before the Unix epoch")]
    Clock,
    #[error("unsupported database URL")]
    UnsupportedDatabase,
}
