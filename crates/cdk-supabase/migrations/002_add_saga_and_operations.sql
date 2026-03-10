-- Migration 002: Add saga table, operation tracking columns, and reservation fields
-- Supports the wallet saga pattern for atomic multi-step operations

-- ============================================================================
-- SAGA TABLE
-- ============================================================================

CREATE TABLE IF NOT EXISTS saga (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    data TEXT NOT NULL,       -- JSON-serialized WalletSaga
    version INTEGER NOT NULL DEFAULT 0,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at BIGINT NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (id, wallet_id)
);

ALTER TABLE saga ENABLE ROW LEVEL SECURITY;

CREATE POLICY saga_rls ON saga
    FOR ALL
    USING (wallet_id = public.get_current_wallet_id())
    WITH CHECK (wallet_id = public.get_current_wallet_id());

-- ============================================================================
-- ADD OPERATION TRACKING COLUMNS
-- ============================================================================

-- mint_quote: track which operation is using this quote + optimistic locking
ALTER TABLE mint_quote ADD COLUMN IF NOT EXISTS used_by_operation TEXT;
ALTER TABLE mint_quote ADD COLUMN IF NOT EXISTS version INTEGER DEFAULT 0;

-- melt_quote: track which operation is using this quote + optimistic locking
ALTER TABLE melt_quote ADD COLUMN IF NOT EXISTS used_by_operation TEXT;
ALTER TABLE melt_quote ADD COLUMN IF NOT EXISTS version INTEGER DEFAULT 0;

-- proof: track which operation created/is using this proof
ALTER TABLE proof ADD COLUMN IF NOT EXISTS used_by_operation TEXT;
ALTER TABLE proof ADD COLUMN IF NOT EXISTS created_by_operation TEXT;

-- transactions: link to saga
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS saga_id TEXT;
