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
INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, total_reserved)
SELECT
    COALESCE(bs.keyset_id, p.keyset_id),
    COALESCE(bs.total_issued, 0),
    COALESCE(p.total_redeemed, 0),
    COALESCE(p.total_reserved, 0)
FROM (
    SELECT keyset_id, SUM(amount) AS total_issued
    FROM blind_signature
    WHERE c IS NOT NULL
    GROUP BY keyset_id
) bs
FULL OUTER JOIN (
    SELECT
        keyset_id,
        SUM(amount) FILTER (WHERE state = 'SPENT')  AS total_redeemed,
        SUM(amount) FILTER (WHERE state <> 'SPENT') AS total_reserved
    FROM proof
    GROUP BY keyset_id
) p ON bs.keyset_id = p.keyset_id
ON CONFLICT (keyset_id) DO UPDATE SET
    total_reserved = EXCLUDED.total_reserved;
