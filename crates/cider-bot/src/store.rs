use std::{collections::HashMap, time::SystemTime};

use sqlx::{AnyPool, Row, any::AnyPoolOptions, migrate::Migrator};

static SQLITE_MIGRATOR: Migrator = sqlx::migrate!("./migrations/sqlite");
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("./migrations/postgres");

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FeatureKey {
    pub self_id: String,
    pub group_id: String,
    pub feature_name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FeatureManifest {
    values: toml::Table,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestParseError {
    #[error("failed to parse TOML manifest: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("nested manifest key is not supported: {0}")]
    NestedKey(String),
}

impl FeatureManifest {
    pub fn from_toml(value: &str) -> Result<Self, ManifestParseError> {
        let values: toml::Table = toml::from_str(value)?;
        if let Some(key) = values
            .iter()
            .find_map(|(key, value)| value.is_table().then(|| key.clone()))
        {
            return Err(ManifestParseError::NestedKey(key));
        }
        Ok(Self { values })
    }

    pub fn values(&self) -> &toml::Table {
        &self.values
    }

    pub fn get(&self, key: &str) -> Option<&toml::Value> {
        self.values.get(key)
    }

    pub fn set_string(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values
            .insert(key.into(), toml::Value::String(value.into()));
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(&self.values)
    }

    pub fn selected_toml(&self, key: &str) -> Result<Option<String>, toml::ser::Error> {
        let Some(value) = self.get(key) else {
            return Ok(None);
        };
        let mut selected = toml::Table::new();
        selected.insert(key.into(), value.clone());
        toml::to_string(&selected).map(Some)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureRecord {
    pub id: i64,
    pub key: FeatureKey,
    pub enabled: bool,
    pub enabled_by: String,
    pub enabled_at: i64,
    pub manifest: FeatureManifest,
}

#[derive(Clone)]
pub struct FeatureStore {
    pool: AnyPool,
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

impl FeatureStore {
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await?;
        let migrator = if url.starts_with("sqlite:") {
            &SQLITE_MIGRATOR
        } else if url.starts_with("postgres:") || url.starts_with("postgresql:") {
            &POSTGRES_MIGRATOR
        } else {
            return Err(StoreError::UnsupportedDatabase);
        };
        migrator.run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn load_all(&self) -> Result<HashMap<FeatureKey, FeatureRecord>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, self_id, group_id, feature_name, \
             CAST(CASE WHEN enabled THEN 1 ELSE 0 END AS BIGINT) AS enabled_value, \
             enabled_by, enabled_at, manifest FROM group_features",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(decode_record)
            .map(|record| (record.key.clone(), record))
            .collect())
    }

    pub async fn enable(
        &self,
        key: &FeatureKey,
        enabled_by: &str,
    ) -> Result<FeatureRecord, StoreError> {
        let enabled_at = unix_timestamp()?;
        sqlx::query(
            "INSERT INTO group_features \
             (self_id, group_id, feature_name, enabled, enabled_by, enabled_at) \
             VALUES (?, ?, ?, TRUE, ?, ?) \
             ON CONFLICT (self_id, group_id, feature_name) DO UPDATE SET \
             enabled = TRUE, enabled_by = excluded.enabled_by, enabled_at = excluded.enabled_at",
        )
        .bind(&key.self_id)
        .bind(&key.group_id)
        .bind(&key.feature_name)
        .bind(enabled_by)
        .bind(enabled_at)
        .execute(&self.pool)
        .await?;
        self.get(key).await?.ok_or(sqlx::Error::RowNotFound.into())
    }

    pub async fn disable(&self, key: &FeatureKey) -> Result<Option<FeatureRecord>, StoreError> {
        let result = sqlx::query(
            "UPDATE group_features SET enabled = FALSE \
             WHERE self_id = ? AND group_id = ? AND feature_name = ? AND enabled = TRUE",
        )
        .bind(&key.self_id)
        .bind(&key.group_id)
        .bind(&key.feature_name)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            Ok(None)
        } else {
            self.get(key).await
        }
    }

    pub async fn update_manifest(
        &self,
        key: &FeatureKey,
        manifest: &FeatureManifest,
    ) -> Result<Option<FeatureRecord>, StoreError> {
        let manifest = manifest.to_toml()?;
        let result = sqlx::query(
            "UPDATE group_features SET manifest = ? \
             WHERE self_id = ? AND group_id = ? AND feature_name = ?",
        )
        .bind(manifest)
        .bind(&key.self_id)
        .bind(&key.group_id)
        .bind(&key.feature_name)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            Ok(None)
        } else {
            self.get(key).await
        }
    }

    pub async fn ready(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.pool).await.is_ok()
    }

    async fn get(&self, key: &FeatureKey) -> Result<Option<FeatureRecord>, StoreError> {
        sqlx::query(
            "SELECT id, self_id, group_id, feature_name, \
             CAST(CASE WHEN enabled THEN 1 ELSE 0 END AS BIGINT) AS enabled_value, \
             enabled_by, enabled_at, manifest FROM group_features \
             WHERE self_id = ? AND group_id = ? AND feature_name = ?",
        )
        .bind(&key.self_id)
        .bind(&key.group_id)
        .bind(&key.feature_name)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(decode_record))
        .map_err(StoreError::Database)
    }
}

fn decode_record(row: sqlx::any::AnyRow) -> FeatureRecord {
    let manifest_source: String = row.get("manifest");
    let manifest = FeatureManifest::from_toml(&manifest_source).unwrap_or_else(|error| {
        tracing::warn!(%error, "invalid stored TOML feature manifest; using empty manifest");
        FeatureManifest::default()
    });
    FeatureRecord {
        id: row.get("id"),
        key: FeatureKey {
            self_id: row.get("self_id"),
            group_id: row.get("group_id"),
            feature_name: row.get("feature_name"),
        },
        enabled: row.get::<i64, _>("enabled_value") != 0,
        enabled_by: row.get("enabled_by"),
        enabled_at: row.get("enabled_at"),
        manifest,
    }
}

fn unix_timestamp() -> Result<i64, StoreError> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .map_err(|_| StoreError::Clock)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    async fn assert_feature_store_contract(store: FeatureStore) {
        let first = FeatureKey {
            self_id: "10".into(),
            group_id: "20".into(),
            feature_name: "dice".into(),
        };
        let second = FeatureKey {
            self_id: "10".into(),
            group_id: "21".into(),
            feature_name: "dice".into(),
        };

        sqlx::query(
            "DELETE FROM group_features WHERE self_id = '10' AND group_id IN ('20', '21') \
             AND feature_name = 'dice'",
        )
        .execute(&store.pool)
        .await
        .unwrap();

        let initial = store.enable(&first, "30").await.unwrap();
        let other = store.enable(&second, "30").await.unwrap();
        assert!(initial.id > 0);
        assert!(other.id > initial.id);
        assert!(initial.enabled);
        assert!(initial.manifest.values().is_empty());

        let manifest = FeatureManifest::from_toml("answer = 42\n").unwrap();
        let updated = store
            .update_manifest(&first, &manifest)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.id, initial.id);
        assert_eq!(updated.manifest, manifest);

        let disabled = store.disable(&first).await.unwrap().unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.id, initial.id);
        assert_eq!(disabled.manifest, manifest);

        let reenabled = store.enable(&first, "31").await.unwrap();
        assert!(reenabled.enabled);
        assert_eq!(reenabled.id, initial.id);
        assert_eq!(reenabled.manifest, manifest);
        assert_eq!(reenabled.enabled_by, "31");

        let records = store.load_all().await.unwrap();
        assert_eq!(records[&first], reenabled);
    }

    #[tokio::test]
    async fn sqlite_feature_store_contract() {
        let directory = tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("test.sqlite").display()
        );
        let store = FeatureStore::connect(&url).await.unwrap();
        let columns = sqlx::query("PRAGMA table_info(group_features)")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        let primary_keys = columns
            .into_iter()
            .filter(|row| row.get::<i64, _>("pk") != 0)
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
        assert_eq!(primary_keys, ["id"]);
        let schema: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'group_features'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert!(schema.contains("INTEGER PRIMARY KEY AUTOINCREMENT"));
        assert!(!schema.contains("manifest_format"));
        assert_feature_store_contract(store).await;
    }

    #[test]
    fn manifest_is_flat_and_can_serialize_one_selected_key() {
        let manifest = FeatureManifest::from_toml(
            "session_mode = \"strict\"\nretries = 3\noptions = [\"a\", \"b\"]\n",
        )
        .unwrap();
        assert_eq!(
            manifest.selected_toml("session_mode").unwrap().as_deref(),
            Some("session_mode = \"strict\"\n")
        );
        assert_eq!(manifest.selected_toml("missing").unwrap(), None);
        assert!(matches!(
            FeatureManifest::from_toml("[session]\nstrict = true\n"),
            Err(ManifestParseError::NestedKey(key)) if key == "session"
        ));
    }

    #[tokio::test]
    async fn invalid_manifests_fall_back_without_overwrite() {
        let directory = tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("fallback.sqlite").display()
        );
        let store = FeatureStore::connect(&url).await.unwrap();
        sqlx::query(
            "INSERT INTO group_features \
             (self_id, group_id, feature_name, enabled, enabled_by, enabled_at, manifest) \
             VALUES ('1', '2', 'bad-toml', TRUE, '3', 0, 'broken = [')",
        )
        .execute(&store.pool)
        .await
        .unwrap();

        let records = store.load_all().await.unwrap();
        let bad_toml = &records[&FeatureKey {
            self_id: "1".into(),
            group_id: "2".into(),
            feature_name: "bad-toml".into(),
        }];
        assert!(bad_toml.manifest.values().is_empty());
    }

    #[tokio::test]
    #[ignore = "requires TEST_POSTGRES_URL"]
    async fn postgres_feature_store_contract() {
        let url = std::env::var("TEST_POSTGRES_URL").expect("set TEST_POSTGRES_URL");
        let store = FeatureStore::connect(&url).await.unwrap();
        assert_feature_store_contract(store).await;
    }
}
