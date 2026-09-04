-- A mint URL is a mutable attribute of a mint, not its identity. Keying every
-- table on the URL meant a mint that moves (NUT-06 `urls`) had to have each of
-- those tables rewritten, and any table missed left rows stranded under the old
-- URL. Tables now reference an internal mint id that never changes, so moving a
-- mint is a single UPDATE on `mint`.

PRAGMA foreign_keys=off;

-- Rows could reference a mint URL that was never added to `mint`. Give those a
-- mint row so nothing is stranded once the reference becomes an id.
INSERT INTO mint (mint_url)
SELECT mint_url FROM (
    SELECT mint_url FROM keyset
    UNION SELECT mint_url FROM proof
    UNION SELECT mint_url FROM mint_quote
    UNION SELECT mint_url FROM melt_quote WHERE mint_url IS NOT NULL
    UNION SELECT mint_url FROM transactions
    UNION SELECT mint_url FROM wallet_sagas
)
WHERE mint_url NOT IN (SELECT mint_url FROM mint);

CREATE TABLE mint_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    mint_url TEXT NOT NULL UNIQUE,
    name TEXT,
    pubkey BLOB,
    version TEXT,
    description TEXT,
    description_long TEXT,
    contact TEXT,
    nuts TEXT,
    motd TEXT,
    icon_url TEXT,
    mint_time INTEGER,
    urls TEXT,
    tos_url TEXT
);

INSERT INTO mint_new (
    mint_url, name, pubkey, version, description, description_long,
    contact, nuts, motd, icon_url, mint_time, urls, tos_url
)
SELECT
    mint_url, name, pubkey, version, description, description_long,
    contact, nuts, motd, icon_url, mint_time, urls, tos_url
FROM mint;

DROP TABLE mint;
ALTER TABLE mint_new RENAME TO mint;

-- keyset carries a foreign key on mint_url, so the column cannot be dropped in
-- place; the table is rebuilt instead.
CREATE TABLE keyset_new (
    id TEXT PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    keyset_u32 INTEGER,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    input_fee_ppk INTEGER,
    final_expiry INTEGER DEFAULT NULL,
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE CASCADE
);

INSERT INTO keyset_new (id, mint_id, keyset_u32, unit, active, input_fee_ppk, final_expiry)
SELECT k.id, m.id, k.keyset_u32, k.unit, k.active, k.input_fee_ppk, k.final_expiry
FROM keyset k
JOIN mint m ON m.mint_url = k.mint_url;

DROP TABLE keyset;
ALTER TABLE keyset_new RENAME TO keyset;

CREATE UNIQUE INDEX IF NOT EXISTS keyset_u32_unique_keyset ON keyset(keyset_u32);
CREATE INDEX IF NOT EXISTS keyset_mint_id_index ON keyset(mint_id);

-- SQLite refuses to drop an indexed column.
DROP INDEX IF EXISTS mint_url_index;
DROP INDEX IF EXISTS wallet_sagas_mint_url_index;

ALTER TABLE proof ADD COLUMN mint_id INTEGER;
UPDATE proof SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = proof.mint_url);
ALTER TABLE proof DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS proof_mint_id_index ON proof(mint_id);

ALTER TABLE mint_quote ADD COLUMN mint_id INTEGER;
UPDATE mint_quote SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = mint_quote.mint_url);
ALTER TABLE mint_quote DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS mint_quote_mint_id_index ON mint_quote(mint_id);

ALTER TABLE melt_quote ADD COLUMN mint_id INTEGER;
UPDATE melt_quote SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = melt_quote.mint_url);
ALTER TABLE melt_quote DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS melt_quote_mint_id_index ON melt_quote(mint_id);

ALTER TABLE transactions ADD COLUMN mint_id INTEGER;
UPDATE transactions SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = transactions.mint_url);
ALTER TABLE transactions DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS transactions_mint_id_index ON transactions(mint_id);

ALTER TABLE wallet_sagas ADD COLUMN mint_id INTEGER;
UPDATE wallet_sagas SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = wallet_sagas.mint_url);
ALTER TABLE wallet_sagas DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS wallet_sagas_mint_id_index ON wallet_sagas(mint_id);

PRAGMA foreign_keys=on;
