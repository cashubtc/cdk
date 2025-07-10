ALTER TABLE mint_quote ADD COLUMN request_lookup_id TEXT;
ALTER TABLE melt_quote ADD COLUMN request_lookup_id TEXT;

CREATE UNIQUE INDEX unique_request_lookup_id_mint ON mint_quote(request_lookup_id);
CREATE UNIQUE INDEX unique_request_lookup_id_melt ON melt_quote(request_lookup_id);
