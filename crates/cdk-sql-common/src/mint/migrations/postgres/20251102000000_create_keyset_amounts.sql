-- Create keyset_amounts table
CREATE TABLE IF NOT EXISTS keyset_amounts (
    keyset_id TEXT NOT NULL,
    type TEXT NOT NULL,
    amount BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (keyset_id, type)
);

-- Create index for faster lookups
CREATE INDEX IF NOT EXISTS idx_keyset_amounts_type ON keyset_amounts(type);

-- Prefill with issued amounts (sum from blind_signature where c IS NOT NULL)
INSERT INTO keyset_amounts (keyset_id, type, amount)
SELECT keyset_id, 'issued', COALESCE(SUM(amount), 0)
FROM blind_signature
WHERE c IS NOT NULL
GROUP BY keyset_id;

-- Prefill with redeemed amounts (sum from proof where state = 'SPENT')
INSERT INTO keyset_amounts (keyset_id, type, amount)
SELECT keyset_id, 'redeemed', COALESCE(SUM(amount), 0)
FROM proof
WHERE state = 'SPENT'
GROUP BY keyset_id;
