CREATE TABLE queue_presentations (
    job_id INTEGER NOT NULL REFERENCES jobs(id),
    plan_version INTEGER NOT NULL CHECK(plan_version > 0),
    completed_at INTEGER NOT NULL,
    dismissed_at INTEGER,
    PRIMARY KEY(job_id, plan_version)
) STRICT;

INSERT INTO queue_presentations(job_id, plan_version, completed_at)
SELECT j.id, j.plan_version,
    COALESCE((SELECT max(e.created_at) FROM events e
              WHERE e.job_id = j.id AND e.plan_version = j.plan_version
                AND e.event_type = 'complete'), unixepoch())
FROM jobs j WHERE j.state = 'complete';

CREATE TRIGGER queue_completed_update AFTER UPDATE OF state, plan_version ON jobs
WHEN NEW.state = 'complete'
 AND (OLD.state != 'complete' OR OLD.plan_version != NEW.plan_version)
BEGIN
    INSERT INTO queue_presentations(job_id, plan_version, completed_at)
    VALUES (NEW.id, NEW.plan_version, unixepoch());
END;

CREATE TRIGGER queue_completed_insert AFTER INSERT ON jobs
WHEN NEW.state = 'complete'
BEGIN
    INSERT INTO queue_presentations(job_id, plan_version, completed_at)
    VALUES (NEW.id, NEW.plan_version, unixepoch());
END;
