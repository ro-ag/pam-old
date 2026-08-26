ALTER TABLE callers ADD COLUMN kind TEXT
    CHECK (kind IS NULL OR kind IN ('cli', 'gui', 'coding-agent', 'local-application'));
