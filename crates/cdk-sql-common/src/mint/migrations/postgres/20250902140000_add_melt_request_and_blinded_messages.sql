-- Drop existing melt_request table and recreate with new schema
DROP TABLE IF EXISTS melt_request;
CREATE TABLE melt_request (
    quote_id TEXT PRIMARY KEY,
    inputs_amount INTEGER NOT NULL,
    inputs_fee INTEGER NOT NULL,
    FOREIGN KEY (quote_id) REFERENCES melt_quote(id)
);

-- Add blinded_messages table
CREATE TABLE blinded_messages (
    quote_id TEXT NOT NULL,
    blinded_message BYTEA NOT NULL,
    keyset_id TEXT NOT NULL,
    amount INTEGER NOT NULL,
    FOREIGN KEY (quote_id) REFERENCES melt_request(quote_id) ON DELETE CASCADE
);

-- Add index for faster lookups on blinded_messages
CREATE INDEX blinded_messages_quote_id_index ON blinded_messages(quote_id);
