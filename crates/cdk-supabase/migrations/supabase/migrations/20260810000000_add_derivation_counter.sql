CREATE TABLE IF NOT EXISTS derivation_counter (
    wallet_id TEXT NOT NULL DEFAULT public.get_current_wallet_id(),
    namespace TEXT NOT NULL,
    counter BIGINT NOT NULL DEFAULT 0 CHECK (counter >= 0),
    PRIMARY KEY (wallet_id, namespace)
);

ALTER TABLE derivation_counter ENABLE ROW LEVEL SECURITY;

CREATE POLICY "Users access own derivation counters" ON derivation_counter
    FOR ALL USING (wallet_id = public.get_current_wallet_id());

GRANT ALL ON derivation_counter TO authenticated;

CREATE OR REPLACE FUNCTION increment_derivation_counter(
    p_namespace TEXT,
    p_increment BIGINT DEFAULT 1
)
RETURNS BIGINT
LANGUAGE sql
SECURITY DEFINER
AS $body$
    INSERT INTO derivation_counter (wallet_id, namespace, counter)
    VALUES (public.get_current_wallet_id(), p_namespace, p_increment)
    ON CONFLICT (wallet_id, namespace)
    DO UPDATE SET counter = derivation_counter.counter + p_increment
    RETURNING counter
$body$;

INSERT INTO schema_info (key, value) VALUES ('schema_version', '9')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
