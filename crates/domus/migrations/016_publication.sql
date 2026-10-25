CREATE TABLE publication_destinations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    run_id INTEGER NOT NULL REFERENCES normalization_runs(id),
    location_id INTEGER NOT NULL REFERENCES locations(id),
    root TEXT NOT NULL,
    root_identity TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('main','backup')),
    relative_folder TEXT NOT NULL CHECK(length(relative_folder) BETWEEN 1 AND 4096),
    folder_key TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL CHECK(length(manifest_sha256)=64),
    album_identity TEXT,
    verified_at INTEGER,
    last_error TEXT CHECK(last_error IS NULL OR length(CAST(last_error AS BLOB))<=2048),
    UNIQUE(job_id,plan_version,location_id),
    UNIQUE(location_id,folder_key),
    FOREIGN KEY(job_id,plan_version) REFERENCES job_plans(job_id,version),
    CHECK(verified_at IS NULL OR album_identity IS NOT NULL)
) STRICT;
CREATE TABLE publication_files (
    destination_id INTEGER NOT NULL REFERENCES publication_destinations(id),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 1000),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 256),
    bytes INTEGER NOT NULL CHECK(bytes>0),
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    verified_at INTEGER,
    PRIMARY KEY(destination_id,ordinal),
    UNIQUE(destination_id,name)
) STRICT;
CREATE TRIGGER immutable_publication_destination
BEFORE UPDATE OF id,job_id,plan_version,run_id,location_id,root,root_identity,kind,
 relative_folder,folder_key,manifest_sha256 ON publication_destinations
BEGIN SELECT RAISE(ABORT,'publication intent is immutable'); END;
CREATE TRIGGER monotonic_publication_destination
BEFORE UPDATE OF album_identity,verified_at ON publication_destinations
WHEN (OLD.album_identity IS NOT NULL AND NEW.album_identity IS NOT OLD.album_identity)
 OR (OLD.verified_at IS NOT NULL AND NEW.verified_at IS NOT OLD.verified_at)
BEGIN SELECT RAISE(ABORT,'publication receipts cannot change'); END;
CREATE TRIGGER retained_publication_destination BEFORE DELETE ON publication_destinations
BEGIN SELECT RAISE(ABORT,'publication intent is retained'); END;
CREATE TRIGGER immutable_publication_file
BEFORE UPDATE OF destination_id,ordinal,name,bytes,sha256 ON publication_files
BEGIN SELECT RAISE(ABORT,'publication file intent is immutable'); END;
CREATE TRIGGER monotonic_publication_file BEFORE UPDATE OF verified_at ON publication_files
WHEN OLD.verified_at IS NOT NULL AND NEW.verified_at IS NOT OLD.verified_at
BEGIN SELECT RAISE(ABORT,'publication file receipt cannot change'); END;
CREATE TRIGGER retained_publication_file BEFORE DELETE ON publication_files
BEGIN SELECT RAISE(ABORT,'publication files are retained'); END;
