DROP TRIGGER immutable_private_root;
DROP TRIGGER retained_private_root;
ALTER TABLE private_roots RENAME TO private_roots_v20;
CREATE TABLE private_roots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    root TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK(kind IN ('staging','cache')),
    created_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;
INSERT INTO private_roots SELECT * FROM private_roots_v20;
DROP TABLE private_roots_v20;
CREATE TRIGGER immutable_private_root BEFORE UPDATE ON private_roots
BEGIN SELECT RAISE(ABORT, 'private root reservations are immutable'); END;
CREATE TRIGGER retained_private_root BEFORE DELETE ON private_roots
BEGIN SELECT RAISE(ABORT, 'private roots require explicit recovery before removal'); END;

DROP TRIGGER immutable_filesystem_root;
DROP TRIGGER retained_filesystem_root;
ALTER TABLE filesystem_roots RENAME TO filesystem_roots_v20;
CREATE TABLE filesystem_roots (
    root TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('media','staging','cache')),
    identity TEXT NOT NULL UNIQUE CHECK(length(identity) BETWEEN 1 AND 80),
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB)) BETWEEN 1 AND 1048576),
    bound_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;
INSERT INTO filesystem_roots SELECT * FROM filesystem_roots_v20;
DROP TABLE filesystem_roots_v20;
CREATE TRIGGER immutable_filesystem_root BEFORE UPDATE ON filesystem_roots
BEGIN SELECT RAISE(ABORT, 'native root bindings are immutable'); END;
CREATE TRIGGER retained_filesystem_root BEFORE DELETE ON filesystem_roots
BEGIN SELECT RAISE(ABORT, 'native root bindings are retained'); END;
