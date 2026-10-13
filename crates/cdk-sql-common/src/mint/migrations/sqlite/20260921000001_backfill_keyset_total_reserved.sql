-- total_reserved is a new column, so it has no stored value to preserve and has
-- to be derived from the proofs the mint is currently holding.
--
-- total_issued and total_redeemed are left alone. Re-deriving them would lower
-- the cap on any mint whose issuance history is incomplete, a restore from a
-- partial backup or an import from another implementation, and freeze proofs
-- the mint really did issue. The counters the mint already keeps are the better
-- evidence, and proofs in custody were counted in neither of them before, so
-- the reservation only takes up headroom that was already accounted for.
--
-- A keyset with no row yet has nothing to preserve, so it is seeded from the
-- source tables.
INSERT OR IGNORE INTO keyset_amounts (keyset_id, total_issued, total_redeemed, total_reserved)
SELECT
    k.keyset_id,
    (
        SELECT COALESCE(SUM(amount), 0)
        FROM blind_signature
        WHERE blind_signature.keyset_id = k.keyset_id
          AND blind_signature.c IS NOT NULL
    ),
    (
        SELECT COALESCE(SUM(amount), 0)
        FROM proof
        WHERE proof.keyset_id = k.keyset_id
          AND proof.state = 'SPENT'
    ),
    0
FROM (
    SELECT keyset_id FROM blind_signature WHERE c IS NOT NULL
    UNION
    SELECT keyset_id FROM proof
) k;

UPDATE keyset_amounts
SET total_reserved = (
        SELECT COALESCE(SUM(amount), 0)
        FROM proof
        WHERE proof.keyset_id = keyset_amounts.keyset_id
          AND proof.state <> 'SPENT'
    );
