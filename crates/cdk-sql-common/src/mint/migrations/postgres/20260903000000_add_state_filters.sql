-- Compact state filters: parameters, pending elements, and built filters.

CREATE TABLE IF NOT EXISTS state_filter_config (
    id BIGINT PRIMARY KEY CHECK (id = 0),
    genesis BIGINT NOT NULL,
    epoch_seconds BIGINT NOT NULL,
    p BIGINT NOT NULL,
    page_size BIGINT NOT NULL
);

-- The primary key deduplicates elements within an epoch on insert.
CREATE TABLE IF NOT EXISTS state_filter_element (
    epoch BIGINT NOT NULL,
    element BYTEA NOT NULL,
    PRIMARY KEY (epoch, element)
);

CREATE TABLE IF NOT EXISTS state_filter (
    epoch BIGINT PRIMARY KEY,
    start_time BIGINT NOT NULL,
    end_time BIGINT NOT NULL,
    data BYTEA NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_state_filter_start ON state_filter(start_time);
