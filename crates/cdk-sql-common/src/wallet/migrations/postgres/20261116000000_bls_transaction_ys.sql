-- Accept BLS G1 identifiers in already-normalized transaction history.
ALTER TABLE transaction_ys DROP CONSTRAINT transaction_ys_y_check;
ALTER TABLE transaction_ys ADD CONSTRAINT transaction_ys_y_check
    CHECK (octet_length(y) IN (33, 48));
