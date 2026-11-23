ALTER TABLE runs ADD COLUMN usage TEXT;

WITH
legacy_tools AS (
    SELECT id, json_extract(data, '$.kind') AS kind,
        json_extract(data, '$.data.entry') AS original,
        CASE json_extract(data, '$.kind')
            WHEN 'tool' THEN json_extract(data, '$.data')
            ELSE json_extract(data, '$.data.record') END AS tool
    FROM entries WHERE json_extract(data, '$.kind') IN ('tool', 'tool_update')
),
calls AS (
    SELECT id, kind, original, json_object(
        'id', json_extract(tool, '$.id'), 'title', json_extract(tool, '$.title'),
        'name', json_extract(tool, '$.name'), 'kind', json_extract(tool, '$.kind'),
        'status', json_extract(tool, '$.status'), 'startedAt', json_extract(tool, '$.startedAt'),
        'rawInput', json_extract(tool, '$.input'), 'rawOutput', json_extract(tool, '$.output'),
        'locations', json((SELECT json_group_array(json_object('path', value, 'line', NULL))
            FROM json_each(tool, '$.locations'))),
        'content', json((SELECT json_group_array(json_object('type', 'diff', 'data', json(value)))
            FROM json_each(tool, '$.diffs'))),
        'terminals', json_array(),
        'viz', CASE WHEN json_extract(tool, '$.viz') IS NOT NULL
            OR json_extract(tool, '$.child') IS NOT NULL
            OR json_extract(tool, '$.childEnvironment') IS NOT NULL
            THEN json_object('tool', coalesce(json_extract(tool, '$.viz'), ''),
                'child', json_extract(tool, '$.child'),
                'environment', json_extract(tool, '$.childEnvironment')) END
    ) AS call FROM legacy_tools
),
transformed AS (
    SELECT id, CASE kind WHEN 'tool_update' THEN json_object(
        'kind', 'tool_update', 'data', json_object('entry', original, 'record', json(call)))
    ELSE json_object('kind', 'transcript', 'data', json_object(
        'key', 'legacy:' || id, 'order', id,
        'body', json_object('type', 'tool', 'data', json(call)))) END AS data
    FROM calls
)
UPDATE entries SET data = transformed.data FROM transformed WHERE entries.id = transformed.id;

UPDATE entries
SET data = json_object('kind', 'transcript', 'data', json_object(
    'key', coalesce(json_extract(data, '$.data.key'), 'legacy:' || id),
    'order', id,
    'body', json_object('type', json_extract(data, '$.kind'), 'data',
        CASE json_extract(data, '$.kind')
            WHEN 'plan' THEN json_object('content', json_object(
                'type', 'items', 'entries', json_extract(data, '$.data.entries')))
            ELSE json_object('text', json_extract(data, '$.data.text'),
                'message', coalesce(json_extract(data, '$.data.key'), 'legacy:' || id))
        END)))
WHERE json_extract(data, '$.kind') IN ('text', 'thought', 'plan');
