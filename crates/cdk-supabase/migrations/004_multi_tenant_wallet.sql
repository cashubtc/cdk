-- Migration 004: Multi-tenant Wallet Support
-- Adds wallet_id to all tables and enables RLS

-- Helper Function: Extract Wallet ID (defaults to auth.uid())
CREATE OR REPLACE FUNCTION public.get_current_wallet_id()
RETURNS text AS $$
  SELECT nullif(
    current_setting('request.jwt.claims', true)::json->>'sub',
    ''
  )::text;
$$ LANGUAGE sql STABLE SECURITY DEFINER;

-- Trigger function for versioning (Optimistic Concurrency)
CREATE OR REPLACE FUNCTION increment_opt_version()
RETURNS TRIGGER AS $$
BEGIN
  IF (TG_OP = 'UPDATE') THEN
      NEW.opt_version = COALESCE(OLD.opt_version, 0) + 1;
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- 1. Mint Table
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='mint' AND column_name='wallet_id') THEN
        ALTER TABLE mint ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
        ALTER TABLE mint ADD COLUMN opt_version INTEGER NOT NULL DEFAULT 1;
    END IF;

    -- Update PK to include wallet_id
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'mint_pkey') THEN
        ALTER TABLE mint DROP CONSTRAINT mint_pkey CASCADE;
    END IF;
    ALTER TABLE mint ADD PRIMARY KEY (mint_url, wallet_id);
END $$;

DROP TRIGGER IF EXISTS increment_mint_opt_version ON mint;
CREATE TRIGGER increment_mint_opt_version BEFORE UPDATE ON mint FOR EACH ROW EXECUTE FUNCTION increment_opt_version();

-- 2. Keyset Table
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='keyset' AND column_name='wallet_id') THEN
        ALTER TABLE keyset ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
        ALTER TABLE keyset ADD COLUMN opt_version INTEGER NOT NULL DEFAULT 1;
    END IF;

    -- Update PK
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'keyset_pkey') THEN
        ALTER TABLE keyset DROP CONSTRAINT keyset_pkey CASCADE;
    END IF;
    ALTER TABLE keyset ADD PRIMARY KEY (id, wallet_id);

    -- Foreign Keys
    -- We need to drop existing FKs to recreate them with composite keys if necessary, 
    -- but since mint table PK changed, we must update FKs referencing it.
    -- However, standard cdk implementation might reference just mint_url.
    -- If we made mint PK composite, we must update FKs.
    
    -- Drop old FKs if they exist
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'keyset_mint_url_fkey') THEN
        ALTER TABLE keyset DROP CONSTRAINT keyset_mint_url_fkey;
    END IF;

    ALTER TABLE keyset ADD CONSTRAINT keyset_mint_url_wallet_id_fkey 
    FOREIGN KEY (mint_url, wallet_id) REFERENCES mint(mint_url, wallet_id) ON DELETE CASCADE;
END $$;

-- 3. Key Table
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='key' AND column_name='wallet_id') THEN
        ALTER TABLE key ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
    END IF;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'key_pkey') THEN
        ALTER TABLE key DROP CONSTRAINT key_pkey CASCADE;
    END IF;
    ALTER TABLE key ADD PRIMARY KEY (id, wallet_id);

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'key_id_fkey') THEN
        ALTER TABLE key DROP CONSTRAINT key_id_fkey;
    END IF;
    -- FK to keyset
    ALTER TABLE key ADD CONSTRAINT key_id_wallet_id_fkey
    FOREIGN KEY (id, wallet_id) REFERENCES keyset(id, wallet_id) ON DELETE CASCADE;
END $$;

-- 4. Keyset Counter
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='keyset_counter' AND column_name='wallet_id') THEN
        ALTER TABLE keyset_counter ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
    END IF;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'keyset_counter_pkey') THEN
        ALTER TABLE keyset_counter DROP CONSTRAINT keyset_counter_pkey CASCADE;
    END IF;
    ALTER TABLE keyset_counter ADD PRIMARY KEY (keyset_id, wallet_id);

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'keyset_counter_keyset_id_fkey') THEN
        ALTER TABLE keyset_counter DROP CONSTRAINT keyset_counter_keyset_id_fkey;
    END IF;
    -- FK to keyset
    ALTER TABLE keyset_counter ADD CONSTRAINT keyset_counter_keyset_id_wallet_id_fkey
    FOREIGN KEY (keyset_id, wallet_id) REFERENCES keyset(id, wallet_id) ON DELETE CASCADE;
END $$;

-- 5. Proof Table
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='proof' AND column_name='wallet_id') THEN
        ALTER TABLE proof ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
        ALTER TABLE proof ADD COLUMN opt_version INTEGER NOT NULL DEFAULT 1;
    END IF;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'proof_pkey') THEN
        ALTER TABLE proof DROP CONSTRAINT proof_pkey CASCADE;
    END IF;
    ALTER TABLE proof ADD PRIMARY KEY (y, wallet_id);

    -- FKs
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'proof_mint_url_fkey') THEN
        ALTER TABLE proof DROP CONSTRAINT proof_mint_url_fkey;
    END IF;
    ALTER TABLE proof ADD CONSTRAINT proof_mint_url_wallet_id_fkey
    FOREIGN KEY (mint_url, wallet_id) REFERENCES mint(mint_url, wallet_id) ON DELETE CASCADE;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'proof_keyset_id_fkey') THEN
        ALTER TABLE proof DROP CONSTRAINT proof_keyset_id_fkey;
    END IF;
    ALTER TABLE proof ADD CONSTRAINT proof_keyset_id_wallet_id_fkey
    FOREIGN KEY (keyset_id, wallet_id) REFERENCES keyset(id, wallet_id) ON DELETE CASCADE;
END $$;

DROP TRIGGER IF EXISTS increment_proof_opt_version ON proof;
CREATE TRIGGER increment_proof_opt_version BEFORE UPDATE ON proof FOR EACH ROW EXECUTE FUNCTION increment_opt_version();

-- 6. Transactions Table
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='transactions' AND column_name='wallet_id') THEN
        ALTER TABLE transactions ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
        ALTER TABLE transactions ADD COLUMN opt_version INTEGER NOT NULL DEFAULT 1;
        ALTER TABLE transactions ADD COLUMN status TEXT NOT NULL DEFAULT 'completed';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'transactions_pkey') THEN
        ALTER TABLE transactions DROP CONSTRAINT transactions_pkey CASCADE;
    END IF;
    ALTER TABLE transactions ADD PRIMARY KEY (id, wallet_id);

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'transactions_mint_url_fkey') THEN
        ALTER TABLE transactions DROP CONSTRAINT transactions_mint_url_fkey;
    END IF;
    ALTER TABLE transactions ADD CONSTRAINT transactions_mint_url_wallet_id_fkey
    FOREIGN KEY (mint_url, wallet_id) REFERENCES mint(mint_url, wallet_id) ON DELETE CASCADE;
END $$;

DROP TRIGGER IF EXISTS increment_transactions_opt_version ON transactions;
CREATE TRIGGER increment_transactions_opt_version BEFORE UPDATE ON transactions FOR EACH ROW EXECUTE FUNCTION increment_opt_version();

-- 7. Mint Quote
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='mint_quote' AND column_name='wallet_id') THEN
        ALTER TABLE mint_quote ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
    END IF;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'mint_quote_pkey') THEN
        ALTER TABLE mint_quote DROP CONSTRAINT mint_quote_pkey CASCADE;
    END IF;
    ALTER TABLE mint_quote ADD PRIMARY KEY (id, wallet_id);

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'mint_quote_mint_url_fkey') THEN
        ALTER TABLE mint_quote DROP CONSTRAINT mint_quote_mint_url_fkey;
    END IF;
    ALTER TABLE mint_quote ADD CONSTRAINT mint_quote_mint_url_wallet_id_fkey
    FOREIGN KEY (mint_url, wallet_id) REFERENCES mint(mint_url, wallet_id) ON DELETE CASCADE;
END $$;

-- 8. Melt Quote
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='melt_quote' AND column_name='wallet_id') THEN
        ALTER TABLE melt_quote ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
    END IF;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'melt_quote_pkey') THEN
        ALTER TABLE melt_quote DROP CONSTRAINT melt_quote_pkey CASCADE;
    END IF;
    ALTER TABLE melt_quote ADD PRIMARY KEY (id, wallet_id);
END $$;

-- 9. KV Store
DO $$ 
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='kv_store' AND column_name='wallet_id') THEN
        ALTER TABLE kv_store ADD COLUMN wallet_id TEXT DEFAULT public.get_current_wallet_id();
    END IF;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'kv_store_pkey') THEN
        ALTER TABLE kv_store DROP CONSTRAINT kv_store_pkey CASCADE;
    END IF;
    ALTER TABLE kv_store ADD PRIMARY KEY (primary_namespace, secondary_namespace, key, wallet_id);
END $$;

-- Enable RLS and enforce wallet_id
-- We assume authentication is handled via Supabase Auth (or similar) which populates request.jwt.claims
-- For the 'service_role' (e.g. migration runner), we might need to bypass.
-- But generally, normal users should only see their own wallet_id.

-- Function to check if user owns the record
-- Note: Postgres RLS policies
ALTER TABLE mint ENABLE ROW LEVEL SECURITY;
ALTER TABLE keyset ENABLE ROW LEVEL SECURITY;
ALTER TABLE key ENABLE ROW LEVEL SECURITY;
ALTER TABLE keyset_counter ENABLE ROW LEVEL SECURITY;
ALTER TABLE proof ENABLE ROW LEVEL SECURITY;
ALTER TABLE transactions ENABLE ROW LEVEL SECURITY;
ALTER TABLE mint_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE melt_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE kv_store ENABLE ROW LEVEL SECURITY;

-- Policy: Users can only see/modify their own data
-- We drop existing policies if any (from 001) to replace with stricter ones
DROP POLICY IF EXISTS "Enable all for authenticated users" ON mint;
CREATE POLICY "Users access own mints" ON mint FOR ALL USING (wallet_id = public.get_current_wallet_id());

DROP POLICY IF EXISTS "Enable all for authenticated users" ON keyset;
CREATE POLICY "Users access own keysets" ON keyset FOR ALL USING (wallet_id = public.get_current_wallet_id());

DROP POLICY IF EXISTS "Enable all for authenticated users" ON key;
CREATE POLICY "Users access own keys" ON key FOR ALL USING (wallet_id = public.get_current_wallet_id());

DROP POLICY IF EXISTS "Enable all for authenticated users" ON keyset_counter;
CREATE POLICY "Users access own counters" ON keyset_counter FOR ALL USING (wallet_id = public.get_current_wallet_id());

DROP POLICY IF EXISTS "Enable all for authenticated users" ON proof;
CREATE POLICY "Users access own proofs" ON proof FOR ALL USING (wallet_id = public.get_current_wallet_id());

DROP POLICY IF EXISTS "Enable all for authenticated users" ON transactions;
CREATE POLICY "Users access own transactions" ON transactions FOR ALL USING (wallet_id = public.get_current_wallet_id());

DROP POLICY IF EXISTS "Enable all for authenticated users" ON mint_quote;
CREATE POLICY "Users access own mint quotes" ON mint_quote FOR ALL USING (wallet_id = public.get_current_wallet_id());

DROP POLICY IF EXISTS "Enable all for authenticated users" ON melt_quote;
CREATE POLICY "Users access own melt quotes" ON melt_quote FOR ALL USING (wallet_id = public.get_current_wallet_id());

DROP POLICY IF EXISTS "Enable all for authenticated users" ON kv_store;
CREATE POLICY "Users access own kv_store" ON kv_store FOR ALL USING (wallet_id = public.get_current_wallet_id());
