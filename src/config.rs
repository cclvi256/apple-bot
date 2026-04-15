use serde::Deserialize;
use std::{fs, io, path::Path};

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Toml(toml::de::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => err.fmt(f),
            Self::Toml(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Toml(err) => Some(err),
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(value: toml::de::Error) -> Self {
        Self::Toml(value)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PublicConfig {
    pub bot: BotConfig,
    pub napcat: NapcatPublicConfig,
    #[serde(default)]
    pub http: HttpConfig,
    pub database: DatabasePublicConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecretConfig {
    pub napcat: NapcatSecretConfig,
    #[serde(default)]
    pub database: DatabaseSecretConfig,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bot: BotConfig,
    pub napcat: NapcatConfig,
    pub http: HttpConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotConfig {
    pub qq: u64,
    pub owner_qq: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NapcatPublicConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NapcatSecretConfig {
    pub client_token: String,
    pub server_token: String,
}

#[derive(Debug, Clone)]
pub struct NapcatConfig {
    pub host: String,
    pub port: u16,
    pub client_token: String,
    pub server_token: String,
}

impl NapcatConfig {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpConfig {
    #[serde(default = "default_http_host")]
    pub host: String,
    #[serde(default = "default_http_port")]
    pub port: u16,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: default_http_host(),
            port: default_http_port(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabasePublicConfig {
    pub kind: String,
    pub url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub name: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DatabaseSecretConfig {
    pub password: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub kind: String,
    pub url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub name: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl PublicConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(input)?)
    }

    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        Self::from_toml_str(&content)
    }

    pub fn merge(self, secrets: SecretConfig) -> AppConfig {
        AppConfig {
            bot: self.bot,
            napcat: NapcatConfig {
                host: self.napcat.host,
                port: self.napcat.port,
                client_token: secrets.napcat.client_token,
                server_token: secrets.napcat.server_token,
            },
            http: self.http,
            database: DatabaseConfig {
                kind: self.database.kind,
                url: self.database.url,
                host: self.database.host,
                port: self.database.port,
                name: self.database.name,
                username: self.database.username,
                password: secrets.database.password,
            },
        }
    }
}

impl SecretConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(input)?)
    }

    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        Self::from_toml_str(&content)
    }
}

impl AppConfig {
    pub fn from_toml_files(
        public_path: impl AsRef<Path>,
        secret_path: impl AsRef<Path>,
    ) -> Result<Self, ConfigError> {
        let public = PublicConfig::from_toml_file(public_path)?;
        let secrets = SecretConfig::from_toml_file(secret_path)?;
        Ok(public.merge(secrets))
    }
}

const fn default_http_port() -> u16 {
    8888
}

fn default_http_host() -> String {
    "0.0.0.0".to_string()
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, PublicConfig, SecretConfig};

    #[test]
    fn merge_public_and_secret_config() {
        let public = PublicConfig::from_toml_str(
            r#"
            [bot]
            qq = 123456
            owner_qq = 654321

            [napcat]
            host = "127.0.0.1"
            port = 3000

            [http]
            port = 9999

            [database]
            kind = "postgres"
            url = "postgres://db.internal/apple"
            username = "apple_bot"
            "#,
        )
        .unwrap();

        let secrets = SecretConfig::from_toml_str(
            r#"
            [napcat]
            client_token = "client-token"
            server_token = "server-token"

            [database]
            password = "secret"
            "#,
        )
        .unwrap();

        let config = public.merge(secrets);
        assert_eq!(config.bot.qq, 123456);
        assert_eq!(config.napcat.base_url(), "http://127.0.0.1:3000");
        assert_eq!(config.napcat.client_token, "client-token");
        assert_eq!(config.http.port, 9999);
        assert_eq!(config.database.password.as_deref(), Some("secret"));
    }

    #[test]
    fn http_defaults_to_port_8888() {
        let public = PublicConfig::from_toml_str(
            r#"
            [bot]
            qq = 123456

            [napcat]
            host = "localhost"
            port = 3000

            [database]
            kind = "sqlite"
            "#,
        )
        .unwrap();

        let secrets = SecretConfig::from_toml_str(
            r#"
            [napcat]
            client_token = "client-token"
            server_token = "server-token"
            "#,
        )
        .unwrap();

        let config = public.merge(secrets);
        assert_eq!(config.http.host, "0.0.0.0");
        assert_eq!(config.http.port, 8888);
    }

    #[test]
    fn load_full_app_config_from_strings() {
        let public = PublicConfig::from_toml_str(
            r#"
            [bot]
            qq = 123456

            [napcat]
            host = "napcat"
            port = 3000

            [database]
            kind = "postgres"
            host = "db"
            port = 5432
            name = "apple_bot"
            username = "apple_bot"
            "#,
        )
        .unwrap();

        let secrets = SecretConfig::from_toml_str(
            r#"
            [napcat]
            client_token = "client-token"
            server_token = "server-token"

            [database]
            password = "secret"
            "#,
        )
        .unwrap();

        let config = public.merge(secrets);
        assert_eq!(config.database.port, Some(5432));
        assert_eq!(config.database.name.as_deref(), Some("apple_bot"));
        assert_eq!(config.database.password.as_deref(), Some("secret"));
        assert_eq!(config.napcat.server_token, "server-token");
        let _ = AppConfig {
            bot: config.bot,
            napcat: config.napcat,
            http: config.http,
            database: config.database,
        };
    }
}
