CREATE TABLE normalization_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    claim_generation INTEGER NOT NULL CHECK(claim_generation > 0),
    profile INTEGER NOT NULL CHECK(profile = 1),
    recipe_sha256 TEXT NOT NULL CHECK(length(recipe_sha256) = 64),
    cover_sha256 TEXT NOT NULL CHECK(length(cover_sha256) = 64),
    cover_bytes INTEGER NOT NULL CHECK(cover_bytes BETWEEN 1 AND 8388608),
    cover_width INTEGER NOT NULL CHECK(cover_width BETWEEN 1 AND 1200),
    cover_height INTEGER NOT NULL CHECK(cover_height BETWEEN 1 AND 1200),
    cover_verified_at INTEGER,
    completed_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    UNIQUE(job_id, plan_version, claim_generation),
    FOREIGN KEY(job_id, plan_version) REFERENCES job_plans(job_id, version),
    CHECK(completed_at IS NULL OR cover_verified_at IS NOT NULL)
) STRICT;

CREATE TRIGGER immutable_normalization_run
BEFORE UPDATE OF id, job_id, plan_version, claim_generation, profile, recipe_sha256,
    cover_sha256, cover_bytes, cover_width, cover_height, created_at ON normalization_runs
BEGIN SELECT RAISE(ABORT, 'normalization identities and recipes are immutable'); END;
CREATE TRIGGER monotonic_normalization_completion
BEFORE UPDATE OF cover_verified_at, completed_at ON normalization_runs
WHEN (OLD.cover_verified_at IS NOT NULL AND NEW.cover_verified_at IS NOT OLD.cover_verified_at)
    OR (OLD.completed_at IS NOT NULL AND NEW.completed_at IS NOT OLD.completed_at)
BEGIN SELECT RAISE(ABORT, 'normalization completion cannot be changed'); END;
CREATE TRIGGER retained_normalization_run BEFORE DELETE ON normalization_runs
BEGIN SELECT RAISE(ABORT, 'normalization attempts are retained'); END;

CREATE TABLE normalization_files (
    run_id INTEGER NOT NULL REFERENCES normalization_runs(id),
    track_number INTEGER NOT NULL CHECK(track_number BETWEEN 1 AND 1000),
    transfer_id INTEGER NOT NULL REFERENCES transfer_files(id),
    input_bytes INTEGER NOT NULL CHECK(input_bytes > 0),
    input_sha256 TEXT NOT NULL CHECK(length(input_sha256) = 64),
    tags_sha256 TEXT NOT NULL CHECK(length(tags_sha256) = 64),
    staging_name TEXT NOT NULL CHECK(length(staging_name) BETWEEN 1 AND 256),
    PRIMARY KEY(run_id, track_number),
    UNIQUE(run_id, transfer_id),
    UNIQUE(run_id, staging_name)
) STRICT;

CREATE TRIGGER immutable_normalization_file BEFORE UPDATE ON normalization_files
BEGIN SELECT RAISE(ABORT, 'normalization file bindings are immutable'); END;
CREATE TRIGGER retained_normalization_file BEFORE DELETE ON normalization_files
BEGIN SELECT RAISE(ABORT, 'normalization file bindings are retained'); END;

CREATE TABLE normalization_receipts (
    run_id INTEGER NOT NULL,
    track_number INTEGER NOT NULL,
    output_bytes INTEGER NOT NULL CHECK(output_bytes > 0),
    output_sha256 TEXT NOT NULL CHECK(length(output_sha256) = 64),
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB)) BETWEEN 1 AND 16384),
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    PRIMARY KEY(run_id, track_number),
    FOREIGN KEY(run_id, track_number) REFERENCES normalization_files(run_id, track_number)
) STRICT;

CREATE TRIGGER immutable_normalization_receipt BEFORE UPDATE ON normalization_receipts
BEGIN SELECT RAISE(ABORT, 'normalization receipts are immutable'); END;
CREATE TRIGGER retained_normalization_receipt BEFORE DELETE ON normalization_receipts
BEGIN SELECT RAISE(ABORT, 'normalization receipts are retained'); END;
