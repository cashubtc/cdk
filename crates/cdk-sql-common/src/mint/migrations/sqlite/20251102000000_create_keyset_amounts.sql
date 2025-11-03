-- Create keyset_amounts table with total_issued and total_redeemed columns
CREATE TABLE IF NOT EXISTS keyset_amounts (
    keyset_id TEXT PRIMARY KEY NOT NULL,
    total_issued INTEGER NOT NULL DEFAULT 0,
    total_redeemed INTEGER NOT NULL DEFAULT 0
);

-- Prefill with issued amounts
INSERT OR IGNORE INTO keyset_amounts (keyset_id, total_issued, total_redeemed)
SELECT keyset_id, SUM(amount) as total_issued, 0 as total_redeemed
FROM blind_signature
WHERE c IS NOT NULL
GROUP BY keyset_id;

-- Update with redeemed amounts
UPDATE keyset_amounts
SET total_redeemed = (
    SELECT COALESCE(SUM(amount), 0)
    FROM proof
    WHERE proof.keyset_id = keyset_amounts.keyset_id
    AND proof.state = 'SPENT'
);

-- Insert keysets that only have redeemed amounts (no issued)
INSERT OR IGNORE INTO keyset_amounts (keyset_id, total_issued, total_redeemed)
SELECT keyset_id, 0 as total_issued, SUM(amount) as total_redeemed
FROM proof
WHERE state = 'SPENT'
AND keyset_id NOT IN (SELECT keyset_id FROM keyset_amounts)
GROUP BY keyset_id;
