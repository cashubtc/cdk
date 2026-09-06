-- Compact state filters: parameters, pending elements, and built filters.

CREATE TABLE IF NOT EXISTS state_filter_config (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    genesis INTEGER NOT NULL,
    epoch_seconds INTEGER NOT NULL,
    p INTEGER NOT NULL,
    page_size INTEGER NOT NULL
);

-- The primary key deduplicates elements within an epoch on insert.
CREATE TABLE IF NOT EXISTS state_filter_element (
    epoch INTEGER NOT NULL,
    element BLOB NOT NULL,
    PRIMARY KEY (epoch, element)
);

CREATE TABLE IF NOT EXISTS state_filter (
    epoch INTEGER PRIMARY KEY,
    start_time INTEGER NOT NULL,
    end_time INTEGER NOT NULL,
    data BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_state_filter_start ON state_filter(start_time);
