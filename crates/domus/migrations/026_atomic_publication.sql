ALTER TABLE publication_destinations
    ADD COLUMN prepared_folder TEXT
    CHECK(prepared_folder IS NULL OR length(prepared_folder) BETWEEN 1 AND 4096);
ALTER TABLE publication_destinations
    ADD COLUMN prepare_attempt INTEGER NOT NULL DEFAULT 0 CHECK(prepare_attempt >= 0);
ALTER TABLE publication_destinations
    ADD COLUMN published_at INTEGER;
ALTER TABLE publication_destinations
    ADD COLUMN atomic INTEGER NOT NULL DEFAULT 0 CHECK(atomic IN (0,1));

UPDATE publication_destinations
SET published_at=verified_at
WHERE verified_at IS NOT NULL;

CREATE TRIGGER immutable_publication_prepared_folder
BEFORE UPDATE OF prepared_folder,prepare_attempt ON publication_destinations
WHEN OLD.album_identity IS NOT NULL AND OLD.prepared_folder IS NOT NULL
 AND (NEW.prepared_folder IS NOT OLD.prepared_folder
      OR NEW.prepare_attempt IS NOT OLD.prepare_attempt)
BEGIN SELECT RAISE(ABORT,'bound publication preparation is immutable'); END;
CREATE TRIGGER monotonic_publication_published
BEFORE UPDATE OF published_at ON publication_destinations
WHEN OLD.published_at IS NOT NULL AND NEW.published_at IS NOT OLD.published_at
BEGIN SELECT RAISE(ABORT,'publication visibility receipts cannot change'); END;
CREATE TRIGGER immutable_publication_mode
BEFORE UPDATE OF atomic ON publication_destinations
WHEN OLD.atomic IS NOT NEW.atomic
BEGIN SELECT RAISE(ABORT,'publication mode is immutable'); END;
