-- Accept BLS G1 identifiers in already-normalized transaction history.
CREATE TABLE transaction_ys_bls (
    transaction_id BLOB NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    y BLOB NOT NULL CHECK (length(y) IN (33, 48)),
    PRIMARY KEY (transaction_id, position)
);

INSERT INTO transaction_ys_bls (transaction_id, position, y)
SELECT transaction_id, position, y FROM transaction_ys;

DROP TABLE transaction_ys;
ALTER TABLE transaction_ys_bls RENAME TO transaction_ys;
