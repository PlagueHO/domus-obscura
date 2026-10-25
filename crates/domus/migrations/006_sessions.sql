CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY CHECK(length(token_hash) = 32),
    tenant_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;

CREATE TABLE session_events (
    id INTEGER PRIMARY KEY,
    event_type TEXT NOT NULL CHECK(event_type IN ('session_created', 'session_revoked')),
    tenant_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT(unixepoch())
) STRICT;
