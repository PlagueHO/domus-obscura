CREATE TABLE locations (
    id INTEGER PRIMARY KEY,
    root TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK(kind IN ('main', 'backup')),
    observed_at INTEGER,
    coverage TEXT NOT NULL DEFAULT 'unknown'
        CHECK(coverage IN ('unknown', 'scanning', 'complete', 'incomplete')),
    last_error TEXT
) STRICT;
CREATE UNIQUE INDEX one_main_location ON locations(kind) WHERE kind = 'main';

CREATE TABLE scans (
    id INTEGER PRIMARY KEY,
    location_id INTEGER NOT NULL REFERENCES locations(id),
    started_at INTEGER NOT NULL DEFAULT(unixepoch()),
    completed_at INTEGER,
    status TEXT NOT NULL CHECK(status IN ('running', 'complete', 'failed')),
    owner TEXT NOT NULL,
    lease_until INTEGER NOT NULL,
    observed_count INTEGER NOT NULL DEFAULT 0,
    error_count INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE UNIQUE INDEX one_location_scan ON scans(location_id) WHERE status = 'running';

CREATE TABLE scan_directories (
    scan_id INTEGER NOT NULL REFERENCES scans(id),
    relative_path TEXT NOT NULL,
    complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0, 1)),
    PRIMARY KEY(scan_id, relative_path)
) STRICT;

CREATE TABLE entries (
    id INTEGER PRIMARY KEY,
    location_id INTEGER NOT NULL REFERENCES locations(id),
    relative_path TEXT NOT NULL,
    parent_path TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('directory', 'file', 'unsafe_link', 'other')),
    size INTEGER,
    modified_at INTEGER,
    scan_id INTEGER NOT NULL REFERENCES scans(id),
    present INTEGER NOT NULL DEFAULT 1 CHECK(present IN (0, 1)),
    UNIQUE(location_id, relative_path)
) STRICT;
CREATE INDEX folder_entries ON entries(location_id, parent_path, id);
CREATE INDEX scan_entries ON entries(location_id, scan_id);

CREATE TABLE scan_errors (
    id INTEGER PRIMARY KEY,
    scan_id INTEGER NOT NULL REFERENCES scans(id),
    detail TEXT NOT NULL
) STRICT;

CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    plan_version INTEGER NOT NULL CHECK(plan_version > 0),
    state TEXT NOT NULL CHECK(state IN (
        'queued', 'searching', 'downloading', 'validating', 'normalizing',
        'publishing', 'awaiting_input', 'backup_pending', 'failed', 'cancelled', 'complete'
    )),
    owner TEXT,
    lease_until INTEGER,
    created_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;

CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES jobs(id),
    event_type TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;
