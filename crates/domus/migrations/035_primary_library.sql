ALTER TABLE locations ADD COLUMN retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1));
DROP INDEX one_main_location;
CREATE UNIQUE INDEX one_main_location ON locations(kind) WHERE kind='main' AND retired=0;

CREATE TABLE primary_library_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    body TEXT NOT NULL CHECK(json_valid(body) AND length(CAST(body AS BLOB))<=16384),
    expected_revision TEXT NOT NULL CHECK(length(expected_revision)=64),
    previous_location_id INTEGER REFERENCES locations(id),
    state TEXT NOT NULL DEFAULT 'approved'
        CHECK(state IN ('approved','activating','active','failed')),
    location_id INTEGER REFERENCES locations(id),
    configuration_revision TEXT CHECK(length(configuration_revision)=64),
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    completed_at INTEGER,
    CHECK((state='active')=(location_id IS NOT NULL AND configuration_revision IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX one_primary_library_change ON primary_library_changes((1))
    WHERE state IN ('approved','activating');
CREATE TRIGGER immutable_primary_library_change
BEFORE UPDATE OF body,expected_revision,previous_location_id,created_at ON primary_library_changes
BEGIN SELECT RAISE(ABORT,'primary library approval is immutable'); END;
CREATE TRIGGER retained_primary_library_change BEFORE DELETE ON primary_library_changes
BEGIN SELECT RAISE(ABORT,'primary library approval is retained'); END;
CREATE TRIGGER valid_primary_library_change BEFORE UPDATE ON primary_library_changes
WHEN NOT (
    OLD.state=NEW.state
    OR (OLD.state='approved' AND NEW.state IN ('activating','failed'))
    OR (OLD.state='activating' AND NEW.state IN ('active','failed'))
)
OR (OLD.state IN ('active','failed') AND (
    NEW.state IS NOT OLD.state OR NEW.location_id IS NOT OLD.location_id
    OR NEW.configuration_revision IS NOT OLD.configuration_revision
    OR NEW.completed_at IS NOT OLD.completed_at OR NEW.last_error IS NOT OLD.last_error
))
BEGIN SELECT RAISE(ABORT,'invalid primary library activation transition'); END;
