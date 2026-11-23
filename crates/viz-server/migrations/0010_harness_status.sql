CREATE TABLE harness_status (
    name TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    checked_at INTEGER NOT NULL
) STRICT;
