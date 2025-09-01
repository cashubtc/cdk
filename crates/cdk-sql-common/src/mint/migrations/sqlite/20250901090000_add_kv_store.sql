-- Add kv_store table for generic key-value storage
CREATE TABLE IF NOT EXISTS kv_store (
    primary_namespace TEXT NOT NULL,
    secondary_namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value BLOB NOT NULL,
    created_time INTEGER NOT NULL,
    updated_time INTEGER NOT NULL,
    PRIMARY KEY (primary_namespace, secondary_namespace, key)
);

-- Index for efficient listing of keys by namespace
CREATE INDEX IF NOT EXISTS idx_kv_store_namespaces 
ON kv_store (primary_namespace, secondary_namespace);

-- Index for efficient querying by update time
CREATE INDEX IF NOT EXISTS idx_kv_store_updated_time 
ON kv_store (updated_time);
