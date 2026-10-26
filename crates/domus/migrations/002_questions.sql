CREATE TABLE questions (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES jobs(id),
    plan_version INTEGER NOT NULL CHECK(plan_version > 0),
    paused_state TEXT NOT NULL,
    prompt TEXT NOT NULL,
    choices TEXT NOT NULL CHECK(json_valid(choices)),
    answer TEXT,
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    answered_at INTEGER,
    CHECK((answer IS NULL) = (answered_at IS NULL))
) STRICT;
CREATE UNIQUE INDEX one_open_question ON questions(job_id) WHERE answer IS NULL;
CREATE INDEX job_questions ON questions(job_id, id);
