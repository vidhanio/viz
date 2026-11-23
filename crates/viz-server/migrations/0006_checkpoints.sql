CREATE TABLE run_checkpoints (
    run INTEGER PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    git_dir TEXT NOT NULL,
    before_ref TEXT NOT NULL,
    after_ref TEXT
) STRICT;
