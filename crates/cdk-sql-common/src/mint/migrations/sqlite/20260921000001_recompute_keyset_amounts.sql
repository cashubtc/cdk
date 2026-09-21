-- Re-derive every counter from the rows that are authoritative for it, so the
-- cap starts from a balance the database can vouch for instead of one that has
-- only ever been maintained incrementally. fee_collected is not derivable and
-- is never named here.
INSERT OR IGNORE INTO keyset_amounts (keyset_id, total_issued, total_redeemed, total_reserved)
SELECT keyset_id, 0, 0, 0 FROM blind_signature WHERE c IS NOT NULL GROUP BY keyset_id;

INSERT OR IGNORE INTO keyset_amounts (keyset_id, total_issued, total_redeemed, total_reserved)
SELECT keyset_id, 0, 0, 0 FROM proof GROUP BY keyset_id;

UPDATE keyset_amounts
SET total_issued = (
        SELECT COALESCE(SUM(amount), 0)
        FROM blind_signature
        WHERE blind_signature.keyset_id = keyset_amounts.keyset_id
          AND blind_signature.c IS NOT NULL
    ),
    total_redeemed = (
        SELECT COALESCE(SUM(amount), 0)
        FROM proof
        WHERE proof.keyset_id = keyset_amounts.keyset_id
          AND proof.state = 'SPENT'
    ),
    total_reserved = (
        SELECT COALESCE(SUM(amount), 0)
        FROM proof
        WHERE proof.keyset_id = keyset_amounts.keyset_id
          AND proof.state <> 'SPENT'
    );
