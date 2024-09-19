-- Melt Request Table
CREATE TABLE IF NOT EXISTS melt_request (
id TEXT PRIMARY KEY,
inputs TEXT NOT NULL,
outputs TEXT,
method TEXT NOT NULL,
unit TEXT NOT NULL
);
