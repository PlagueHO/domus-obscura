ALTER TABLE scans ADD COLUMN folder_scope TEXT;
CREATE TABLE library_artists (
    id INTEGER PRIMARY KEY,
    location_id INTEGER NOT NULL REFERENCES locations(id),
    name_key TEXT NOT NULL,
    name TEXT NOT NULL,
    file_count INTEGER NOT NULL DEFAULT 0 CHECK(file_count >= 0),
    UNIQUE(location_id, name_key)
) STRICT;
CREATE INDEX library_artist_page ON library_artists(location_id, id) WHERE file_count > 0;
CREATE TABLE library_albums (
    id INTEGER PRIMARY KEY,
    location_id INTEGER NOT NULL REFERENCES locations(id),
    artist_id INTEGER NOT NULL REFERENCES library_artists(id),
    title_key TEXT NOT NULL,
    title TEXT NOT NULL,
    folder TEXT NOT NULL,
    file_count INTEGER NOT NULL DEFAULT 0 CHECK(file_count >= 0),
    UNIQUE(location_id, artist_id, title_key, folder)
) STRICT;
CREATE INDEX library_album_page ON library_albums(location_id, id) WHERE file_count > 0;
CREATE INDEX library_artist_albums ON library_albums(location_id, artist_id, id) WHERE file_count > 0;
CREATE INDEX library_album_match ON library_albums(location_id, artist_id, title_key, id) WHERE file_count > 0;
CREATE TABLE library_metadata (
    entry_id INTEGER PRIMARY KEY REFERENCES entries(id),
    location_id INTEGER NOT NULL REFERENCES locations(id),
    scan_id INTEGER NOT NULL REFERENCES scans(id),
    observed_at INTEGER NOT NULL DEFAULT(unixepoch()),
    profile INTEGER NOT NULL DEFAULT 1,
    body TEXT NOT NULL CHECK(json_valid(body)),
    status TEXT NOT NULL,
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    artist_id INTEGER REFERENCES library_artists(id),
    album_id INTEGER REFERENCES library_albums(id)
) STRICT;
CREATE INDEX library_album_files ON library_metadata(location_id, album_id, entry_id) WHERE active=1;
CREATE INDEX library_metadata_status ON library_metadata(location_id, status, entry_id) WHERE active=1;
CREATE TABLE library_metadata_counts (
    location_id INTEGER PRIMARY KEY REFERENCES locations(id),
    total_files INTEGER NOT NULL DEFAULT 0,
    files INTEGER NOT NULL DEFAULT 0,
    issues INTEGER NOT NULL DEFAULT 0
) STRICT;
INSERT INTO library_metadata_counts(location_id,total_files)
SELECT l.id,(SELECT count(*) FROM entries e WHERE e.location_id=l.id AND e.kind='file' AND e.present=1)
FROM locations l;
CREATE TRIGGER library_location_insert AFTER INSERT ON locations BEGIN
    INSERT INTO library_metadata_counts(location_id) VALUES (NEW.id);
END;
CREATE TRIGGER library_entry_insert AFTER INSERT ON entries WHEN NEW.kind='file' AND NEW.present=1 BEGIN
    UPDATE library_metadata_counts SET total_files=total_files+1 WHERE location_id=NEW.location_id;
END;
CREATE TRIGGER library_entry_count_update AFTER UPDATE OF kind,present ON entries BEGIN
    UPDATE library_metadata_counts SET total_files=total_files
      +CASE WHEN NEW.kind='file' AND NEW.present=1 THEN 1 ELSE 0 END
      -CASE WHEN OLD.kind='file' AND OLD.present=1 THEN 1 ELSE 0 END
      WHERE location_id=NEW.location_id;
END;
CREATE TRIGGER library_metadata_insert AFTER INSERT ON library_metadata WHEN NEW.active=1 BEGIN
    UPDATE library_artists SET file_count=file_count+1 WHERE id=NEW.artist_id;
    UPDATE library_albums SET file_count=file_count+1 WHERE id=NEW.album_id;
    UPDATE library_metadata_counts SET files=files+1,
      issues=issues+CASE WHEN NEW.status IN ('tagged','asset') THEN 0 ELSE 1 END
      WHERE location_id=NEW.location_id;
END;
CREATE TRIGGER library_metadata_delete AFTER DELETE ON library_metadata WHEN OLD.active=1 BEGIN
    UPDATE library_artists SET file_count=file_count-1 WHERE id=OLD.artist_id;
    UPDATE library_albums SET file_count=file_count-1 WHERE id=OLD.album_id;
    UPDATE library_metadata_counts SET files=files-1,
      issues=issues-CASE WHEN OLD.status IN ('tagged','asset') THEN 0 ELSE 1 END
      WHERE location_id=OLD.location_id;
END;
CREATE TRIGGER library_metadata_update_old AFTER UPDATE ON library_metadata WHEN OLD.active=1 BEGIN
    UPDATE library_artists SET file_count=file_count-1 WHERE id=OLD.artist_id;
    UPDATE library_albums SET file_count=file_count-1 WHERE id=OLD.album_id;
    UPDATE library_metadata_counts SET files=files-1,
      issues=issues-CASE WHEN OLD.status IN ('tagged','asset') THEN 0 ELSE 1 END
      WHERE location_id=OLD.location_id;
END;
CREATE TRIGGER library_metadata_update_new AFTER UPDATE ON library_metadata WHEN NEW.active=1 BEGIN
    UPDATE library_artists SET file_count=file_count+1 WHERE id=NEW.artist_id;
    UPDATE library_albums SET file_count=file_count+1 WHERE id=NEW.album_id;
    UPDATE library_metadata_counts SET files=files+1,
      issues=issues+CASE WHEN NEW.status IN ('tagged','asset') THEN 0 ELSE 1 END
      WHERE location_id=NEW.location_id;
END;
CREATE TRIGGER library_entry_changed AFTER UPDATE OF scan_id,present,kind ON entries BEGIN
    UPDATE library_metadata SET active=0 WHERE entry_id=NEW.id;
END;
CREATE TABLE library_folder_observations (
    location_id INTEGER NOT NULL REFERENCES locations(id),
    folder TEXT NOT NULL,
    state TEXT NOT NULL,
    observed_at INTEGER,
    last_error TEXT,
    PRIMARY KEY(location_id, folder)
) STRICT;
