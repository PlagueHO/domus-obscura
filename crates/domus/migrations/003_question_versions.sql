DROP INDEX one_open_question;
CREATE UNIQUE INDEX one_open_question ON questions(job_id, plan_version) WHERE answer IS NULL;
