CREATE TABLE catalog_gate (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    generation INTEGER NOT NULL CHECK(generation >= 0),
    lease_until_ms INTEGER NOT NULL CHECK(lease_until_ms >= 0),
    next_request_ms INTEGER NOT NULL CHECK(next_request_ms >= 0),
    failures INTEGER NOT NULL CHECK(failures BETWEEN 0 AND 10)
) STRICT;
INSERT INTO catalog_gate VALUES (1, 0, 0, 0, 0);

CREATE TABLE catalog_cache (
    source TEXT PRIMARY KEY CHECK(length(source) BETWEEN 1 AND 4096),
    body BLOB NOT NULL CHECK(length(body) BETWEEN 1 AND 2097152),
    payload_sha256 BLOB NOT NULL CHECK(length(payload_sha256) = 32),
    fetched_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK(expires_at > fetched_at),
    accessed_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX catalog_cache_lru ON catalog_cache(accessed_at_ms, source);
CREATE INDEX catalog_cache_expiry ON catalog_cache(expires_at);
