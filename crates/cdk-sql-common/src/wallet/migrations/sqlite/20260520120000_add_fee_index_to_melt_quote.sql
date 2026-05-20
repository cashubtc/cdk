-- NUT-30 (#365) introduced server-assigned `fee_index` as the selection
-- key for onchain melt quotes. The wallet stores it alongside the
-- existing `estimated_blocks` mirror so we can echo the right field back
-- when executing the quote.
ALTER TABLE melt_quote ADD COLUMN fee_index INTEGER;
