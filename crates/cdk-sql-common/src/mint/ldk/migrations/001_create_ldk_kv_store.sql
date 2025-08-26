-- Create table for LDK KV store data
CREATE TABLE IF NOT EXISTS ldk_kv_store (
    primary_namespace   TEXT            NOT NULL,
    secondary_namespace TEXT            NOT NULL,
    key                 TEXT            NOT NULL,
    value               BYTEA,
    PRIMARY KEY (primary_namespace, secondary_namespace, key)
);

-- Create index for faster lookups by namespace
CREATE INDEX IF NOT EXISTS idx_ldk_kv_store_primary_namespace
ON ldk_kv_store(primary_namespace);

CREATE INDEX IF NOT EXISTS idx_ldk_kv_store_secondary_namespace
ON ldk_kv_store(primary_namespace, secondary_namespace);
