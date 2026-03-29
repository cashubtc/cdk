-- RPC: atomically upsert-increment keyset counter, return new value
CREATE OR REPLACE FUNCTION increment_keyset_counter(
    p_keyset_id TEXT,
    p_increment INTEGER DEFAULT 1
)
RETURNS INTEGER
LANGUAGE sql
SECURITY DEFINER
AS $body$
    INSERT INTO keyset_counter (keyset_id, wallet_id, counter)
    VALUES (p_keyset_id, public.get_current_wallet_id(), p_increment)
    ON CONFLICT (keyset_id, wallet_id)
    DO UPDATE SET counter = keyset_counter.counter + p_increment
    RETURNING counter
$body$;
