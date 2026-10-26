CREATE TABLE settings_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB)) <= 16384),
    created_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;

CREATE TRIGGER immutable_settings_update BEFORE UPDATE ON settings_versions
BEGIN SELECT RAISE(ABORT, 'settings versions are immutable'); END;
CREATE TRIGGER immutable_settings_delete BEFORE DELETE ON settings_versions
BEGIN SELECT RAISE(ABORT, 'settings versions are retained'); END;

CREATE TABLE job_plans (
    job_id INTEGER NOT NULL REFERENCES jobs(id),
    version INTEGER NOT NULL CHECK(version > 0),
    settings_version INTEGER NOT NULL REFERENCES settings_versions(id),
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB)) <= 2097152),
    approved_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    PRIMARY KEY(job_id, version)
) STRICT;

CREATE TRIGGER immutable_plan_body
BEFORE UPDATE OF job_id, version, settings_version, body ON job_plans
BEGIN SELECT RAISE(ABORT, 'plan bodies are immutable'); END;
ALTER TABLE events ADD COLUMN plan_version INTEGER CHECK(plan_version > 0);
