-- NUT-07 (v3): the input digest a v3 proof's witness signed, returned with the
-- witness while the proof is pending or spent.
ALTER TABLE proof ADD COLUMN input_digest TEXT;
