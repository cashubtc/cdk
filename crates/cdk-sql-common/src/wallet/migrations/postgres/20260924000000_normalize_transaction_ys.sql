-- Store one proof identifier per row, preserving the original list order.
CREATE TABLE transaction_ys (
    transaction_id BYTEA NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    y BYTEA NOT NULL CHECK (octet_length(y) IN (33, 48)),
    PRIMARY KEY (transaction_id, position)
);

-- Preserve JSON point arrays written by federation wallets as well as the
-- concatenated compressed point encoding used by older wallets.
INSERT INTO transaction_ys (transaction_id, position, y)
SELECT id, ordinal - 1, decode(point, 'hex')
FROM transactions
CROSS JOIN LATERAL jsonb_array_elements_text(
    CASE WHEN substring(ys FROM 1 FOR 1) = decode('5b', 'hex')
         THEN convert_from(ys, 'UTF8')::jsonb ELSE '[]'::jsonb END
) WITH ORDINALITY AS points(point, ordinal);

INSERT INTO transaction_ys (transaction_id, position, y)
SELECT id, offset_bytes / width, substring(ys FROM offset_bytes + 1 FOR width)
FROM transactions
CROSS JOIN LATERAL (
    SELECT CASE WHEN substring(ys FROM 1 FOR 1) IN (decode('02', 'hex'), decode('03', 'hex'))
                THEN 33 ELSE 48 END AS width
) AS encoding
CROSS JOIN LATERAL generate_series(0, octet_length(ys) - 1, width) AS offsets(offset_bytes)
WHERE substring(ys FROM 1 FOR 1) != decode('5b', 'hex');

ALTER TABLE transactions DROP COLUMN ys;
