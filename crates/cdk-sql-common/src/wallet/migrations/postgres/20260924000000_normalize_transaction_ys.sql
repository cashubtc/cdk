-- Store one proof identifier per row, preserving the original list order.
CREATE TABLE transaction_ys (
    transaction_id BYTEA NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    y BYTEA NOT NULL CHECK (octet_length(y) IN (33, 48)),
    PRIMARY KEY (transaction_id, position)
);

INSERT INTO transaction_ys (transaction_id, position, y)
SELECT id, offset_bytes / 33, substring(ys FROM offset_bytes + 1 FOR 33)
FROM transactions
CROSS JOIN LATERAL generate_series(0, octet_length(ys) - 1, 33) AS offsets(offset_bytes);

ALTER TABLE transactions DROP COLUMN ys;
