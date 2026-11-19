-- v3 (nutroot) proofs: the spend info a token or payment carried, as JSON.
ALTER TABLE proof ADD COLUMN IF NOT EXISTS spend_info TEXT;
