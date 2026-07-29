CREATE INDEX idx_mint_quote_federation_payment_observation
ON mint_quote (amount_paid, amount_issued, id);

CREATE INDEX idx_melt_quote_federation_payment_observation
ON melt_quote (state, id);
