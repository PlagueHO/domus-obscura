CREATE TABLE filesystem_roots (
    root TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('media','staging')),
    identity TEXT NOT NULL UNIQUE CHECK(length(identity) BETWEEN 1 AND 80),
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB)) BETWEEN 1 AND 1048576),
    bound_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;

CREATE TRIGGER immutable_filesystem_root BEFORE UPDATE ON filesystem_roots
BEGIN SELECT RAISE(ABORT, 'native root bindings are immutable'); END;
CREATE TRIGGER retained_filesystem_root BEFORE DELETE ON filesystem_roots
BEGIN SELECT RAISE(ABORT, 'native root bindings are retained'); END;
