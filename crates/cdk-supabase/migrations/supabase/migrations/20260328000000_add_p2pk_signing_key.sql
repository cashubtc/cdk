-- Migration 005: Add p2pk_signing_key table
-- Stores P2PK signing keys for the wallet

CREATE TABLE IF NOT EXISTS p2pk_signing_key (
    pubkey TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    derivation_index INTEGER NOT NULL,
    derivation_path TEXT NOT NULL,
    created_time BIGINT NOT NULL,
    PRIMARY KEY (pubkey, wallet_id)
);

CREATE INDEX IF NOT EXISTS idx_p2pk_signing_key_wallet_id ON p2pk_signing_key(wallet_id);
CREATE INDEX IF NOT EXISTS idx_p2pk_signing_key_derivation_index ON p2pk_signing_key(derivation_index);

ALTER TABLE p2pk_signing_key ENABLE ROW LEVEL SECURITY;

CREATE POLICY "Users access own p2pk keys" ON p2pk_signing_key
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

GRANT ALL ON p2pk_signing_key TO authenticated;
GRANT ALL ON p2pk_signing_key TO service_role;

-- Bump schema version
INSERT INTO schema_info (key, value) VALUES ('schema_version', '5')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
