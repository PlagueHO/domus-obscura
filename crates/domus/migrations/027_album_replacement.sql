CREATE TABLE album_replacement_reviews (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    location_id INTEGER NOT NULL REFERENCES locations(id),
    root TEXT NOT NULL,
    root_identity TEXT NOT NULL,
    relative_folder TEXT NOT NULL CHECK(length(relative_folder) BETWEEN 1 AND 4096),
    album_identity TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL CHECK(length(manifest_sha256)=64),
    action TEXT NOT NULL CHECK(action IN ('skip','copy','replace')),
    output_folder TEXT,
    approved_at INTEGER NOT NULL DEFAULT(unixepoch()),
    FOREIGN KEY(job_id,plan_version) REFERENCES job_plans(job_id,version),
    CHECK((action='copy' AND output_folder IS NOT NULL)
       OR (action IN ('skip','replace') AND output_folder IS NULL))
) STRICT;
CREATE TABLE album_replacement_review_files (
    review_id INTEGER NOT NULL REFERENCES album_replacement_reviews(id),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 1000),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 256),
    bytes INTEGER NOT NULL CHECK(bytes>=0),
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    PRIMARY KEY(review_id,ordinal),
    UNIQUE(review_id,name)
) STRICT;
CREATE TABLE publication_replacement_journal (
    destination_id INTEGER PRIMARY KEY REFERENCES publication_destinations(id),
    review_id INTEGER NOT NULL REFERENCES album_replacement_reviews(id),
    retained_folder TEXT NOT NULL CHECK(length(retained_folder) BETWEEN 1 AND 4096),
    restore_folder TEXT NOT NULL CHECK(length(restore_folder) BETWEEN 1 AND 4096),
    state TEXT NOT NULL CHECK(state IN (
        'planned','retained','published','restore_prepared',
        'restore_current_moved','restore_original_visible','restored'
    )),
    retained_at INTEGER,
    published_at INTEGER,
    restore_started_at INTEGER,
    restored_at INTEGER
) STRICT;
CREATE TRIGGER immutable_album_replacement_review
BEFORE UPDATE ON album_replacement_reviews
BEGIN SELECT RAISE(ABORT,'album replacement review is immutable'); END;
CREATE TRIGGER retained_album_replacement_review
BEFORE DELETE ON album_replacement_reviews
BEGIN SELECT RAISE(ABORT,'album replacement review is retained'); END;
CREATE TRIGGER immutable_album_replacement_review_file
BEFORE UPDATE ON album_replacement_review_files
BEGIN SELECT RAISE(ABORT,'album replacement review files are immutable'); END;
CREATE TRIGGER retained_album_replacement_review_file
BEFORE DELETE ON album_replacement_review_files
BEGIN SELECT RAISE(ABORT,'album replacement review files are retained'); END;
CREATE TRIGGER valid_publication_replacement_state
BEFORE UPDATE OF state ON publication_replacement_journal
WHEN NOT (
    (OLD.state='planned' AND NEW.state='retained')
 OR (OLD.state='retained' AND NEW.state='published')
 OR (OLD.state='published' AND NEW.state='restore_prepared')
 OR (OLD.state='restore_prepared' AND NEW.state='restore_current_moved')
 OR (OLD.state='restore_current_moved' AND NEW.state='restore_original_visible')
 OR (OLD.state='restore_original_visible' AND NEW.state='restored')
 OR OLD.state=NEW.state
)
BEGIN SELECT RAISE(ABORT,'invalid replacement recovery transition'); END;
CREATE TRIGGER immutable_publication_replacement_intent
BEFORE UPDATE OF destination_id,review_id,retained_folder,restore_folder
ON publication_replacement_journal
WHEN NEW.destination_id!=OLD.destination_id
  OR NEW.review_id!=OLD.review_id
  OR NEW.retained_folder!=OLD.retained_folder
  OR NEW.restore_folder!=OLD.restore_folder
BEGIN SELECT RAISE(ABORT,'replacement recovery intent is immutable'); END;
CREATE TRIGGER retained_publication_replacement_journal
BEFORE DELETE ON publication_replacement_journal
BEGIN SELECT RAISE(ABORT,'replacement recovery journal is retained'); END;
