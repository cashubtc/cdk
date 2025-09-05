CREATE TABLE keyset_new (
  id TEXT PRIMARY KEY,
  unit TEXT NOT NULL,
  active BOOL NOT NULL,
  valid_from INTEGER NOT NULL,
  valid_to INTEGER,
  max_order INTEGER NOT NULL,
  amounts TEXT DEFAULT NULL,
  input_fee_ppk INTEGER,
  derivation_path TEXT NOT NULL,
  derivation_path_index INTEGER
);


INSERT INTO keyset_new SELECT
    id,
    unit,
    active,
    valid_from,
    valid_to,
    max_order,
    NULL,
    input_fee_ppk,
    derivation_path,
    derivation_path_index
FROM keyset;

DROP TABLE keyset;

ALTER TABLE keyset_new RENAME TO keyset;

CREATE INDEX unit_index ON keyset(unit);
CREATE INDEX active_index ON keyset(active);
