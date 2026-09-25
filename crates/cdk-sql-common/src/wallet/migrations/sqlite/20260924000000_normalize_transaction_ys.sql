-- Store one proof identifier per row, preserving the original list order.
CREATE TABLE transaction_ys (
    transaction_id BLOB NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    y BLOB NOT NULL CHECK (length(y) IN (33, 48)),
    PRIMARY KEY (transaction_id, position)
);

WITH RECURSIVE points(transaction_id, position, bytes) AS (
    SELECT id, 0, ys FROM transactions WHERE length(ys) > 0
    UNION ALL
    SELECT transaction_id, position + 1, substr(bytes, 34)
    FROM points WHERE length(bytes) > 33
)
INSERT INTO transaction_ys (transaction_id, position, y)
SELECT transaction_id, position, substr(bytes, 1, 33) FROM points;

ALTER TABLE transactions DROP COLUMN ys;
