-- Migration to add transactions table
CREATE TABLE IF NOT EXISTS transactions (
    id BLOB PRIMARY KEY,
    mint_url TEXT NOT NULL,
    direction TEXT CHECK (direction IN ('Incoming', 'Outgoing')) NOT NULL,
    amount INTEGER NOT NULL,
    fee INTEGER NOT NULL,
    unit TEXT NOT NULL,
    ys BLOB NOT NULL,
    timestamp INTEGER NOT NULL,
    memo TEXT,
    metadata TEXT
);

CREATE INDEX IF NOT EXISTS mint_url_index ON transactions(mint_url);
CREATE INDEX IF NOT EXISTS direction_index ON transactions(direction);
CREATE INDEX IF NOT EXISTS unit_index ON transactions(unit);
CREATE INDEX IF NOT EXISTS timestamp_index ON transactions(timestamp);
