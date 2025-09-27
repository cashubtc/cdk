-- Remove NOT NULL constraint from c column in blind_signature table
-- SQLite does not support ALTER COLUMN directly, so we need to recreate the table

-- Step 1 - Create new table with nullable c column and signed_time column
CREATE TABLE blind_signature_new (
    blinded_message BLOB PRIMARY KEY,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL,
    c BLOB NULL,
    dleq_e TEXT,
    dleq_s TEXT,
    quote_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    signed_time INTEGER
);

-- Step 2 - Copy existing data from old blind_signature table
INSERT INTO blind_signature_new (blinded_message, amount, keyset_id, c, dleq_e, dleq_s, quote_id, created_time)
SELECT blinded_message, amount, keyset_id, c, dleq_e, dleq_s, quote_id, created_time
FROM blind_signature;

-- Step 3 - Insert data from blinded_messages table with NULL c column
INSERT INTO blind_signature_new (blinded_message, amount, keyset_id, c, quote_id, created_time)
SELECT blinded_message, amount, keyset_id, NULL as c, quote_id, 0 as created_time
FROM blinded_messages
WHERE NOT EXISTS (
    SELECT 1 FROM blind_signature_new 
    WHERE blind_signature_new.blinded_message = blinded_messages.blinded_message
);

-- Step 4 - Drop old table and rename new table
DROP TABLE blind_signature;
ALTER TABLE blind_signature_new RENAME TO blind_signature;

-- Step 5 - Recreate indexes
CREATE INDEX IF NOT EXISTS keyset_id_index ON blind_signature(keyset_id);
CREATE INDEX IF NOT EXISTS blind_signature_quote_id_index ON blind_signature(quote_id);

-- Step 6 - Drop the blinded_messages table as data has been migrated
DROP TABLE IF EXISTS blinded_messages;
