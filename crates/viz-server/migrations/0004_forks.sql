ALTER TABLE threads ADD COLUMN forked_from INTEGER;

ALTER TABLE threads ADD COLUMN forked_at INTEGER;

UPDATE threads SET session = json_set(session, '$.handoff',
    json_object('kind', 'moved', 'from', json_extract(session, '$.handoff')))
    WHERE json_type(session, '$.handoff') = 'text';
