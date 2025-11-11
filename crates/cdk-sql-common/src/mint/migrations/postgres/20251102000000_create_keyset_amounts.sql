-- Create keyset_amounts table with total_issued and total_redeemed columns
CREATE TABLE IF NOT EXISTS keyset_amounts (
    keyset_id TEXT PRIMARY KEY NOT NULL,
    total_issued BIGINT NOT NULL DEFAULT 0,
    total_redeemed BIGINT NOT NULL DEFAULT 0
);

-- Prefill with issued and redeemed amounts using FULL OUTER JOIN
INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed)
SELECT
    COALESCE(bs.keyset_id, p.keyset_id) as keyset_id,
    COALESCE(bs.total_issued, 0) as total_issued,
    COALESCE(p.total_redeemed, 0) as total_redeemed
FROM (
    SELECT keyset_id, SUM(amount) as total_issued
    FROM blind_signature
    WHERE c IS NOT NULL
    GROUP BY keyset_id
) bs
FULL OUTER JOIN (
    SELECT keyset_id, SUM(amount) as total_redeemed
    FROM proof
    WHERE state = 'SPENT'
    GROUP BY keyset_id
) p ON bs.keyset_id = p.keyset_id;
