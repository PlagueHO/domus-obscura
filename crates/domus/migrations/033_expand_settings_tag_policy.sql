PRAGMA defer_foreign_keys=ON;

CREATE TABLE settings_versions_expanded (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB)) <= 65536),
    created_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;

INSERT INTO settings_versions_expanded SELECT * FROM settings_versions;
DROP TABLE settings_versions;
ALTER TABLE settings_versions_expanded RENAME TO settings_versions;

CREATE TRIGGER immutable_settings_update BEFORE UPDATE ON settings_versions
BEGIN SELECT RAISE(ABORT, 'settings versions are immutable'); END;
CREATE TRIGGER immutable_settings_delete BEFORE DELETE ON settings_versions
BEGIN SELECT RAISE(ABORT, 'settings versions are retained'); END;

PRAGMA defer_foreign_keys=OFF;
