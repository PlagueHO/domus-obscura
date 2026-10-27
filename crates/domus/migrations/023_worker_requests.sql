CREATE TABLE worker_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    lane TEXT NOT NULL CHECK(lane IN ('scan','acquisition')),
    target TEXT NOT NULL CHECK(length(target) BETWEEN 1 AND 100),
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB)) BETWEEN 1 AND 4096),
    configuration_binding TEXT NOT NULL CHECK(length(configuration_binding)=64),
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK(state IN ('queued','running','interrupted','waiting','failed','complete')),
    generation INTEGER NOT NULL DEFAULT 0,
    owner TEXT,
    lease_until INTEGER,
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT(unixepoch()),
    last_error TEXT CHECK(length(CAST(last_error AS BLOB)) <= 2048),
    CHECK((owner IS NULL) = (lease_until IS NULL))
) STRICT;
CREATE UNIQUE INDEX worker_request_active_target ON worker_requests(target)
    WHERE state != 'complete';
CREATE INDEX worker_request_unfinished_history ON worker_requests(id)
    WHERE state != 'complete';
CREATE INDEX worker_request_eligible ON worker_requests(lane,id)
    WHERE state IN ('queued','interrupted','running');
CREATE INDEX worker_request_job_history ON worker_requests(target,id);
CREATE TABLE worker_host (
    id INTEGER PRIMARY KEY CHECK(id=1),
    owner TEXT NOT NULL,
    generation INTEGER NOT NULL,
    revision TEXT NOT NULL CHECK(length(revision)=64),
    heartbeat INTEGER NOT NULL,
    stopping INTEGER NOT NULL CHECK(stopping IN (0,1))
) STRICT;
CREATE TRIGGER immutable_worker_request BEFORE UPDATE OF lane,target,body,configuration_binding,created_at ON worker_requests
BEGIN SELECT RAISE(ABORT,'worker request is immutable'); END;
