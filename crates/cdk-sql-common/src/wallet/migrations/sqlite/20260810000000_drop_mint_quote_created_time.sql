-- Drop the unused created_time column from mint_quote: added for ordering queries
-- that were never implemented, it has never been written or read.
ALTER TABLE mint_quote DROP COLUMN created_time;
