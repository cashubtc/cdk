CREATE TABLE keyset (
  id TEXT PRIMARY KEY, unit TEXT NOT NULL,
  active BOOL NOT NULL, valid_from INTEGER NOT NULL,
  valid_to INTEGER, derivation_path TEXT NOT NULL,
  max_order INTEGER NOT NULL, input_fee_ppk INTEGER,
  derivation_path_index INTEGER
);
CREATE INDEX unit_index ON keyset(unit);
CREATE INDEX active_index ON keyset(active);
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
  created_time INTEGER NOT NULL DEFAULT 0,
  paid_time INTEGER,
  payment_method TEXT NOT NULL DEFAULT 'bolt11',
  options TEXT,
  request_lookup_id_kind TEXT NOT NULL DEFAULT 'payment_hash'
);
CREATE INDEX melt_quote_state_index ON melt_quote(state);
CREATE UNIQUE INDEX unique_request_lookup_id_melt ON melt_quote(request_lookup_id);
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
  blinded_message BYTEA PRIMARY KEY,
  amount INTEGER NOT NULL,
  keyset_id TEXT NOT NULL,
  c BYTEA NOT NULL,
  dleq_e TEXT,
  dleq_s TEXT,
  quote_id TEXT,
  created_time INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS "mint_quote" (
  id TEXT PRIMARY KEY, amount INTEGER,
  unit TEXT NOT NULL, request TEXT NOT NULL,
  expiry INTEGER NOT NULL, request_lookup_id TEXT UNIQUE,
  pubkey TEXT, created_time INTEGER NOT NULL DEFAULT 0,
  amount_paid INTEGER NOT NULL DEFAULT 0,
  amount_issued INTEGER NOT NULL DEFAULT 0,
  payment_method TEXT NOT NULL DEFAULT 'BOLT11',
  request_lookup_id_kind TEXT NOT NULL DEFAULT 'payment_hash'
);
CREATE INDEX idx_mint_quote_created_time ON mint_quote(created_time);
CREATE INDEX idx_mint_quote_expiry ON mint_quote(expiry);
CREATE INDEX idx_mint_quote_request_lookup_id ON mint_quote(request_lookup_id);
CREATE INDEX idx_mint_quote_request_lookup_id_and_kind ON mint_quote(
  request_lookup_id, request_lookup_id_kind
);
CREATE TABLE mint_quote_payments (
  id SERIAL PRIMARY KEY,
  quote_id TEXT NOT NULL,
  payment_id TEXT NOT NULL UNIQUE,
  timestamp INTEGER NOT NULL,
  amount INTEGER NOT NULL,
  FOREIGN KEY (quote_id) REFERENCES mint_quote(id)
);
CREATE INDEX idx_mint_quote_payments_payment_id ON mint_quote_payments(payment_id);
CREATE INDEX idx_mint_quote_payments_quote_id ON mint_quote_payments(quote_id);
CREATE TABLE mint_quote_issued (
  id SERIAL PRIMARY KEY,
  quote_id TEXT NOT NULL,
  amount INTEGER NOT NULL,
  timestamp INTEGER NOT NULL,
  FOREIGN KEY (quote_id) REFERENCES mint_quote(id)
);
CREATE INDEX idx_mint_quote_issued_quote_id ON mint_quote_issued(quote_id);
CREATE INDEX idx_melt_quote_request_lookup_id_and_kind ON mint_quote(
  request_lookup_id, request_lookup_id_kind
);
