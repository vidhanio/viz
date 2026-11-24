CREATE TABLE blobs (
    id TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mime TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE thread_attachments (
    thread INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    blob TEXT NOT NULL REFERENCES blobs(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    mime TEXT NOT NULL,
    PRIMARY KEY (thread, id)
) STRICT;

CREATE INDEX thread_attachments_blob ON thread_attachments(blob);
