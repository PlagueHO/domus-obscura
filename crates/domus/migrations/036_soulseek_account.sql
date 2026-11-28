CREATE TABLE soulseek_account_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    expected_revision TEXT NOT NULL CHECK(length(expected_revision)=64),
    file_identity TEXT,
    candidate_revision TEXT CHECK(length(candidate_revision)=64),
    state TEXT NOT NULL DEFAULT 'preparing'
        CHECK(state IN ('preparing','approved','activating','active','failed')),
    configuration_revision TEXT CHECK(length(configuration_revision)=64),
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    completed_at INTEGER,
    CHECK(state IN ('preparing','failed') OR file_identity IS NOT NULL),
    CHECK((state='active')=(configuration_revision IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX one_soulseek_account_change ON soulseek_account_changes((1))
    WHERE state IN ('preparing','approved','activating');
CREATE TRIGGER immutable_soulseek_account_intent
BEFORE UPDATE OF expected_revision,created_at ON soulseek_account_changes
BEGIN SELECT RAISE(ABORT,'Soulseek account intent is immutable'); END;
CREATE TRIGGER retained_soulseek_account_change BEFORE DELETE ON soulseek_account_changes
BEGIN SELECT RAISE(ABORT,'Soulseek account history is retained'); END;
CREATE TRIGGER valid_soulseek_account_change BEFORE UPDATE ON soulseek_account_changes
WHEN NOT (
    OLD.state=NEW.state
    OR (OLD.state='preparing' AND NEW.state IN ('approved','failed'))
    OR (OLD.state='approved' AND NEW.state IN ('activating','failed'))
    OR (OLD.state='activating' AND NEW.state IN ('active','failed'))
)
OR (OLD.file_identity IS NOT NULL AND NEW.file_identity IS NOT OLD.file_identity)
OR (OLD.candidate_revision IS NOT NULL AND NEW.candidate_revision IS NOT OLD.candidate_revision)
OR (OLD.state IN ('active','failed') AND (
    NEW.state IS NOT OLD.state OR NEW.configuration_revision IS NOT OLD.configuration_revision
    OR NEW.last_error IS NOT OLD.last_error OR NEW.completed_at IS NOT OLD.completed_at
))
BEGIN SELECT RAISE(ABORT,'invalid Soulseek account transition'); END;
