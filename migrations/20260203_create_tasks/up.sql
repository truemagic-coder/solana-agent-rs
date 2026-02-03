CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    prompt TEXT NOT NULL,
    run_at INTEGER NOT NULL,
    interval_minutes INTEGER,
    enabled BOOLEAN NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_run_at INTEGER,
    next_run_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS scheduled_tasks_due_idx ON scheduled_tasks (enabled, next_run_at);
