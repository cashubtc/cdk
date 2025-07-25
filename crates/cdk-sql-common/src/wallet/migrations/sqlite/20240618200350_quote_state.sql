ALTER TABLE melt_quote ADD state TEXT CHECK ( state IN ('UNPAID', 'PENDING', 'PAID' ) ) NOT NULL DEFAULT 'UNPAID';
ALTER TABLE melt_quote ADD payment_preimage TEXT;
ALTER TABLE melt_quote DROP COLUMN paid;
CREATE INDEX IF NOT EXISTS melt_quote_state_index ON melt_quote(state);
DROP INDEX IF EXISTS paid_index;
