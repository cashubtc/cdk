ALTER TABLE federation_journal_entries ADD COLUMN IF NOT EXISTS operation_outcome_hash TEXT NOT NULL;
ALTER TABLE federation_journal_entries ADD COLUMN IF NOT EXISTS state_digest TEXT NOT NULL;
ALTER TABLE federation_journal_entries ADD COLUMN IF NOT EXISTS consensus_evidence_json TEXT;
