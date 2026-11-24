ALTER TABLE entries ADD COLUMN rev INTEGER NOT NULL DEFAULT 0;

CREATE INDEX entries_thread_visible
    ON entries(thread, id)
    WHERE json_extract(data, '$.kind') != 'tool_update';

CREATE INDEX entries_thread_turn_starts
    ON entries(thread, id)
    WHERE json_extract(data, '$.kind') IN ('prompt', 'continued');

ALTER TABLE blobs ADD COLUMN preview TEXT REFERENCES blobs(id) ON DELETE SET NULL;
ALTER TABLE blobs ADD COLUMN preview_mime TEXT;
CREATE INDEX blobs_preview ON blobs(preview) WHERE preview IS NOT NULL;

ALTER TABLE projects ADD COLUMN icon_blob TEXT REFERENCES blobs(id) ON DELETE SET NULL;
ALTER TABLE projects ADD COLUMN icon_mime TEXT;
ALTER TABLE projects ADD COLUMN icon_fetched_at INTEGER;
ALTER TABLE projects ADD COLUMN icon_mode TEXT;

CREATE TABLE thread_pulls (
    thread INTEGER PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
    cwd TEXT NOT NULL,
    remote_host TEXT NOT NULL,
    remote_owner TEXT NOT NULL,
    remote_repo TEXT NOT NULL,
    pull_json TEXT NOT NULL,
    finished_at INTEGER
) STRICT;
