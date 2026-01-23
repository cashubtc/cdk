-- CDK Wallet Database Schema for Supabase
-- This schema creates the necessary tables for the CDK wallet to work with Supabase
-- Includes multi-tenant support with wallet_id and atomic RPC functions

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ============================================================================
-- HELPER FUNCTIONS
-- ============================================================================

-- Helper Function: Extract Wallet ID from JWT (defaults to auth.uid())
CREATE OR REPLACE FUNCTION public.get_current_wallet_id()
RETURNS text AS $$
  SELECT COALESCE(
    nullif(current_setting('request.jwt.claims', true)::json->>'sub', ''),
    auth.uid()::text
  );
$$ LANGUAGE sql STABLE SECURITY DEFINER;

-- Trigger function for optimistic concurrency versioning
CREATE OR REPLACE FUNCTION increment_opt_version()
RETURNS TRIGGER AS $$
BEGIN
  IF (TG_OP = 'UPDATE') THEN
      NEW.opt_version = COALESCE(OLD.opt_version, 0) + 1;
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- MIGRATIONS TABLE
-- ============================================================================

-- Migrations Table - Tracks applied migrations
CREATE TABLE IF NOT EXISTS migrations (
    name TEXT PRIMARY KEY,
    applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- ============================================================================
-- RPC FUNCTIONS
-- ============================================================================

-- RPC Function: exec_sql
-- Allows executing raw SQL via the REST API (for automated migrations)
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

GRANT EXECUTE ON FUNCTION exec_sql(TEXT) TO authenticated;
GRANT EXECUTE ON FUNCTION exec_sql(TEXT) TO service_role;

-- ============================================================================
-- CORE TABLES
-- ============================================================================
-- Note: Foreign keys are intentionally omitted because wallet data can arrive
-- out of order (e.g., receiving proofs/keys before registering the mint/keyset).
-- RLS policies provide the necessary data isolation per wallet_id.

-- KV Store Table - Stores generic key-value pairs with namespace isolation
CREATE TABLE IF NOT EXISTS kv_store (
    primary_namespace TEXT NOT NULL,
    secondary_namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL, -- hex encoded bytea (encrypted)
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    PRIMARY KEY (primary_namespace, secondary_namespace, key, wallet_id)
);

-- Mint Table - Stores mint information
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

CREATE TRIGGER increment_mint_opt_version 
    BEFORE UPDATE ON mint 
    FOR EACH ROW EXECUTE FUNCTION increment_opt_version();

-- Keyset Table - Stores keyset metadata
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

-- Key Table - Stores keyset keys (public keys)
CREATE TABLE IF NOT EXISTS key (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    keys TEXT NOT NULL, -- json string of keys
    keyset_u32 BIGINT,
    PRIMARY KEY (id, wallet_id)
);

-- Keyset Counter Table - Stores keyset counters for proof generation
CREATE TABLE IF NOT EXISTS keyset_counter (
    keyset_id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    counter INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (keyset_id, wallet_id)
);

-- Mint Quote Table - Stores mint quotes
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

-- Melt Quote Table - Stores melt quotes
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

-- Proof Table - Stores proofs (encrypted secrets)
CREATE TABLE IF NOT EXISTS proof (
    y TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    mint_url TEXT NOT NULL,
    state TEXT NOT NULL,
    spending_condition TEXT,
    unit TEXT NOT NULL,
    amount BIGINT NOT NULL,
    keyset_id TEXT NOT NULL,
    secret TEXT NOT NULL, -- encrypted
    c TEXT NOT NULL, -- encrypted
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

CREATE TRIGGER increment_proof_opt_version 
    BEFORE UPDATE ON proof 
    FOR EACH ROW EXECUTE FUNCTION increment_opt_version();

-- Transactions Table - Stores wallet transactions
CREATE TABLE IF NOT EXISTS transactions (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
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
    status TEXT NOT NULL DEFAULT 'completed',
    opt_version INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (id, wallet_id)
);

CREATE INDEX IF NOT EXISTS idx_transactions_mint_url ON transactions(mint_url);
CREATE INDEX IF NOT EXISTS idx_transactions_direction ON transactions(direction);
CREATE INDEX IF NOT EXISTS idx_transactions_unit ON transactions(unit);
CREATE INDEX IF NOT EXISTS idx_transactions_timestamp ON transactions(timestamp);
CREATE INDEX IF NOT EXISTS idx_transactions_wallet_id ON transactions(wallet_id);

CREATE TRIGGER increment_transactions_opt_version 
    BEFORE UPDATE ON transactions 
    FOR EACH ROW EXECUTE FUNCTION increment_opt_version();

-- ============================================================================
-- ATOMIC RPC FUNCTIONS (with wallet_id support)
-- ============================================================================

-- Function: increment_keyset_counter
-- Atomically increments the keyset counter and returns the new value
CREATE OR REPLACE FUNCTION increment_keyset_counter(
    p_keyset_id TEXT,
    p_increment INTEGER DEFAULT 1
)
RETURNS INTEGER
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
    new_counter INTEGER;
    v_wallet_id TEXT;
BEGIN
    v_wallet_id := public.get_current_wallet_id();
    
    INSERT INTO keyset_counter (keyset_id, wallet_id, counter)
    VALUES (p_keyset_id, v_wallet_id, p_increment)
    ON CONFLICT (keyset_id, wallet_id)
    DO UPDATE SET counter = keyset_counter.counter + p_increment
    RETURNING counter INTO new_counter;
    
    RETURN new_counter;
END;
$$;

GRANT EXECUTE ON FUNCTION increment_keyset_counter(TEXT, INTEGER) TO authenticated;
GRANT EXECUTE ON FUNCTION increment_keyset_counter(TEXT, INTEGER) TO service_role;

COMMENT ON FUNCTION increment_keyset_counter(TEXT, INTEGER) IS 
    'Atomically increments the keyset counter by the specified amount and returns the new value. '
    'Creates the counter with the increment value if it does not exist. '
    'Automatically scoped to the current wallet_id.';

-- Function: update_proofs_atomic
-- Atomically adds new proofs and removes proofs by Y values in a single transaction
CREATE OR REPLACE FUNCTION update_proofs_atomic(
    p_proofs_to_add JSONB DEFAULT '[]'::JSONB,
    p_ys_to_remove JSONB DEFAULT '[]'::JSONB
)
RETURNS JSONB
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
    added_count INTEGER := 0;
    removed_count INTEGER := 0;
    proof_record JSONB;
    v_wallet_id TEXT;
BEGIN
    v_wallet_id := public.get_current_wallet_id();
    
    -- Remove proofs by Y values first (to handle any conflicts)
    IF jsonb_array_length(p_ys_to_remove) > 0 THEN
        DELETE FROM proof
        WHERE y = ANY(SELECT jsonb_array_elements_text(p_ys_to_remove))
          AND wallet_id = v_wallet_id;
        
        GET DIAGNOSTICS removed_count = ROW_COUNT;
    END IF;
    
    -- Add/upsert new proofs
    IF jsonb_array_length(p_proofs_to_add) > 0 THEN
        FOR proof_record IN SELECT * FROM jsonb_array_elements(p_proofs_to_add)
        LOOP
            INSERT INTO proof (
                y, wallet_id, mint_url, state, spending_condition, unit, amount,
                keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r
            )
            VALUES (
                proof_record->>'y',
                v_wallet_id,
                proof_record->>'mint_url',
                proof_record->>'state',
                proof_record->>'spending_condition',
                proof_record->>'unit',
                (proof_record->>'amount')::BIGINT,
                proof_record->>'keyset_id',
                proof_record->>'secret',
                proof_record->>'c',
                proof_record->>'witness',
                proof_record->>'dleq_e',
                proof_record->>'dleq_s',
                proof_record->>'dleq_r'
            )
            ON CONFLICT (y, wallet_id) DO UPDATE SET
                mint_url = EXCLUDED.mint_url,
                state = EXCLUDED.state,
                spending_condition = EXCLUDED.spending_condition,
                unit = EXCLUDED.unit,
                amount = EXCLUDED.amount,
                keyset_id = EXCLUDED.keyset_id,
                secret = EXCLUDED.secret,
                c = EXCLUDED.c,
                witness = EXCLUDED.witness,
                dleq_e = EXCLUDED.dleq_e,
                dleq_s = EXCLUDED.dleq_s,
                dleq_r = EXCLUDED.dleq_r;
            
            added_count := added_count + 1;
        END LOOP;
    END IF;
    
    RETURN jsonb_build_object(
        'added', added_count,
        'removed', removed_count
    );
END;
$$;

GRANT EXECUTE ON FUNCTION update_proofs_atomic(JSONB, JSONB) TO authenticated;
GRANT EXECUTE ON FUNCTION update_proofs_atomic(JSONB, JSONB) TO service_role;

COMMENT ON FUNCTION update_proofs_atomic(JSONB, JSONB) IS 
    'Atomically adds and removes proofs in a single transaction. '
    'p_proofs_to_add is a JSON array of proof objects to upsert. '
    'p_ys_to_remove is a JSON array of Y value strings (hex) to delete. '
    'Returns JSON with added and removed counts. '
    'Automatically scoped to the current wallet_id.';

-- ============================================================================
-- ROW LEVEL SECURITY
-- ============================================================================

-- Enable RLS on all tables
ALTER TABLE kv_store ENABLE ROW LEVEL SECURITY;
ALTER TABLE mint ENABLE ROW LEVEL SECURITY;
ALTER TABLE keyset ENABLE ROW LEVEL SECURITY;
ALTER TABLE key ENABLE ROW LEVEL SECURITY;
ALTER TABLE keyset_counter ENABLE ROW LEVEL SECURITY;
ALTER TABLE mint_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE melt_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE proof ENABLE ROW LEVEL SECURITY;
ALTER TABLE transactions ENABLE ROW LEVEL SECURITY;

-- RLS Policies: Users can only access their own data
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

-- Grant permissions to authenticated users
GRANT ALL ON ALL TABLES IN SCHEMA public TO authenticated;
GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO authenticated;
