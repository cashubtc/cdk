ALTER TABLE federation_journal_entries ADD COLUMN operation_outcome_hash TEXT NOT NULL;
ALTER TABLE federation_journal_entries ADD COLUMN state_digest TEXT NOT NULL;
ALTER TABLE federation_journal_entries ADD COLUMN consensus_evidence_json TEXT;
