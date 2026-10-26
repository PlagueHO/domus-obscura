CREATE TABLE caa_cache (
    cache_key TEXT PRIMARY KEY CHECK(length(cache_key) BETWEEN 1 AND 512),
    body BLOB NOT NULL CHECK(length(body) BETWEEN 1 AND 8388608),
    sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
    sources TEXT NOT NULL CHECK(length(sources) BETWEEN 2 AND 24576),
    fetched_at INTEGER NOT NULL CHECK(fetched_at > 0),
    expires_at INTEGER NOT NULL CHECK(expires_at = fetched_at + 86400),
    accessed_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE artwork_caa_evidence (
    candidate_id INTEGER PRIMARY KEY REFERENCES artwork_candidates(id),
    release_id TEXT NOT NULL CHECK(length(release_id) = 36),
    image_id TEXT NOT NULL CHECK(length(image_id) BETWEEN 1 AND 20),
    listing BLOB NOT NULL CHECK(length(listing) BETWEEN 1 AND 2097152),
    listing_sha256 BLOB NOT NULL CHECK(length(listing_sha256) = 32),
    listing_sources TEXT NOT NULL CHECK(length(listing_sources) BETWEEN 2 AND 24576),
    listing_fetched_at INTEGER NOT NULL CHECK(listing_fetched_at > 0),
    image_sha256 BLOB NOT NULL CHECK(length(image_sha256) = 32),
    image_sources TEXT NOT NULL CHECK(length(image_sources) BETWEEN 2 AND 24576),
    image_fetched_at INTEGER NOT NULL CHECK(image_fetched_at > 0),
    permitted_use_acknowledged INTEGER NOT NULL CHECK(permitted_use_acknowledged = 1)
) STRICT;

CREATE TRIGGER immutable_caa_evidence BEFORE UPDATE ON artwork_caa_evidence
BEGIN SELECT RAISE(ABORT, 'CAA artwork evidence is immutable'); END;
CREATE TRIGGER retained_caa_evidence BEFORE DELETE ON artwork_caa_evidence
BEGIN SELECT RAISE(ABORT, 'CAA artwork evidence is retained'); END;
