ALTER TABLE agent_tasks ADD COLUMN dismissed INTEGER NOT NULL DEFAULT 0 CHECK (dismissed IN (0, 1));

DELETE FROM agent_tasks WHERE phase = 'settled' AND parent NOT IN (SELECT id FROM threads);
