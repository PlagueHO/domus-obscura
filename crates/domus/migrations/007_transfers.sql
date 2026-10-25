CREATE TABLE acquisition_sources (
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    peer TEXT NOT NULL CHECK(length(CAST(peer AS BLOB)) BETWEEN 1 AND 256),
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    PRIMARY KEY(job_id, plan_version),
    FOREIGN KEY(job_id, plan_version) REFERENCES job_plans(job_id, version)
) STRICT;

CREATE TRIGGER immutable_acquisition_source_update BEFORE UPDATE ON acquisition_sources
BEGIN SELECT RAISE(ABORT, 'acquisition sources are immutable'); END;
CREATE TRIGGER retained_acquisition_source_delete BEFORE DELETE ON acquisition_sources
BEGIN SELECT RAISE(ABORT, 'acquisition sources are retained'); END;

CREATE TABLE transfer_files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    track_number INTEGER NOT NULL CHECK(track_number BETWEEN 1 AND 1000),
    remote_path TEXT NOT NULL CHECK(length(CAST(remote_path AS BLOB)) BETWEEN 1 AND 4096),
    expected_bytes INTEGER NOT NULL CHECK(expected_bytes > 0),
    state TEXT NOT NULL DEFAULT 'planned'
        CHECK(state IN ('planned', 'submitting', 'active', 'received')),
    remote_token INTEGER CHECK(remote_token BETWEEN 0 AND 4294967295),
    received_bytes INTEGER NOT NULL DEFAULT 0 CHECK(received_bytes BETWEEN 0 AND expected_bytes),
    payload_sha256 BLOB CHECK(payload_sha256 IS NULL OR length(payload_sha256) = 32),
    updated_at INTEGER NOT NULL DEFAULT(unixepoch()),
    UNIQUE(job_id, plan_version, track_number),
    UNIQUE(job_id, plan_version, remote_path),
    FOREIGN KEY(job_id, plan_version) REFERENCES acquisition_sources(job_id, plan_version),
    CHECK((state IN ('planned', 'submitting') AND remote_token IS NULL
               AND received_bytes = 0 AND payload_sha256 IS NULL)
        OR (state = 'active' AND remote_token IS NOT NULL AND payload_sha256 IS NULL)
        OR (state = 'received' AND remote_token IS NOT NULL
               AND received_bytes = expected_bytes AND payload_sha256 IS NOT NULL))
) STRICT;

CREATE TRIGGER immutable_transfer_identity
BEFORE UPDATE OF job_id, plan_version, track_number, remote_path, expected_bytes ON transfer_files
BEGIN SELECT RAISE(ABORT, 'transfer identities are immutable'); END;
CREATE TRIGGER retained_transfer_delete BEFORE DELETE ON transfer_files
BEGIN SELECT RAISE(ABORT, 'transfer records are retained'); END;
