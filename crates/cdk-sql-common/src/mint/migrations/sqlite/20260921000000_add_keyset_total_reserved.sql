-- Proofs the mint is holding but has not yet burned. Tracking them lets the
-- per-keyset cap be checked when the mint takes custody, rather than after a
-- melt has already paid the invoice.
ALTER TABLE keyset_amounts ADD COLUMN total_reserved INTEGER NOT NULL DEFAULT 0;
