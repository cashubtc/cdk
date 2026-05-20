-- NUT-30 (#365) renamed `selected_estimated_blocks` to `selected_fee_index`
-- and made onchain selection key off a server-assigned `fee_index` instead
-- of `estimated_blocks`. Existing onchain melt quotes are rewritten in
-- place: each row's stored `fee_options` JSON gets a `fee_index` injected
-- (assigned by array position, 0..N-1), and any `selected_estimated_blocks`
-- value is translated to the matching `fee_index` of the same option.
--
-- SQLite has no JSON_OBJECT_REPLACE, so we use json_set + json_each in a
-- correlated subquery to rewrite each element. Rows with NULL or empty
-- fee_options are left alone.
UPDATE melt_quote
SET fee_options = (
    SELECT json_group_array(
        json_object(
            'fee_index', je.key,
            'fee_reserve', json_extract(je.value, '$.fee_reserve'),
            'estimated_blocks', json_extract(je.value, '$.estimated_blocks')
        )
    )
    FROM json_each(melt_quote.fee_options) AS je
)
WHERE fee_options IS NOT NULL AND fee_options != '' AND json_array_length(fee_options) > 0;

ALTER TABLE melt_quote ADD COLUMN selected_fee_index INTEGER;

-- Backfill selected_fee_index by matching the legacy selected_estimated_blocks
-- to the rewritten fee_options array (the rewrite above preserved input order,
-- so fee_index = array position; we look up the position whose estimated_blocks
-- equals the legacy value).
UPDATE melt_quote
SET selected_fee_index = (
    SELECT je.key
    FROM json_each(melt_quote.fee_options) AS je
    WHERE json_extract(je.value, '$.estimated_blocks') = melt_quote.selected_estimated_blocks
    LIMIT 1
)
WHERE selected_estimated_blocks IS NOT NULL AND fee_options IS NOT NULL;

ALTER TABLE melt_quote DROP COLUMN selected_estimated_blocks;
