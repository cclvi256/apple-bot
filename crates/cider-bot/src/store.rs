use std::{collections::HashSet, time::SystemTime};

use sqlx::{AnyPool, any::AnyPoolOptions};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FeatureKey {
    pub self_id: String,
    pub group_id: String,
    pub feature_name: String,
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
    #[error("system clock is before the Unix epoch")]
    Clock,
}

impl FeatureStore {
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn load_enabled(&self) -> Result<HashSet<FeatureKey>, StoreError> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            "SELECT self_id, group_id, feature_name FROM group_features",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(self_id, group_id, feature_name)| FeatureKey {
                self_id,
                group_id,
                feature_name,
            })
            .collect())
    }

    pub async fn enable(&self, key: &FeatureKey, enabled_by: &str) -> Result<bool, StoreError> {
        let enabled_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| StoreError::Clock)?
            .as_secs() as i64;
        let result = sqlx::query(
            "INSERT INTO group_features \
             (self_id, group_id, feature_name, enabled_by, enabled_at) \
             VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(&key.self_id)
        .bind(&key.group_id)
        .bind(&key.feature_name)
        .bind(enabled_by)
        .bind(enabled_at)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn disable(&self, key: &FeatureKey) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "DELETE FROM group_features \
             WHERE self_id = ? AND group_id = ? AND feature_name = ?",
        )
        .bind(&key.self_id)
        .bind(&key.group_id)
        .bind(&key.feature_name)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn ready(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.pool).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    async fn assert_feature_switch_round_trip(store: FeatureStore) {
        let key = FeatureKey {
            self_id: "10".into(),
            group_id: "20".into(),
            feature_name: "dice".into(),
        };

        store.disable(&key).await.unwrap();
        assert!(store.enable(&key, "30").await.unwrap());
        assert!(!store.enable(&key, "30").await.unwrap());
        assert!(store.load_enabled().await.unwrap().contains(&key));
        assert!(store.disable(&key).await.unwrap());
        assert!(!store.load_enabled().await.unwrap().contains(&key));
    }

    #[tokio::test]
    async fn sqlite_feature_switch_round_trip() {
        let directory = tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("test.sqlite").display()
        );
        let store = FeatureStore::connect(&url).await.unwrap();
        assert_feature_switch_round_trip(store).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_POSTGRES_URL"]
    async fn postgres_feature_switch_round_trip() {
        let url = std::env::var("TEST_POSTGRES_URL").expect("set TEST_POSTGRES_URL");
        let store = FeatureStore::connect(&url).await.unwrap();
        assert_feature_switch_round_trip(store).await;
    }
}
