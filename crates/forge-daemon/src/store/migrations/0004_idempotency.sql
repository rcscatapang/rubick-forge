-- A key that makes creating a task safe to retry.
--
-- Any client that might send `POST /tasks` twice — a hub retrying after a
-- crash, a phone on a bad connection — can name its attempt. A second create
-- with the same key returns the first task rather than making another.
--
-- Deliberately generic: a worker honouring this does not learn that hubs or
-- queues exist.
ALTER TABLE tasks ADD COLUMN idempotency_key TEXT;

CREATE UNIQUE INDEX tasks_idempotency_key
    ON tasks(idempotency_key) WHERE idempotency_key IS NOT NULL;
