ALTER TABLE melt_quote DROP CONSTRAINT IF EXISTS melt_quote_state_check;

ALTER TABLE melt_quote
ADD CONSTRAINT melt_quote_state_check
CHECK (state IN ('UNPAID', 'PENDING', 'PAID', 'FAILED'));
