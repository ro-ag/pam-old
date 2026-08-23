CREATE TABLE connectors (
    connector_id TEXT PRIMARY KEY NOT NULL
        CHECK (length(CAST(connector_id AS BLOB)) BETWEEN 1 AND 128),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    base_url TEXT
        CHECK (base_url IS NULL OR length(CAST(base_url AS BLOB)) BETWEEN 9 AND 1024),
    last_test_status TEXT
        CHECK (last_test_status IS NULL OR last_test_status IN ('passed', 'failed')),
    last_test_at_ms INTEGER CHECK (last_test_at_ms IS NULL OR last_test_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;
