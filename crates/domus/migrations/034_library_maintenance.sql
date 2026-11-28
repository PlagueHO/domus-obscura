CREATE TABLE library_scan_settings (
    version INTEGER PRIMARY KEY,
    interval_minutes INTEGER CHECK (interval_minutes BETWEEN 60 AND 10080),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
INSERT INTO library_scan_settings(version, interval_minutes) VALUES (1, NULL);
CREATE TRIGGER immutable_library_scan_settings BEFORE UPDATE ON library_scan_settings
BEGIN
    SELECT RAISE(ABORT, 'saved library scan policy is immutable');
END;
CREATE TRIGGER retained_library_scan_settings BEFORE DELETE ON library_scan_settings
BEGIN
    SELECT RAISE(ABORT, 'saved library scan policy is retained');
END;

CREATE TABLE library_scan_schedule (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    next_due INTEGER,
    last_request INTEGER REFERENCES worker_requests(id),
    last_error TEXT
);
INSERT INTO library_scan_schedule(id) VALUES (1);
