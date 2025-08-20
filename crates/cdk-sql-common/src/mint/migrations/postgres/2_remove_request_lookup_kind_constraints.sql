-- Set existing NULL or empty request_lookup_id_kind values to 'payment_hash' in melt_quote
UPDATE melt_quote SET request_lookup_id_kind = 'payment_hash' WHERE request_lookup_id_kind IS NULL OR request_lookup_id_kind = '';

-- Remove NOT NULL constraint and default value from request_lookup_id_kind in melt_quote table  
ALTER TABLE melt_quote ALTER COLUMN request_lookup_id_kind DROP NOT NULL;
ALTER TABLE melt_quote ALTER COLUMN request_lookup_id_kind DROP DEFAULT;
