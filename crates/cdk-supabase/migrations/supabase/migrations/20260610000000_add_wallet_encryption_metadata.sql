-- Migration 007: Add per-wallet encryption metadata for password-based keys.

CREATE TABLE IF NOT EXISTS wallet_encryption_metadata (
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    version INTEGER NOT NULL,
    kdf TEXT NOT NULL,
    salt TEXT NOT NULL,
    scrypt_log_n INTEGER NOT NULL,
    scrypt_r INTEGER NOT NULL,
    scrypt_p INTEGER NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_id),
    CHECK (version > 0),
    CHECK (length(salt) >= 32),
    CHECK (scrypt_log_n > 0),
    CHECK (scrypt_r > 0),
    CHECK (scrypt_p > 0)
);

ALTER TABLE wallet_encryption_metadata ENABLE ROW LEVEL SECURITY;

CREATE POLICY "Users access own wallet encryption metadata" ON wallet_encryption_metadata
    FOR ALL USING (wallet_id = public.get_current_wallet_id())
    WITH CHECK (wallet_id = public.get_current_wallet_id());

GRANT ALL ON wallet_encryption_metadata TO authenticated;
GRANT ALL ON wallet_encryption_metadata TO service_role;

INSERT INTO schema_info (key, value) VALUES ('schema_version', '7')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW();
