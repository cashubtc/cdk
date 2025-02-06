CREATE TABLE IF NOT EXISTS transaction (
    id TEXT PRIMARY KEY,
    amount INTEGER NOT NULL,
    direction TEXT NOT NULL,
    mint_url TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    unit TEXT NOT NULL,
    ys TEXT NOT NULL,
    memo TEXT,
    metadata TEXT NOT NULL,
);

CREATE INDEX IF NOT EXISTS timestamp_index ON transaction(timestamp);
