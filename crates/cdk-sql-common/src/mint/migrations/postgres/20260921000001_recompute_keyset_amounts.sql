-- Re-derive every counter from the rows that are authoritative for it, so the
-- cap starts from a balance the database can vouch for instead of one that has
-- only ever been maintained incrementally. fee_collected is not derivable and
-- is never named here.
--
-- The zeroing pass is needed because the join below cannot reach a keyset_amounts
-- row whose source rows have all gone away.
UPDATE keyset_amounts SET total_issued = 0, total_redeemed = 0, total_reserved = 0;

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
    total_issued   = EXCLUDED.total_issued,
    total_redeemed = EXCLUDED.total_redeemed,
    total_reserved = EXCLUDED.total_reserved;
