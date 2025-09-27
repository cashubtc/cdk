-- Remove NOT NULL constraint from c column in blind_signature table
ALTER TABLE blind_signature ALTER COLUMN c DROP NOT NULL;

-- Add signed_time column to blind_signature table
ALTER TABLE blind_signature ADD COLUMN signed_time INTEGER NULL;

-- Update existing records to set signed_time equal to created_time for existing signatures
UPDATE blind_signature SET signed_time = created_time WHERE c IS NOT NULL;

-- Insert data from blinded_messages table into blind_signature table with NULL c column
INSERT INTO blind_signature (blinded_message, amount, keyset_id, c, quote_id, created_time, signed_time)
SELECT blinded_message, amount, keyset_id, NULL as c, quote_id, 0 as created_time, NULL as signed_time
FROM blinded_messages
WHERE NOT EXISTS (
    SELECT 1 FROM blind_signature 
    WHERE blind_signature.blinded_message = blinded_messages.blinded_message
);

-- Create index on quote_id if it does not exist
CREATE INDEX IF NOT EXISTS blind_signature_quote_id_index ON blind_signature(quote_id);

-- Drop the blinded_messages table as data has been migrated
DROP TABLE IF EXISTS blinded_messages;
