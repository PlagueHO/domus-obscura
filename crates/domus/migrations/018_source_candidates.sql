CREATE TABLE source_candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256)=64),
    review_sha256 TEXT NOT NULL CHECK(length(review_sha256)=64),
    listing TEXT NOT NULL CHECK(length(CAST(listing AS BLOB)) BETWEEN 1 AND 1048576),
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    approved_at INTEGER,
    UNIQUE(job_id,plan_version,review_sha256),
    UNIQUE(id,job_id,plan_version),
    FOREIGN KEY(job_id,plan_version) REFERENCES job_plans(job_id,version)
) STRICT;
CREATE INDEX source_candidate_history ON source_candidates(job_id,plan_version,id);
CREATE TRIGGER immutable_source_candidate
BEFORE UPDATE OF id,job_id,plan_version,plan_sha256,review_sha256,listing,created_at ON source_candidates
BEGIN SELECT RAISE(ABORT,'source candidate is immutable'); END;
CREATE TRIGGER monotonic_source_candidate
BEFORE UPDATE OF approved_at ON source_candidates
WHEN OLD.approved_at IS NOT NULL AND NEW.approved_at IS NOT OLD.approved_at
BEGIN SELECT RAISE(ABORT,'source approval cannot change'); END;
CREATE TRIGGER retained_source_candidate BEFORE DELETE ON source_candidates
BEGIN SELECT RAISE(ABORT,'source candidate is retained'); END;

CREATE TABLE acquisition_source_reviews (
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    candidate_id INTEGER NOT NULL UNIQUE,
    PRIMARY KEY(job_id,plan_version),
    FOREIGN KEY(job_id,plan_version) REFERENCES acquisition_sources(job_id,plan_version),
    FOREIGN KEY(candidate_id,job_id,plan_version) REFERENCES source_candidates(id,job_id,plan_version)
) STRICT;
CREATE TRIGGER approved_acquisition_source_review BEFORE INSERT ON acquisition_source_reviews
WHEN NOT EXISTS(SELECT 1 FROM source_candidates c JOIN acquisition_sources s
    ON s.job_id=c.job_id AND s.plan_version=c.plan_version
    WHERE c.id=NEW.candidate_id AND c.job_id=NEW.job_id AND c.plan_version=NEW.plan_version
      AND c.approved_at IS NOT NULL AND s.peer=json_extract(c.listing,'$.peer'))
BEGIN SELECT RAISE(ABORT,'reviewed acquisition requires its approved candidate'); END;
CREATE TRIGGER immutable_acquisition_source_review BEFORE UPDATE ON acquisition_source_reviews
BEGIN SELECT RAISE(ABORT,'source review binding is immutable'); END;
CREATE TRIGGER retained_acquisition_source_review BEFORE DELETE ON acquisition_source_reviews
BEGIN SELECT RAISE(ABORT,'source review binding is retained'); END;
