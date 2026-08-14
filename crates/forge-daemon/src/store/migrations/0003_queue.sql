-- The hub's task queue.
--
-- Only the always-on daemon with `hub = true` ever has rows here; a worker's
-- copy of this table stays empty and costs nothing.
CREATE TABLE queued_tasks (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Matched against each machine's projects by *name*, not path: the same
    -- repository is checked out somewhere different on every Mac (D25).
    project_name  TEXT NOT NULL,
    adapter       TEXT NOT NULL,
    title         TEXT NOT NULL,
    prompt        TEXT,

    -- A machine name, or NULL for "whichever machine can take it".
    target        TEXT,

    -- 'queued', 'dispatched' or 'cancelled'.
    state         TEXT NOT NULL DEFAULT 'queued',

    -- Why a queued row is still queued, for a UI that has to explain itself.
    reason        TEXT,

    -- Where it went, once it went.
    machine       TEXT,
    remote_task   INTEGER,

    -- Which machines could have taken it, recorded before the choice so the
    -- placement decision is auditable afterwards (requirement 3).
    considered    TEXT,

    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
) STRICT;

-- The dispatch loop asks for exactly this.
CREATE INDEX queued_tasks_pending ON queued_tasks(state) WHERE state = 'queued';
