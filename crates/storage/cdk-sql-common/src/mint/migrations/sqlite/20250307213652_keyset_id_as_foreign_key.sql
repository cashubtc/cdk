-- Add foreign key constraints for keyset_id in SQLite
-- SQL requires recreating tables to add foreign keys

-- First, ensure we have the right schema information
PRAGMA foreign_keys = OFF;

-- Create new proof table with foreign key constraint
CREATE TABLE proof_new (
    y BLOB PRIMARY KEY,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL REFERENCES keyset(id),
    secret TEXT NOT NULL,
    c BLOB NOT NULL,
    witness TEXT,
    state TEXT CHECK (state IN ('SPENT', 'PENDING', 'UNSPENT', 'RESERVED', 'UNKNOWN')) NOT NULL,
    quote_id TEXT
);

-- Copy data from old proof table to new one
INSERT INTO proof_new SELECT * FROM proof;

-- Create new blind_signature table with foreign key constraint
CREATE TABLE blind_signature_new (
    y BLOB PRIMARY KEY,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL REFERENCES keyset(id),
    c BLOB NOT NULL,
    dleq_e TEXT,
    dleq_s TEXT,
    quote_id TEXT
);

-- Copy data from old blind_signature table to new one
INSERT INTO blind_signature_new SELECT * FROM blind_signature;

-- Drop old tables
DROP TABLE IF EXISTS proof;
DROP TABLE IF EXISTS blind_signature;

-- Rename new tables to original names
ALTER TABLE proof_new RENAME TO proof;
ALTER TABLE blind_signature_new RENAME TO blind_signature;

-- Recreate all indexes
CREATE INDEX IF NOT EXISTS proof_keyset_id_index ON proof(keyset_id);
CREATE INDEX IF NOT EXISTS state_index ON proof(state);
CREATE INDEX IF NOT EXISTS secret_index ON proof(secret);
CREATE INDEX IF NOT EXISTS blind_signature_keyset_id_index ON blind_signature(keyset_id);

-- Re-enable foreign keys
PRAGMA foreign_keys = ON;
