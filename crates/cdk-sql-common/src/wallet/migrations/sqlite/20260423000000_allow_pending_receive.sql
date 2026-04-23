-- Create a new table with the updated CHECK constraint
CREATE TABLE IF NOT EXISTS proof_new (
    y BLOB PRIMARY KEY,
    mint_url TEXT NOT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED', 'PENDING_SPENT', 'PENDING_RECEIVE' ) ) NOT NULL,
    spending_condition TEXT,
    unit TEXT NOT NULL,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL,
    secret TEXT NOT NULL,
    c BLOB NOT NULL,
    witness TEXT,
    dleq_e BLOB,
    dleq_s BLOB,
    dleq_r BLOB,
    used_by_operation TEXT,
    created_by_operation TEXT,
    p2pk_e BLOB
);

CREATE INDEX IF NOT EXISTS secret_index ON proof_new(secret);
CREATE INDEX IF NOT EXISTS state_index ON proof_new(state);
CREATE INDEX IF NOT EXISTS spending_condition_index ON proof_new(spending_condition);
CREATE INDEX IF NOT EXISTS unit_index ON proof_new(unit);
CREATE INDEX IF NOT EXISTS amount_index ON proof_new(amount);
CREATE INDEX IF NOT EXISTS mint_url_index ON proof_new(mint_url);
CREATE INDEX IF NOT EXISTS proof_used_by_operation_index ON proof_new(used_by_operation);
CREATE INDEX IF NOT EXISTS proof_created_by_operation_index ON proof_new(created_by_operation);

-- Copy data from old proof table to new proof table
INSERT INTO proof_new (y, mint_url, state, spending_condition, unit, amount, keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r, used_by_operation, created_by_operation, p2pk_e)
SELECT y, mint_url, state, spending_condition, unit, amount, keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r, used_by_operation, created_by_operation, p2pk_e
FROM proof;

-- Drop the old proof table
DROP TABLE proof;

-- Rename the new proof table to proof
ALTER TABLE proof_new RENAME TO proof;
