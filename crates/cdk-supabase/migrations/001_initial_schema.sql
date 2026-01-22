-- CDK Wallet Database Schema for Supabase
-- This schema creates the necessary tables for the CDK wallet to work with Supabase

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- Migrations Table
-- Tracks applied migrations
CREATE TABLE IF NOT EXISTS migrations (
    name TEXT PRIMARY KEY,
    applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- RPC Function: exec_sql
-- Allows executing raw SQL via the REST API
-- This is used for automated migrations
-- SECURITY DEFINER allows it to run with elevated privileges
CREATE OR REPLACE FUNCTION exec_sql(query TEXT)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
BEGIN
    EXECUTE query;
END;
$$;

-- Grant execute permission to authenticated and service_role
GRANT EXECUTE ON FUNCTION exec_sql(TEXT) TO authenticated;
GRANT EXECUTE ON FUNCTION exec_sql(TEXT) TO service_role;

-- KV Store Table
-- Stores generic key-value pairs with namespace isolation
CREATE TABLE IF NOT EXISTS kv_store (
    primary_namespace TEXT NOT NULL,
    secondary_namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL, -- hex encoded bytea
    PRIMARY KEY (primary_namespace, secondary_namespace, key)
);

-- Mint Table
-- Stores mint information
CREATE TABLE IF NOT EXISTS mint (
    mint_url TEXT PRIMARY KEY,
    name TEXT,
    pubkey TEXT,
    version TEXT,
    description TEXT,
    description_long TEXT,
    contact TEXT,
    nuts TEXT,
    icon_url TEXT,
    urls TEXT,
    motd TEXT,
    mint_time BIGINT,
    tos_url TEXT
);

-- Keyset Table
-- Stores keyset metadata
CREATE TABLE IF NOT EXISTS keyset (
    mint_url TEXT NOT NULL,
    id TEXT NOT NULL,
    unit TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT false,
    input_fee_ppk BIGINT NOT NULL,
    final_expiry BIGINT,
    keyset_u32 BIGINT,
    PRIMARY KEY (id),
    FOREIGN KEY (mint_url) REFERENCES mint(mint_url) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_keyset_mint_url ON keyset(mint_url);
CREATE INDEX IF NOT EXISTS idx_keyset_active ON keyset(active);

-- Key Table
-- Stores keyset keys (public keys)
CREATE TABLE IF NOT EXISTS key (
    id TEXT PRIMARY KEY,
    keys TEXT NOT NULL, -- json string of keys
    keyset_u32 BIGINT,
    FOREIGN KEY (id) REFERENCES keyset(id) ON DELETE CASCADE
);

-- Mint Quote Table
-- Stores mint quotes
CREATE TABLE IF NOT EXISTS mint_quote (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    amount BIGINT NOT NULL,
    unit TEXT NOT NULL,
    request TEXT,
    state TEXT NOT NULL,
    expiry BIGINT NOT NULL,
    secret_key TEXT,
    payment_method TEXT NOT NULL,
    amount_issued BIGINT NOT NULL DEFAULT 0,
    amount_paid BIGINT NOT NULL DEFAULT 0,
    FOREIGN KEY (mint_url) REFERENCES mint(mint_url) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_mint_quote_mint_url ON mint_quote(mint_url);
CREATE INDEX IF NOT EXISTS idx_mint_quote_state ON mint_quote(state);
CREATE INDEX IF NOT EXISTS idx_mint_quote_amount_issued ON mint_quote(amount_issued);

-- Melt Quote Table
-- Stores melt quotes
CREATE TABLE IF NOT EXISTS melt_quote (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount BIGINT NOT NULL,
    request TEXT NOT NULL,
    fee_reserve BIGINT NOT NULL,
    state TEXT NOT NULL,
    expiry BIGINT NOT NULL,
    payment_preimage TEXT,
    payment_method TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_melt_quote_state ON melt_quote(state);

-- Proof Table
-- Stores proofs
CREATE TABLE IF NOT EXISTS proof (
    y TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    state TEXT NOT NULL,
    spending_condition TEXT,
    unit TEXT NOT NULL,
    amount BIGINT NOT NULL,
    keyset_id TEXT NOT NULL,
    secret TEXT NOT NULL,
    c TEXT NOT NULL,
    witness TEXT,
    dleq_e TEXT,
    dleq_s TEXT,
    dleq_r TEXT,
    FOREIGN KEY (mint_url) REFERENCES mint(mint_url) ON DELETE CASCADE,
    FOREIGN KEY (keyset_id) REFERENCES keyset(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_proof_mint_url ON proof(mint_url);
CREATE INDEX IF NOT EXISTS idx_proof_state ON proof(state);
CREATE INDEX IF NOT EXISTS idx_proof_unit ON proof(unit);
CREATE INDEX IF NOT EXISTS idx_proof_keyset_id ON proof(keyset_id);

-- Keyset Counter Table
-- Stores keyset counters for proof generation
CREATE TABLE IF NOT EXISTS keyset_counter (
    keyset_id TEXT PRIMARY KEY,
    counter INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (keyset_id) REFERENCES keyset(id) ON DELETE CASCADE
);

-- Transactions Table
-- Stores wallet transactions
CREATE TABLE IF NOT EXISTS transactions (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    direction TEXT NOT NULL,
    unit TEXT NOT NULL,
    amount BIGINT NOT NULL,
    fee BIGINT NOT NULL DEFAULT 0,
    ys TEXT[], -- array of Y values (hex encoded)
    timestamp BIGINT NOT NULL,
    memo TEXT,
    metadata TEXT, -- json string
    quote_id TEXT,
    payment_request TEXT,
    payment_proof TEXT,
    payment_method TEXT,
    FOREIGN KEY (mint_url) REFERENCES mint(mint_url) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_transactions_mint_url ON transactions(mint_url);
CREATE INDEX IF NOT EXISTS idx_transactions_direction ON transactions(direction);
CREATE INDEX IF NOT EXISTS idx_transactions_unit ON transactions(unit);
CREATE INDEX IF NOT EXISTS idx_transactions_timestamp ON transactions(timestamp);

-- Enable Row Level Security (RLS) for multi-tenancy
-- Note: You should configure RLS policies based on your authentication setup

ALTER TABLE kv_store ENABLE ROW LEVEL SECURITY;
ALTER TABLE mint ENABLE ROW LEVEL SECURITY;
ALTER TABLE keyset ENABLE ROW LEVEL SECURITY;
ALTER TABLE key ENABLE ROW LEVEL SECURITY;
ALTER TABLE mint_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE melt_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE proof ENABLE ROW LEVEL SECURITY;
ALTER TABLE keyset_counter ENABLE ROW LEVEL SECURITY;
ALTER TABLE transactions ENABLE ROW LEVEL SECURITY;

-- Example RLS policies (adjust based on your auth setup)
-- These policies allow all operations for authenticated users
-- You may want to make these more restrictive based on user_id

CREATE POLICY "Enable all for authenticated users" ON kv_store
    FOR ALL
    USING (auth.role() = 'authenticated');

CREATE POLICY "Enable all for authenticated users" ON mint
    FOR ALL
    USING (auth.role() = 'authenticated');

CREATE POLICY "Enable all for authenticated users" ON keyset
    FOR ALL
    USING (auth.role() = 'authenticated');

CREATE POLICY "Enable all for authenticated users" ON key
    FOR ALL
    USING (auth.role() = 'authenticated');

CREATE POLICY "Enable all for authenticated users" ON mint_quote
    FOR ALL
    USING (auth.role() = 'authenticated');

CREATE POLICY "Enable all for authenticated users" ON melt_quote
    FOR ALL
    USING (auth.role() = 'authenticated');

CREATE POLICY "Enable all for authenticated users" ON proof
    FOR ALL
    USING (auth.role() = 'authenticated');

CREATE POLICY "Enable all for authenticated users" ON keyset_counter
    FOR ALL
    USING (auth.role() = 'authenticated');

CREATE POLICY "Enable all for authenticated users" ON transactions
    FOR ALL
    USING (auth.role() = 'authenticated');

-- Grant permissions to authenticated users
GRANT ALL ON ALL TABLES IN SCHEMA public TO authenticated;
GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO authenticated;
