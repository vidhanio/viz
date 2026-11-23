ALTER TABLE threads ADD COLUMN settled_at INTEGER;

ALTER TABLE threads ADD COLUMN settled_by TEXT CHECK (settled_by IN ('user', 'inactivity', 'pull_request'));

ALTER TABLE threads ADD COLUMN kept INTEGER NOT NULL DEFAULT 0 CHECK (kept IN (0, 1));

ALTER TABLE threads ADD COLUMN snoozed_until INTEGER;

ALTER TABLE threads ADD COLUMN pin INTEGER;

ALTER TABLE threads ADD COLUMN sort_order INTEGER;

ALTER TABLE threads ADD COLUMN auto_settle TEXT NOT NULL DEFAULT 'follow' CHECK (auto_settle IN ('follow', 'never'));

ALTER TABLE threads ADD COLUMN last_input_at INTEGER;

UPDATE threads SET pin = ranked.position FROM
    (SELECT id, row_number() OVER (ORDER BY updated_at DESC, id DESC) - 1 AS position FROM threads WHERE pinned = 1) AS ranked
    WHERE threads.id = ranked.id;

ALTER TABLE threads DROP COLUMN pinned;
