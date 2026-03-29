-- CDK Wallet Database Schema for Supabase (tables, indexes, triggers, RLS)
-- Functions are defined in separate files (20241001000001-20241001000004) to
-- work around the Supavisor prepared-statement restriction (SQLSTATE 42601).

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ============================================================================
-- CORE TABLES
-- ============================================================================

CREATE TABLE IF NOT EXISTS kv_store (
    primary_namespace TEXT NOT NULL,
    secondary_namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    PRIMARY KEY (primary_namespace, secondary_namespace, key, wallet_id)
);

CREATE TABLE IF NOT EXISTS mint (
    mint_url TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
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
    tos_url TEXT,
    opt_version INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (mint_url, wallet_id)
);

CREATE TABLE IF NOT EXISTS keyset (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    mint_url TEXT NOT NULL,
    unit TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT false,
    input_fee_ppk BIGINT NOT NULL,
    final_expiry BIGINT,
    keyset_u32 BIGINT,
    opt_version INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (id, wallet_id)
);

CREATE INDEX IF NOT EXISTS idx_keyset_mint_url ON keyset(mint_url);
CREATE INDEX IF NOT EXISTS idx_keyset_active ON keyset(active);
CREATE INDEX IF NOT EXISTS idx_keyset_wallet_id ON keyset(wallet_id);

CREATE TABLE IF NOT EXISTS key (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    keys TEXT NOT NULL,
    keyset_u32 BIGINT,
    PRIMARY KEY (id, wallet_id)
);

CREATE TABLE IF NOT EXISTS keyset_counter (
    keyset_id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    counter INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (keyset_id, wallet_id)
);

CREATE TABLE IF NOT EXISTS mint_quote (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
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
    PRIMARY KEY (id, wallet_id)
);

CREATE INDEX IF NOT EXISTS idx_mint_quote_mint_url ON mint_quote(mint_url);
CREATE INDEX IF NOT EXISTS idx_mint_quote_state ON mint_quote(state);
CREATE INDEX IF NOT EXISTS idx_mint_quote_amount_issued ON mint_quote(amount_issued);
CREATE INDEX IF NOT EXISTS idx_mint_quote_wallet_id ON mint_quote(wallet_id);

CREATE TABLE IF NOT EXISTS melt_quote (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    unit TEXT NOT NULL,
    amount BIGINT NOT NULL,
    request TEXT NOT NULL,
    fee_reserve BIGINT NOT NULL,
    state TEXT NOT NULL,
    expiry BIGINT NOT NULL,
    payment_preimage TEXT,
    payment_method TEXT NOT NULL,
    PRIMARY KEY (id, wallet_id)
);

CREATE INDEX IF NOT EXISTS idx_melt_quote_state ON melt_quote(state);
CREATE INDEX IF NOT EXISTS idx_melt_quote_wallet_id ON melt_quote(wallet_id);

CREATE TABLE IF NOT EXISTS proof (
    y TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
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
    opt_version INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (y, wallet_id)
);

CREATE INDEX IF NOT EXISTS idx_proof_mint_url ON proof(mint_url);
CREATE INDEX IF NOT EXISTS idx_proof_state ON proof(state);
CREATE INDEX IF NOT EXISTS idx_proof_unit ON proof(unit);
CREATE INDEX IF NOT EXISTS idx_proof_keyset_id ON proof(keyset_id);
CREATE INDEX IF NOT EXISTS idx_proof_wallet_id ON proof(wallet_id);

CREATE TABLE IF NOT EXISTS transactions (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    mint_url TEXT NOT NULL,
    direction TEXT NOT NULL,
    unit TEXT NOT NULL,
    amount BIGINT NOT NULL,
    fee BIGINT NOT NULL DEFAULT 0,
    ys TEXT[],
    timestamp BIGINT NOT NULL,
    memo TEXT,
    metadata TEXT,
    quote_id TEXT,
    payment_request TEXT,
    payment_proof TEXT,
    payment_method TEXT,
    status TEXT NOT NULL DEFAULT 'completed',
    opt_version INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (id, wallet_id)
);

CREATE INDEX IF NOT EXISTS idx_transactions_mint_url ON transactions(mint_url);
CREATE INDEX IF NOT EXISTS idx_transactions_direction ON transactions(direction);
CREATE INDEX IF NOT EXISTS idx_transactions_unit ON transactions(unit);
CREATE INDEX IF NOT EXISTS idx_transactions_timestamp ON transactions(timestamp);
CREATE INDEX IF NOT EXISTS idx_transactions_wallet_id ON transactions(wallet_id);

-- ============================================================================
-- ROW LEVEL SECURITY
-- ============================================================================

ALTER TABLE kv_store ENABLE ROW LEVEL SECURITY;
ALTER TABLE mint ENABLE ROW LEVEL SECURITY;
ALTER TABLE keyset ENABLE ROW LEVEL SECURITY;
ALTER TABLE key ENABLE ROW LEVEL SECURITY;
ALTER TABLE keyset_counter ENABLE ROW LEVEL SECURITY;
ALTER TABLE mint_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE melt_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE proof ENABLE ROW LEVEL SECURITY;
ALTER TABLE transactions ENABLE ROW LEVEL SECURITY;

CREATE POLICY "Users access own kv_store" ON kv_store
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

CREATE POLICY "Users access own mints" ON mint
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

CREATE POLICY "Users access own keysets" ON keyset
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

CREATE POLICY "Users access own keys" ON key
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

CREATE POLICY "Users access own counters" ON keyset_counter
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

CREATE POLICY "Users access own mint quotes" ON mint_quote
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

CREATE POLICY "Users access own melt quotes" ON melt_quote
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

CREATE POLICY "Users access own proofs" ON proof
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

CREATE POLICY "Users access own transactions" ON transactions
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

GRANT ALL ON ALL TABLES IN SCHEMA public TO authenticated;
GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO authenticated;
