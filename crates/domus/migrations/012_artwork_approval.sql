CREATE TABLE artwork_candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    image_id TEXT NOT NULL CHECK(length(image_id) BETWEEN 1 AND 20),
    source TEXT NOT NULL CHECK(length(source) BETWEEN 1 AND 256),
    input_sha256 BLOB NOT NULL CHECK(length(input_sha256) = 32),
    background TEXT NOT NULL CHECK(length(background) BETWEEN 1 AND 32),
    profile INTEGER NOT NULL CHECK(profile = 1),
    jpeg BLOB NOT NULL CHECK(length(jpeg) BETWEEN 1 AND 8388608),
    sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
    width INTEGER NOT NULL CHECK(width BETWEEN 1 AND 1200),
    height INTEGER NOT NULL CHECK(height BETWEEN 1 AND 1200),
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    UNIQUE(job_id, plan_version, image_id, input_sha256, background),
    UNIQUE(id, job_id, plan_version),
    FOREIGN KEY(job_id, plan_version) REFERENCES job_plans(job_id, version)
) STRICT;

CREATE TRIGGER immutable_artwork_candidate BEFORE UPDATE ON artwork_candidates
BEGIN SELECT RAISE(ABORT, 'artwork candidates are immutable'); END;
CREATE TRIGGER retained_artwork_candidate BEFORE DELETE ON artwork_candidates
BEGIN SELECT RAISE(ABORT, 'artwork candidates are retained'); END;

CREATE TABLE artwork_approvals (
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    candidate_id INTEGER NOT NULL,
    approved_at INTEGER NOT NULL DEFAULT(unixepoch()),
    PRIMARY KEY(job_id, plan_version),
    FOREIGN KEY(candidate_id, job_id, plan_version)
        REFERENCES artwork_candidates(id, job_id, plan_version)
) STRICT;

CREATE TRIGGER immutable_artwork_approval BEFORE UPDATE ON artwork_approvals
BEGIN SELECT RAISE(ABORT, 'artwork approvals are immutable'); END;
CREATE TRIGGER retained_artwork_approval BEFORE DELETE ON artwork_approvals
BEGIN SELECT RAISE(ABORT, 'artwork approvals are retained'); END;
