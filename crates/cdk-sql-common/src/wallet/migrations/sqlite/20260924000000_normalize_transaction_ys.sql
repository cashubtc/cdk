-- Store one proof identifier per row, preserving the original list order.
CREATE TABLE transaction_ys (
    transaction_id BLOB NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    y BLOB NOT NULL CHECK (length(y) IN (33, 48)),
    PRIMARY KEY (transaction_id, position)
);

-- Federation wallets may contain JSON arrays of hex points; older wallets
-- contain concatenated compressed secp256k1 or BLS points.
INSERT INTO transaction_ys (transaction_id, position, y)
SELECT transactions.id, CAST(json_each.key AS INTEGER), unhex(json_each.value)
FROM transactions, json_each(CASE WHEN substr(ys, 1, 1) = x'5b' THEN CAST(ys AS TEXT) ELSE '[]' END);

WITH RECURSIVE points(transaction_id, position, bytes, width) AS (
    SELECT id, 0, ys, CASE WHEN hex(substr(ys, 1, 1)) IN ('02', '03') THEN 33 ELSE 48 END
    FROM transactions WHERE length(ys) > 0 AND substr(ys, 1, 1) != x'5b'
    UNION ALL
    SELECT transaction_id, position + 1, substr(bytes, width + 1), width
    FROM points WHERE length(bytes) > width
)
INSERT INTO transaction_ys (transaction_id, position, y)
SELECT transaction_id, position, substr(bytes, 1, width) FROM points;

ALTER TABLE transactions DROP COLUMN ys;
