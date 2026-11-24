CREATE TABLE tag_reviews (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES jobs(id),
    plan_version INTEGER NOT NULL,
    question_id INTEGER NOT NULL UNIQUE REFERENCES questions(id),
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
    cover_sha256 TEXT NOT NULL CHECK(length(cover_sha256) = 64),
    approved_at INTEGER,
    resumed_at INTEGER,
    UNIQUE(job_id, plan_version),
    CHECK(resumed_at IS NULL OR approved_at IS NOT NULL)
) STRICT;
CREATE TABLE tag_review_tracks (
    review_id INTEGER NOT NULL REFERENCES tag_reviews(id),
    track_number INTEGER NOT NULL CHECK(track_number BETWEEN 1 AND 1000),
    transfer_id INTEGER NOT NULL REFERENCES transfer_files(id),
    input_bytes INTEGER NOT NULL CHECK(input_bytes > 0),
    input_sha256 TEXT NOT NULL CHECK(length(input_sha256) = 64),
    source_body TEXT NOT NULL CHECK(length(CAST(source_body AS BLOB)) <= 65536),
    output_body TEXT NOT NULL CHECK(length(CAST(output_body AS BLOB)) <= 65536),
    PRIMARY KEY(review_id, track_number)
) STRICT;
CREATE TRIGGER immutable_tag_review_tracks_update BEFORE UPDATE ON tag_review_tracks
BEGIN SELECT RAISE(ABORT, 'tag review tracks are immutable'); END;
CREATE TRIGGER immutable_tag_review_tracks_delete BEFORE DELETE ON tag_review_tracks
BEGIN SELECT RAISE(ABORT, 'tag review tracks are immutable'); END;
CREATE TRIGGER immutable_tag_review_binding
BEFORE UPDATE OF job_id, plan_version, question_id, sha256, cover_sha256 ON tag_reviews
BEGIN SELECT RAISE(ABORT, 'tag review binding is immutable'); END;
CREATE TRIGGER retained_tag_review BEFORE DELETE ON tag_reviews
BEGIN SELECT RAISE(ABORT, 'tag reviews are retained'); END;
CREATE TRIGGER immutable_tag_review_approval BEFORE UPDATE OF approved_at, resumed_at ON tag_reviews
WHEN (OLD.approved_at IS NOT NULL AND NEW.approved_at IS NOT OLD.approved_at)
  OR (OLD.resumed_at IS NOT NULL AND NEW.resumed_at IS NOT OLD.resumed_at)
BEGIN SELECT RAISE(ABORT, 'tag approval is immutable'); END;
