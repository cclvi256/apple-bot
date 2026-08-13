CREATE TABLE group_features (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    self_id TEXT NOT NULL,
    group_id TEXT NOT NULL,
    feature_name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    enabled_by TEXT NOT NULL,
    enabled_at BIGINT NOT NULL,
    manifest_format TEXT NOT NULL DEFAULT 'toml',
    manifest TEXT NOT NULL DEFAULT '',
    CONSTRAINT group_features_identity UNIQUE (self_id, group_id, feature_name)
);
