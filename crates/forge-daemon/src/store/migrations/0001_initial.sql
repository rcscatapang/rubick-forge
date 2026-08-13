-- Registered git repositories.
CREATE TABLE projects (
    id               INTEGER PRIMARY KEY,
    name             TEXT NOT NULL,
    path             TEXT NOT NULL UNIQUE,
    default_branch   TEXT NOT NULL,
    adapter_settings TEXT NOT NULL DEFAULT '{}',
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

-- Units of work. Deleting a project takes its tasks with it; the repo on disk
-- is never touched.
CREATE TABLE tasks (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    title          TEXT NOT NULL,
    adapter        TEXT NOT NULL,
    base_branch    TEXT NOT NULL,
    branch         TEXT NOT NULL,
    worktree_path  TEXT,
    initial_prompt TEXT,
    status         TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE INDEX tasks_by_project ON tasks (project_id);
CREATE INDEX tasks_by_status ON tasks (status);

-- One row per run of a task. Restarting a task adds a row rather than reusing
-- one, so a task keeps its history.
CREATE TABLE sessions (
    id         INTEGER PRIMARY KEY,
    task_id    INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    tmux_name  TEXT NOT NULL,
    pid        INTEGER,
    status     TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at   TEXT
);

CREATE INDEX sessions_by_task ON sessions (task_id);

-- A tmux name is `forge-<task-id>`, so it repeats across a task's sessions.
-- Only one of them may be live at a time.
CREATE UNIQUE INDEX sessions_one_live_per_tmux_name
    ON sessions (tmux_name) WHERE ended_at IS NULL;

-- The activity feed. Deliberately free of foreign keys: history outlives the
-- tasks and sessions it describes.
CREATE TABLE events (
    id         INTEGER PRIMARY KEY,
    ts         TEXT NOT NULL,
    kind       TEXT NOT NULL,
    task_id    INTEGER,
    session_id INTEGER,
    payload    TEXT NOT NULL
);

CREATE INDEX events_by_task ON events (task_id);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
