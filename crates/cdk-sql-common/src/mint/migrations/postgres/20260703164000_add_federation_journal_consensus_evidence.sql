ALTER TABLE federation_journal_entries ADD COLUMN IF NOT EXISTS operation_outcome_hash TEXT;
ALTER TABLE federation_journal_entries ADD COLUMN IF NOT EXISTS state_digest TEXT;
ALTER TABLE federation_journal_entries ADD COLUMN IF NOT EXISTS consensus_evidence_json TEXT;
