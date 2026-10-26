CREATE TABLE job_controls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL REFERENCES jobs(id),
    plan_version INTEGER NOT NULL CHECK(plan_version>0),
    action TEXT NOT NULL CHECK(action IN ('pause','cancel')),
    requested_at INTEGER NOT NULL DEFAULT(unixepoch()),
    applied_at INTEGER,
    question_id INTEGER UNIQUE REFERENCES questions(id),
    paused_state TEXT,
    continued_at INTEGER,
    CHECK((applied_at IS NULL AND paused_state IS NULL AND question_id IS NULL)
       OR (applied_at IS NOT NULL AND paused_state IS NOT NULL
           AND ((action='pause' AND question_id IS NOT NULL) OR (action='cancel' AND question_id IS NULL)))),
    CHECK(continued_at IS NULL OR (action='pause' AND applied_at IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX one_pending_job_control ON job_controls(job_id,plan_version)
WHERE applied_at IS NULL;
CREATE INDEX job_control_history ON job_controls(job_id,plan_version,id);
CREATE TRIGGER immutable_job_control
BEFORE UPDATE OF id,job_id,plan_version,action,requested_at ON job_controls
BEGIN SELECT RAISE(ABORT,'job control intent is immutable'); END;
CREATE TRIGGER monotonic_job_control
BEFORE UPDATE OF applied_at,question_id,paused_state,continued_at ON job_controls
WHEN (OLD.applied_at IS NOT NULL AND
      (NEW.applied_at IS NOT OLD.applied_at OR NEW.question_id IS NOT OLD.question_id
       OR NEW.paused_state IS NOT OLD.paused_state))
  OR (OLD.continued_at IS NOT NULL AND NEW.continued_at IS NOT OLD.continued_at)
BEGIN SELECT RAISE(ABORT,'job control receipt cannot change'); END;
CREATE TRIGGER retained_job_control BEFORE DELETE ON job_controls
BEGIN SELECT RAISE(ABORT,'job control is retained'); END;
