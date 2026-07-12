CREATE TABLE IF NOT EXISTS derivation_counter (
    namespace TEXT PRIMARY KEY,
    counter INTEGER NOT NULL DEFAULT 0 CHECK (counter >= 0)
);
