CREATE TABLE IF NOT EXISTS group_features (
    self_id TEXT NOT NULL,
    group_id TEXT NOT NULL,
    feature_name TEXT NOT NULL,
    enabled_by TEXT NOT NULL,
    enabled_at BIGINT NOT NULL,
    PRIMARY KEY (self_id, group_id, feature_name)
);
