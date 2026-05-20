-- NUT-30 (#365) renamed `selected_estimated_blocks` to `selected_fee_index`
-- and made onchain selection key off a server-assigned `fee_index` instead
-- of `estimated_blocks`. Existing onchain melt quotes are rewritten in
-- place: each row's stored `fee_options` JSON gets a `fee_index` injected
-- (assigned by array position, 0..N-1), and any `selected_estimated_blocks`
-- value is translated to the matching `fee_index` of the same option.

ALTER TABLE melt_quote ADD COLUMN selected_fee_index INTEGER;

UPDATE melt_quote
SET fee_options = sub.rewritten
FROM (
    SELECT id,
           jsonb_agg(
               jsonb_build_object(
                   'fee_index', (e.ord - 1)::int,
                   'fee_reserve', e.elem->'fee_reserve',
                   'estimated_blocks', e.elem->'estimated_blocks'
               )
               ORDER BY e.ord
           )::text AS rewritten
    FROM melt_quote q,
         jsonb_array_elements(q.fee_options::jsonb) WITH ORDINALITY AS e(elem, ord)
    WHERE q.fee_options IS NOT NULL
      AND q.fee_options <> ''
      AND jsonb_array_length(q.fee_options::jsonb) > 0
    GROUP BY id
) AS sub
WHERE melt_quote.id = sub.id;

UPDATE melt_quote q
SET selected_fee_index = sub.matched
FROM (
    SELECT q.id, (e.ord - 1)::int AS matched
    FROM melt_quote q,
         jsonb_array_elements(q.fee_options::jsonb) WITH ORDINALITY AS e(elem, ord)
    WHERE q.selected_estimated_blocks IS NOT NULL
      AND (e.elem->>'estimated_blocks')::int = q.selected_estimated_blocks
) AS sub
WHERE q.id = sub.id;

ALTER TABLE melt_quote DROP COLUMN selected_estimated_blocks;
