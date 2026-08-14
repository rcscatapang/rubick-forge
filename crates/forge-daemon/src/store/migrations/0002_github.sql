-- A task's link to GitHub, which only exists for tasks in a GitHub project.
--
-- A separate table rather than columns on `tasks`: most installs have no
-- GitHub at all, and the polling loop writes here often enough that keeping it
-- off the row every other query reads is worth the join.
CREATE TABLE task_github (
    task_id       INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,

    -- The issue this task was started from, if it was.
    issue_number  INTEGER,

    -- The pull request opened for its branch, once there is one.
    pr_number     INTEGER,
    pr_url        TEXT,
    -- 'open', 'merged' or 'closed'. GitHub's own words, not an enum we invent.
    pr_state      TEXT,

    -- The commit the checks last reported on, and what they said.
    head_sha      TEXT,
    checks        TEXT NOT NULL DEFAULT 'none',

    -- When the daemon last heard from GitHub, so a stale chip can say so.
    polled_at     TEXT
) STRICT;

-- The polling loop asks for every task with an open pull request.
CREATE INDEX task_github_open ON task_github(pr_state) WHERE pr_state = 'open';
