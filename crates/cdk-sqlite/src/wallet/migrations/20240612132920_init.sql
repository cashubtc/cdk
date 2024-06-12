-- Mints
CREATE TABLE IF NOT EXISTS mint (
    mint_url TEXT PRIMARY KEY,
    name TEXT,
    pubkey BLOB,
    version TEXT,
    description TEXT,
    description_long TEXT,
    contact TEXT,
    nuts TEXT,
    motd TEXT
);


CREATE TABLE IF NOT EXISTS keyset (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    counter INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(mint_url) REFERENCES mint(mint_url) ON UPDATE CASCADE ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS mint_quote (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    amount INTEGER NOT NULL,
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    paid BOOL NOT NULL DEFAULT FALSE,
    expiry INTEGER NOT NULL
);


CREATE INDEX IF NOT EXISTS paid_index ON mint_quote(paid);
CREATE INDEX IF NOT EXISTS request_index ON mint_quote(request);

CREATE TABLE IF NOT EXISTS melt_quote (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount INTEGER NOT NULL,
    request TEXT NOT NULL,
    fee_reserve INTEGER NOT NULL,
    paid BOOL NOT NULL DEFAULT FALSE,
    expiry INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS paid_index ON melt_quote(paid);
CREATE INDEX IF NOT EXISTS request_index ON melt_quote(request);

CREATE TABLE IF NOT EXISTS key (
    id TEXT PRIMARY KEY,
    keys TEXT NOT NULL
);


-- Proof Table
CREATE TABLE IF NOT EXISTS proof (
y BLOB PRIMARY KEY,
mint_url TEXT NOT NULL,
state TEXT CHECK ( state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED' ) ) NOT NULL,
spending_condition TEXT,
unit TEXT NOT NULL,
amount INTEGER NOT NULL,
keyset_id TEXT NOT NULL,
secret TEXT NOT NULL,
c BLOB NOT NULL,
witness TEXT
);

CREATE INDEX IF NOT EXISTS secret_index ON proof(secret);
CREATE INDEX IF NOT EXISTS state_index ON proof(state);
CREATE INDEX IF NOT EXISTS spending_condition_index ON proof(spending_condition);
CREATE INDEX IF NOT EXISTS unit_index ON proof(unit);
CREATE INDEX IF NOT EXISTS amount_index ON proof(amount);
CREATE INDEX IF NOT EXISTS mint_url_index ON proof(mint_url);

CREATE TABLE IF NOT EXISTS nostr_last_checked (
    key BLOB PRIMARY KEY,
    last_check INTEGER NOT NULL
);
