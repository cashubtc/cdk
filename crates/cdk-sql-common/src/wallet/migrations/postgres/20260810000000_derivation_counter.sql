CREATE TABLE IF NOT EXISTS derivation_counter (
    namespace TEXT PRIMARY KEY,
    counter BIGINT NOT NULL DEFAULT 0 CHECK (counter >= 0)
);
