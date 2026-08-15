use std::{collections::HashSet, env, fs, path::Path};

use crate::error::ConfigError;
use serde::Deserialize;

const DEFAULT_CONFIG_PATH: &str = "/etc/cider-bot/config.toml";
const DEFAULT_SECRET_PATH: &str = "/etc/cider-bot/secret.toml";

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: String,
    pub max_body_bytes: usize,
    pub database_url: String,
    pub owners: HashSet<String>,
    pub napcat_base_url: String,
    pub napcat_timeout_ms: u64,
    pub webhook_token: String,
    pub api_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicConfig {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    database: DatabaseConfig,
    bot: BotConfig,
    napcat: NapcatConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretConfig {
    #[serde(default)]
    database: SecretDatabaseConfig,
    napcat: NapcatSecretConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerConfig {
    #[serde(default = "default_listen")]
    listen: String,
    #[serde(default = "default_max_body_bytes")]
    max_body_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            max_body_bytes: default_max_body_bytes(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseConfig {
    #[serde(default = "default_database_url")]
    url: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: default_database_url(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretDatabaseConfig {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BotConfig {
    owners: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NapcatConfig {
    base_url: String,
    #[serde(default = "default_napcat_timeout_ms")]
    timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NapcatSecretConfig {
    webhook_token: String,
    api_token: String,
}

fn default_listen() -> String {
    "0.0.0.0:8080".into()
}

const fn default_max_body_bytes() -> usize {
    1024 * 1024
}

fn default_database_url() -> String {
    "sqlite:///data/cider-bot.sqlite?mode=rwc".into()
}

const fn default_napcat_timeout_ms() -> u64 {
    3_000
}

impl Config {
    pub fn load_from_environment() -> Result<Self, ConfigError> {
        let config_path =
            env::var("CIDER_CONFIG_FILE").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.into());
        let secret_path =
            env::var("CIDER_SECRET_FILE").unwrap_or_else(|_| DEFAULT_SECRET_PATH.into());
        Self::load(config_path, secret_path)
    }

    pub fn load(
        config_path: impl AsRef<Path>,
        secret_path: impl AsRef<Path>,
    ) -> Result<Self, ConfigError> {
        let public: PublicConfig = read_toml(config_path.as_ref())?;
        let secret: SecretConfig = read_toml(secret_path.as_ref())?;
        let database_url = secret.database.url.unwrap_or(public.database.url);
        let owners = public
            .bot
            .owners
            .into_iter()
            .map(|owner| {
                if is_qq_number(&owner) {
                    Ok(owner)
                } else {
                    Err(ConfigError::InvalidOwner(owner))
                }
            })
            .collect::<Result<HashSet<_>, _>>()?;

        require_nonempty("server.listen", &public.server.listen)?;
        require_nonempty("database.url", &database_url)?;
        require_nonempty("napcat.base_url", &public.napcat.base_url)?;
        require_nonempty("napcat.webhook_token", &secret.napcat.webhook_token)?;
        require_nonempty("napcat.api_token", &secret.napcat.api_token)?;

        if !matches_database_scheme(&database_url) {
            return Err(ConfigError::UnsupportedDatabase);
        }
        if !(public.napcat.base_url.starts_with("http://")
            || public.napcat.base_url.starts_with("https://"))
        {
            return Err(ConfigError::InvalidNapcatUrl);
        }

        Ok(Self {
            listen: public.server.listen,
            max_body_bytes: public.server.max_body_bytes,
            database_url,
            owners,
            napcat_base_url: public.napcat.base_url.trim_end_matches('/').into(),
            napcat_timeout_ms: public.napcat.timeout_ms,
            webhook_token: secret.napcat.webhook_token,
            api_token: secret.napcat.api_token,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(database_url: String) -> Self {
        Self {
            listen: "127.0.0.1:0".into(),
            max_body_bytes: default_max_body_bytes(),
            database_url,
            owners: HashSet::from(["10000".into()]),
            napcat_base_url: "http://napcat:3000".into(),
            napcat_timeout_ms: 100,
            webhook_token: "webhook-secret".into(),
            api_token: "api-secret".into(),
        }
    }
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ConfigError> {
    let value = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.display().to_string(),
        source,
    })?;
    toml::from_str(&value).map_err(|source| ConfigError::Parse {
        path: path.display().to_string(),
        source,
    })
}

fn require_nonempty(name: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        Err(ConfigError::Empty(name))
    } else {
        Ok(())
    }
}

fn is_qq_number(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn matches_database_scheme(value: &str) -> bool {
    value.starts_with("sqlite:")
        || value.starts_with("postgres:")
        || value.starts_with("postgresql:")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn secret_database_url_overrides_sqlite_default() {
        let directory = tempdir().unwrap();
        let config = directory.path().join("config.toml");
        let secret = directory.path().join("secret.toml");
        fs::write(
            &config,
            r#"
                [bot]
                owners = ["10000"]
                [napcat]
                base_url = "http://napcat:3000"
            "#,
        )
        .unwrap();
        fs::write(
            &secret,
            r#"
                [database]
                url = "postgres://bot:pass@postgres/bot"
                [napcat]
                webhook_token = "event"
                api_token = "api"
            "#,
        )
        .unwrap();

        let loaded = Config::load(config, secret).unwrap();
        assert_eq!(loaded.database_url, "postgres://bot:pass@postgres/bot");
    }
}
