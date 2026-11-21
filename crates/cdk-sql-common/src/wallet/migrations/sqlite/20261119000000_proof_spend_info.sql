-- v3 (nutroot) proofs: the spend info a token or payment carried, as JSON,
-- kept until the proof is swept or sent on.
ALTER TABLE proof ADD COLUMN spend_info TEXT;
