DROP INDEX IF EXISTS entries_tool;

CREATE INDEX entries_tool ON entries(thread, tool_session, json_extract(data, '$.data.body.data.id'))
    WHERE tool_session IS NOT NULL;
