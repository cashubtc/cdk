CREATE TABLE proof_new (
    y BLOB PRIMARY KEY,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL, -- no FK constraint here
    secret TEXT NOT NULL,
    c BLOB NOT NULL,
    witness TEXT,
    state TEXT CHECK (state IN ('SPENT', 'PENDING', 'UNSPENT', 'RESERVED', 'UNKNOWN')) NOT NULL,
    quote_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0
);

INSERT INTO proof_new (y, amount, keyset_id, secret, c, witness, state, quote_id, created_time) SELECT y, amount, keyset_id, secret, c, witness, state, quote_id, created_time FROM proof;
DROP TABLE proof;
ALTER TABLE proof_new RENAME TO proof;


CREATE TABLE blind_signature_new (
    y BLOB PRIMARY KEY,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL,  -- FK removed
    c BLOB NOT NULL,
    dleq_e TEXT,
    dleq_s TEXT,
    quote_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0
);

INSERT INTO blind_signature_new (y, amount, keyset_id, c, dleq_e, dleq_s, quote_id, created_time) SELECT y, amount, keyset_id, c, dleq_e, dleq_s, quote_id, created_time FROM blind_signature;
DROP TABLE blind_signature;
ALTER TABLE blind_signature_new RENAME TO blind_signature;
