CREATE TABLE album_restore_approvals (
    destination_id INTEGER PRIMARY KEY REFERENCES publication_destinations(id),
    preview_sha256 TEXT NOT NULL CHECK(length(preview_sha256)=64),
    album_identity TEXT NOT NULL,
    current_files TEXT NOT NULL CHECK(json_valid(current_files)),
    approved_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;
CREATE TABLE album_restore_status (
    destination_id INTEGER PRIMARY KEY REFERENCES publication_destinations(id),
    last_error TEXT,
    question_id INTEGER REFERENCES questions(id)
) STRICT;
CREATE TABLE album_restore_questions (
    question_id INTEGER PRIMARY KEY REFERENCES questions(id),
    destination_id INTEGER NOT NULL REFERENCES publication_destinations(id),
    resolved_at INTEGER
) STRICT;
CREATE UNIQUE INDEX one_active_album_restore_question
ON album_restore_questions(destination_id) WHERE resolved_at IS NULL;
CREATE TRIGGER immutable_album_restore_approval
BEFORE UPDATE ON album_restore_approvals
BEGIN SELECT RAISE(ABORT,'restore approval is immutable'); END;
CREATE TRIGGER retained_album_restore_approval
BEFORE DELETE ON album_restore_approvals
BEGIN SELECT RAISE(ABORT,'restore approval is retained'); END;
CREATE TRIGGER immutable_album_restore_question_binding
BEFORE UPDATE OF question_id,destination_id ON album_restore_questions
BEGIN SELECT RAISE(ABORT,'restore question binding is immutable'); END;
