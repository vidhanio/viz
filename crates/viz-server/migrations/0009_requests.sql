CREATE TABLE run_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    run INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    body TEXT NOT NULL,
    state TEXT NOT NULL,
    pending INTEGER NOT NULL GENERATED ALWAYS AS (json_extract(state, '$.type') = 'pending') VIRTUAL
) STRICT;

CREATE INDEX run_requests_pending ON run_requests(thread, id) WHERE pending;
CREATE INDEX run_requests_run ON run_requests(run);

ALTER TABLE threads ADD COLUMN access TEXT;

CREATE TABLE project_access (
    project TEXT NOT NULL REFERENCES projects(path) ON DELETE CASCADE ON UPDATE CASCADE,
    harness TEXT NOT NULL,
    access TEXT NOT NULL,
    PRIMARY KEY (project, harness)
) STRICT;
