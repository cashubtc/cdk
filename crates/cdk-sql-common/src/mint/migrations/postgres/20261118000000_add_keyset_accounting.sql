CREATE TABLE keyset_accounting (
    event_id TEXT PRIMARY KEY NOT NULL,
    keyset_id TEXT NOT NULL,
    total_issued BIGINT NOT NULL DEFAULT 0 CHECK (total_issued >= 0),
    total_redeemed BIGINT NOT NULL DEFAULT 0 CHECK (total_redeemed >= 0),
    fee_collected BIGINT NOT NULL DEFAULT 0 CHECK (fee_collected >= 0)
);

CREATE INDEX keyset_accounting_keyset_id ON keyset_accounting (keyset_id);

CREATE VIEW keyset_accounting_totals AS
SELECT keyset_id,
    CAST(SUM(total_issued) AS BIGINT) AS total_issued,
    CAST(SUM(total_redeemed) AS BIGINT) AS total_redeemed,
    CAST(SUM(fee_collected) AS BIGINT) AS fee_collected
FROM (
    SELECT keyset_id, total_issued, total_redeemed, fee_collected FROM keyset_amounts
    UNION ALL
    SELECT keyset_id, total_issued, total_redeemed, fee_collected FROM keyset_accounting
) AS amounts
GROUP BY keyset_id;
