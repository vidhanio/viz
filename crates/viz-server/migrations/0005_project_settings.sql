ALTER TABLE projects ADD COLUMN settings TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(settings));
