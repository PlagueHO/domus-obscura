CREATE TABLE application_identity (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    identity BLOB NOT NULL CHECK(length(identity) = 32)
) STRICT;
INSERT INTO application_identity VALUES (1, randomblob(32));
CREATE TRIGGER immutable_application_identity BEFORE UPDATE ON application_identity
BEGIN SELECT RAISE(ABORT, 'application identity is immutable'); END;
CREATE TRIGGER retained_application_identity BEFORE DELETE ON application_identity
BEGIN SELECT RAISE(ABORT, 'application identity is retained'); END;

CREATE TABLE private_roots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    root TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK(kind = 'staging'),
    created_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;
CREATE TRIGGER immutable_private_root BEFORE UPDATE ON private_roots
BEGIN SELECT RAISE(ABORT, 'private root reservations are immutable'); END;
CREATE TRIGGER retained_private_root BEFORE DELETE ON private_roots
BEGIN SELECT RAISE(ABORT, 'private roots require explicit recovery before removal'); END;
