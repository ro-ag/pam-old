CREATE TABLE activity_days (
    day_start_ms INTEGER PRIMARY KEY CHECK (day_start_ms >= 0),
    events INTEGER NOT NULL CHECK (events > 0)
) STRICT;

INSERT INTO activity_days(day_start_ms, events)
SELECT (occurred_at_ms / 86400000) * 86400000 AS day_start_ms, COUNT(*)
FROM audit_events
GROUP BY day_start_ms;
