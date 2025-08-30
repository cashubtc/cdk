CREATE TABLE mint (
  mint_url TEXT PRIMARY KEY, name TEXT, 
  pubkey BYTEA, version TEXT, description TEXT, 
  description_long TEXT, contact TEXT, 
  nuts TEXT, motd TEXT, icon_url TEXT, 
  mint_time INTEGER, urls TEXT, tos_url TEXT
);
CREATE TABLE keyset (
  id TEXT PRIMARY KEY, 
  mint_url TEXT NOT NULL, 
  unit TEXT NOT NULL, 
  active BOOL NOT NULL, 
  counter INTEGER NOT NULL DEFAULT 0, 
  input_fee_ppk INTEGER, 
  final_expiry INTEGER DEFAULT NULL, 
  FOREIGN KEY(mint_url) REFERENCES mint(mint_url) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE TABLE melt_quote (
  id TEXT PRIMARY KEY, 
  unit TEXT NOT NULL, 
  amount INTEGER NOT NULL, 
  request TEXT NOT NULL, 
  fee_reserve INTEGER NOT NULL, 
  expiry INTEGER NOT NULL, 
  state TEXT CHECK (
    state IN ('UNPAID', 'PENDING', 'PAID')
  ) NOT NULL DEFAULT 'UNPAID', 
  payment_preimage TEXT
);
CREATE TABLE key (
  id TEXT PRIMARY KEY, keys TEXT NOT NULL
);
CREATE INDEX melt_quote_state_index ON melt_quote(state);
CREATE TABLE IF NOT EXISTS "proof" (
  y BYTEA PRIMARY KEY, 
  mint_url TEXT NOT NULL, 
  state TEXT CHECK (
    state IN (
      'SPENT', 'UNSPENT', 'PENDING', 'RESERVED', 
      'PENDING_SPENT'
    )
  ) NOT NULL, 
  spending_condition TEXT, 
  unit TEXT NOT NULL, 
  amount INTEGER NOT NULL, 
  keyset_id TEXT NOT NULL, 
  secret TEXT NOT NULL, 
  c BYTEA NOT NULL, 
  witness TEXT, 
  dleq_e BYTEA, 
  dleq_s BYTEA, 
  dleq_r BYTEA
);
CREATE TABLE transactions (
  id BYTEA PRIMARY KEY, 
  mint_url TEXT NOT NULL, 
  direction TEXT CHECK (
    direction IN ('Incoming', 'Outgoing')
  ) NOT NULL, 
  amount INTEGER NOT NULL, 
  fee INTEGER NOT NULL, 
  unit TEXT NOT NULL, 
  ys BYTEA NOT NULL, 
  timestamp INTEGER NOT NULL, 
  memo TEXT, 
  metadata TEXT
);
CREATE INDEX mint_url_index ON transactions(mint_url);
CREATE INDEX direction_index ON transactions(direction);
CREATE INDEX unit_index ON transactions(unit);
CREATE INDEX timestamp_index ON transactions(timestamp);
CREATE TABLE IF NOT EXISTS "mint_quote" (
  id TEXT PRIMARY KEY, mint_url TEXT NOT NULL, 
  payment_method TEXT NOT NULL DEFAULT 'bolt11', 
  amount INTEGER, unit TEXT NOT NULL, 
  request TEXT NOT NULL, state TEXT NOT NULL, 
  expiry INTEGER NOT NULL, amount_paid INTEGER NOT NULL DEFAULT 0, 
  amount_issued INTEGER NOT NULL DEFAULT 0, 
  secret_key TEXT
);
