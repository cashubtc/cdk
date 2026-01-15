-- Migration to add wallet sagas table and proof operation tracking

-- Create wallet_sagas table with version for optimistic locking
CREATE TABLE IF NOT EXISTS wallet_sagas (
    id TEXT PRIMARY KEY,
    kind TEXT CHECK (kind IN ('send', 'receive', 'swap', 'mint', 'melt')) NOT NULL,
    state TEXT NOT NULL,
    amount BIGINT NOT NULL,
    mint_url TEXT NOT NULL,
    unit TEXT NOT NULL,
    quote_id TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    data TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0
);

-- Create indexes for efficient queries
CREATE INDEX IF NOT EXISTS wallet_sagas_mint_url_index ON wallet_sagas(mint_url);
CREATE INDEX IF NOT EXISTS wallet_sagas_kind_index ON wallet_sagas(kind);
CREATE INDEX IF NOT EXISTS wallet_sagas_created_at_index ON wallet_sagas(created_at);

-- Add operation tracking columns to proof table
ALTER TABLE proof ADD COLUMN IF NOT EXISTS used_by_operation TEXT;
ALTER TABLE proof ADD COLUMN IF NOT EXISTS created_by_operation TEXT;

-- Create index for efficient operation-based proof queries
CREATE INDEX IF NOT EXISTS proof_used_by_operation_index ON proof(used_by_operation);
CREATE INDEX IF NOT EXISTS proof_created_by_operation_index ON proof(created_by_operation);

-- Add operation tracking to quote tables to prevent concurrent operations on same quote
ALTER TABLE melt_quote ADD COLUMN IF NOT EXISTS used_by_operation TEXT;
ALTER TABLE mint_quote ADD COLUMN IF NOT EXISTS used_by_operation TEXT;

-- Create indexes for efficient operation-based quote queries
CREATE INDEX IF NOT EXISTS melt_quote_used_by_operation_index ON melt_quote(used_by_operation);
CREATE INDEX IF NOT EXISTS mint_quote_used_by_operation_index ON mint_quote(used_by_operation);
