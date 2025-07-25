CREATE TABLE keyset (
  id TEXT PRIMARY KEY, unit TEXT NOT NULL,
  active BOOL NOT NULL, valid_from INTEGER NOT NULL,
  valid_to INTEGER, derivation_path TEXT NOT NULL,
  max_order INTEGER NOT NULL, input_fee_ppk INTEGER,
  derivation_path_index INTEGER
);
CREATE TABLE mint_quote (
  id TEXT PRIMARY KEY,
  amount INTEGER NOT NULL,
  unit TEXT NOT NULL,
  request TEXT NOT NULL,
  expiry INTEGER NOT NULL,
  state TEXT CHECK (
    state IN (
      'UNPAID', 'PENDING', 'PAID', 'ISSUED'
    )
  ) NOT NULL DEFAULT 'UNPAID',
  request_lookup_id TEXT,
  pubkey TEXT,
  created_time INTEGER NOT NULL DEFAULT 0,
  paid_time INTEGER,
  issued_time INTEGER
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
  payment_preimage TEXT,
  request_lookup_id TEXT,
  msat_to_pay INTEGER,
  created_time INTEGER NOT NULL DEFAULT 0,
  paid_time INTEGER
);
CREATE TABLE melt_request (
  id TEXT PRIMARY KEY, inputs TEXT NOT NULL,
  outputs TEXT, method TEXT NOT NULL,
  unit TEXT NOT NULL
);
CREATE TABLE config (
  id TEXT PRIMARY KEY, value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS "proof" (
  y BYTEA PRIMARY KEY,
  amount INTEGER NOT NULL,
  keyset_id TEXT NOT NULL,
  secret TEXT NOT NULL,
  c BYTEA NOT NULL,
  witness TEXT,
  state TEXT CHECK (
    state IN (
      'SPENT', 'PENDING', 'UNSPENT', 'RESERVED',
      'UNKNOWN'
    )
  ) NOT NULL,
  quote_id TEXT,
  created_time INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS "blind_signature" (
  y BYTEA PRIMARY KEY,
  amount INTEGER NOT NULL,
  keyset_id TEXT NOT NULL,
  c BYTEA NOT NULL,
  dleq_e TEXT,
  dleq_s TEXT,
  quote_id TEXT,
  created_time INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX unit_index ON keyset(unit);
CREATE INDEX active_index ON keyset(active);
CREATE INDEX request_index ON mint_quote(request);
CREATE INDEX expiry_index ON mint_quote(expiry);
CREATE INDEX melt_quote_state_index ON melt_quote(state);
CREATE INDEX mint_quote_state_index ON mint_quote(state);
CREATE UNIQUE INDEX unique_request_lookup_id_mint ON mint_quote(request_lookup_id);
CREATE UNIQUE INDEX unique_request_lookup_id_melt ON melt_quote(request_lookup_id);
