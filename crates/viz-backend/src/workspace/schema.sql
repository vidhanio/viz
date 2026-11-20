-- One database and one writer per environment.
CREATE TABLE IF NOT EXISTS projects (
    path TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS threads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    user_title INTEGER NOT NULL,
    harness TEXT NOT NULL,
    cwd TEXT NOT NULL,
    project TEXT REFERENCES projects(path) ON DELETE SET NULL,
    parent INTEGER,
    parent_environment TEXT,
    archived INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    unread INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    session TEXT NOT NULL,
    model TEXT,
    thinking TEXT,
    importing INTEGER NOT NULL DEFAULT 0,
    setup_failed INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX IF NOT EXISTS threads_project ON threads(project);

CREATE INDEX IF NOT EXISTS thread_session ON threads(harness, cwd, json_extract(session, '$.session_id'));

CREATE TABLE IF NOT EXISTS entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    data TEXT NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS entries_thread ON entries(thread, id);

CREATE VIRTUAL TABLE IF NOT EXISTS search_entries USING fts5(text, thread UNINDEXED);

CREATE TRIGGER IF NOT EXISTS entry_search_delete AFTER DELETE ON entries BEGIN
    DELETE FROM search_entries WHERE rowid = old.id;
END;

CREATE TABLE IF NOT EXISTS runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled', 'interrupted')),
    position INTEGER NOT NULL,
    request_id TEXT,
    text TEXT NOT NULL,
    origin TEXT,
    attachments TEXT,
    prompt_entry INTEGER,
    end_entry INTEGER,
    answer TEXT,
    error TEXT,
    UNIQUE (thread, request_id)
) STRICT;

CREATE INDEX IF NOT EXISTS runs_thread ON runs(thread, id DESC);

CREATE INDEX IF NOT EXISTS runs_queue ON runs(thread, position, id) WHERE status = 'queued';

CREATE INDEX IF NOT EXISTS runs_settled ON runs(thread, end_entry DESC) WHERE end_entry IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS runs_active ON runs(thread) WHERE status = 'running';

CREATE TABLE IF NOT EXISTS attachments (
    thread INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    mime TEXT NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (thread, id)
) STRICT;

-- A receipt survives thread deletion: an idempotent replay cannot allocate a second thread or run.
CREATE TABLE IF NOT EXISTS launch_requests (
    request_id TEXT PRIMARY KEY,
    thread INTEGER NOT NULL,
    run INTEGER
) STRICT;

CREATE TABLE IF NOT EXISTS task_receipts (
    environment TEXT NOT NULL,
    task TEXT NOT NULL,
    thread INTEGER NOT NULL,
    run INTEGER NOT NULL,
    PRIMARY KEY (environment, task)
) STRICT;

CREATE TABLE IF NOT EXISTS task_retractions (
    environment TEXT NOT NULL,
    task TEXT NOT NULL,
    PRIMARY KEY (environment, task)
) STRICT;

CREATE TABLE IF NOT EXISTS task_wakes (
    thread INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    task TEXT NOT NULL,
    run INTEGER,
    PRIMARY KEY (thread, task)
) STRICT;

CREATE TABLE IF NOT EXISTS harness_catalogs (
    name TEXT PRIMARY KEY,
    data TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS agent_tasks (
    id TEXT PRIMARY KEY,
    parent INTEGER NOT NULL,
    parent_run INTEGER,
    environment TEXT NOT NULL,
    thread INTEGER,
    run INTEGER,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    title TEXT NOT NULL,
    result TEXT,
    error TEXT,
    payload TEXT,
    request_id TEXT,
    phase TEXT NOT NULL CHECK (phase IN ('pending', 'offered', 'retracting', 'accepted', 'cancelling', 'settled')),
    delivery TEXT NOT NULL CHECK (delivery IN ('prompt', 'entry_only', 'delivered')),
    wake_run INTEGER,
    CHECK ((thread IS NULL) = (run IS NULL))
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS agent_tasks_request ON agent_tasks (parent, request_id) WHERE request_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS agent_tasks_pending ON agent_tasks (id) WHERE delivery != 'delivered';

CREATE INDEX IF NOT EXISTS agent_tasks_family ON agent_tasks (parent, parent_run, wake_run);

CREATE TABLE IF NOT EXISTS stopped_task_runs (
    thread INTEGER NOT NULL,
    run INTEGER NOT NULL,
    handled INTEGER NOT NULL CHECK (handled IN (0, 1)),
    PRIMARY KEY (thread, run)
) STRICT;

CREATE INDEX IF NOT EXISTS stopped_task_runs_pending ON stopped_task_runs (thread, run) WHERE handled = 0;
