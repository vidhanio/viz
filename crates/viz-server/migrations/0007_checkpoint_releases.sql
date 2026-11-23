ALTER TABLE run_checkpoints ADD COLUMN common_dir TEXT;

CREATE TABLE checkpoint_releases (
    ref_id TEXT PRIMARY KEY,
    git_dir TEXT NOT NULL
) STRICT;
