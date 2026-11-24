ALTER TABLE runs RENAME COLUMN text TO prompt;
ALTER TABLE runs DROP COLUMN attachments;
ALTER TABLE runs DROP COLUMN context;

CREATE TABLE fork_requests (
    request_id TEXT PRIMARY KEY NOT NULL,
    thread INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    prompt TEXT
) STRICT;
