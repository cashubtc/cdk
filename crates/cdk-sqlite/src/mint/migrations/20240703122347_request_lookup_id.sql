ALTER TABLE mint_quote ADD request_lookup_id TEXT UNIQUE;
ALTER TABLE melt_quote ADD request_lookup_id TEXT UNIQUE;

CREATE INDEX IF NOT EXISTS mint_quote_request_lookup_id_index ON mint_quote(request_lookup_id);
CREATE INDEX IF NOT EXISTS melt_quote_request_lookup_id_index ON melt_quote(request_lookup_id);
