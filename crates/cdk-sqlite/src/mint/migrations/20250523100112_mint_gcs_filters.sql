CREATE TABLE IF NOT EXISTS spent_filters (
    keyset_id TEXT PRIMARY KEY,
    content BLOB NOT NULL,
    num_items INTEGER NOT NULL,
    inv_false_positive_rate INTEGER NOT NULL,
    remainder_bitlength INTEGER NOT NULL,
    time TIMESTAMP NOT NULL
);