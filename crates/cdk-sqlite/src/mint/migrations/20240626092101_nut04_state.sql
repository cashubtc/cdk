ALTER TABLE mint_quote ADD state TEXT CHECK ( state IN ('UNPAID', 'PENDING', 'PAID', 'ISSUED' ) ) NOT NULL DEFAULT 'UNPAID';
ALTER TABLE mint_quote DROP COLUMN paid;
CREATE INDEX IF NOT EXISTS mint_quote_state_index ON mint_quote(state);
