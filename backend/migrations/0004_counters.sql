-- Gap-free human-readable numbers.
-- YugabyteDB hands out sequence values in per-connection blocks, so consecutive tasks
-- could get #501 and #601. A counter row updated inside the creating transaction keeps
-- numbers consecutive. Contention on this row is negligible at company scale.
CREATE TABLE counters (
    name   TEXT PRIMARY KEY,
    value  BIGINT NOT NULL
);
INSERT INTO counters (name, value) SELECT 'task_number', COALESCE(MAX(number), 0) FROM tasks;
