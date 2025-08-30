-- Create a new table with the updated CHECK constraint
CREATE TABLE proof_new (
    y BLOB PRIMARY KEY,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL,
    secret TEXT NOT NULL,
    c BLOB NOT NULL,
    witness TEXT,
    state TEXT CHECK (state IN ('SPENT', 'PENDING', 'UNSPENT')) NOT NULL
);

-- Copy the data from the old table to the new table
INSERT INTO proof_new (y, amount, keyset_id, secret, c, witness, state)
SELECT y, amount, keyset_id, secret, c, witness, state
FROM proof;

-- Drop the old table
DROP TABLE proof;

-- Rename the new table to the original table name
ALTER TABLE proof_new RENAME TO proof;
