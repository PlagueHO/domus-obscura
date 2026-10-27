CREATE TABLE transfer_evidence (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transfer_id INTEGER NOT NULL REFERENCES transfer_files(id),
    claim_generation INTEGER NOT NULL CHECK(claim_generation > 0),
    session_generation INTEGER NOT NULL CHECK(session_generation >= 0),
    connection_id INTEGER NOT NULL CHECK(connection_id >= 0),
    kind TEXT NOT NULL CHECK(kind IN ('submitted', 'queued', 'offered', 'denied', 'interrupted', 'sealed')),
    value INTEGER NOT NULL CHECK(value BETWEEN 0 AND 4294967295),
    observed_at INTEGER NOT NULL DEFAULT(unixepoch()),
    UNIQUE(transfer_id, claim_generation, session_generation, connection_id, kind, value),
    CHECK((kind = 'sealed' AND session_generation = 0 AND connection_id = 0)
       OR (kind != 'sealed' AND session_generation > 0 AND connection_id > 0))
) STRICT;

CREATE INDEX transfer_evidence_page ON transfer_evidence(transfer_id, id);

CREATE TRIGGER immutable_transfer_evidence_update BEFORE UPDATE ON transfer_evidence
BEGIN SELECT RAISE(ABORT, 'transfer evidence is immutable'); END;
CREATE TRIGGER retained_transfer_evidence_delete BEFORE DELETE ON transfer_evidence
BEGIN SELECT RAISE(ABORT, 'transfer evidence is retained'); END;
