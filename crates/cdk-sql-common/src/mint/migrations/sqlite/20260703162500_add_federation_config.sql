CREATE TABLE IF NOT EXISTS federation_config (
    federation_id TEXT PRIMARY KEY NOT NULL,
    config_digest TEXT NOT NULL,
    config_json TEXT NOT NULL
);
