CREATE TABLE recovery_operations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entry_id INTEGER NOT NULL UNIQUE,
    job_id INTEGER NOT NULL,
    plan_version INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('delete','cleanup')),
    version TEXT NOT NULL,
    preview_sha256 TEXT NOT NULL CHECK(length(preview_sha256)=64),
    binding TEXT NOT NULL CHECK(json_valid(binding)),
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','complete')),
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT(unixepoch()),
    completed_at INTEGER,
    FOREIGN KEY(job_id,plan_version) REFERENCES job_plans(job_id,version)
) STRICT;
CREATE INDEX recovery_operations_job ON recovery_operations(job_id,id);
CREATE TABLE recovery_items (
    operation_id INTEGER NOT NULL REFERENCES recovery_operations(id),
    ordinal INTEGER NOT NULL,
    artifact_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    identity TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes>=0),
    sha256 TEXT NOT NULL,
    directory INTEGER NOT NULL CHECK(directory IN (0,1)),
    state TEXT NOT NULL DEFAULT 'planned' CHECK(state IN ('planned','removing','removed')),
    PRIMARY KEY(operation_id,ordinal),
    UNIQUE(operation_id,path)
) STRICT;
CREATE INDEX recovery_items_artifact ON recovery_items(artifact_id,operation_id);
CREATE TABLE scratch_cleanup_admissions (
    run_id INTEGER PRIMARY KEY REFERENCES normalization_runs(id),
    root TEXT NOT NULL,
    root_identity TEXT NOT NULL,
    last_error TEXT
) STRICT;
CREATE TRIGGER immutable_scratch_cleanup_admission
BEFORE UPDATE OF run_id,root,root_identity ON scratch_cleanup_admissions
BEGIN SELECT RAISE(ABORT,'scratch cleanup admission is immutable'); END;
CREATE TRIGGER retained_scratch_cleanup_admission BEFORE DELETE ON scratch_cleanup_admissions
BEGIN SELECT RAISE(ABORT,'scratch cleanup admission is retained'); END;
CREATE TRIGGER immutable_recovery_approval
BEFORE UPDATE OF entry_id,job_id,plan_version,kind,version,preview_sha256,binding,created_at
ON recovery_operations
BEGIN SELECT RAISE(ABORT,'recovery approval is immutable'); END;
CREATE TRIGGER retained_recovery_operation BEFORE DELETE ON recovery_operations
BEGIN SELECT RAISE(ABORT,'recovery operation is retained'); END;
CREATE TRIGGER monotonic_recovery_operation BEFORE UPDATE ON recovery_operations
WHEN OLD.state='complete' AND (NEW.state!='complete' OR NEW.completed_at IS NOT OLD.completed_at)
BEGIN SELECT RAISE(ABORT,'recovery completion is immutable'); END;
CREATE TRIGGER immutable_recovery_item
BEFORE UPDATE OF operation_id,ordinal,artifact_id,path,identity,bytes,sha256,directory
ON recovery_items
BEGIN SELECT RAISE(ABORT,'recovery item is immutable'); END;
CREATE TRIGGER monotonic_recovery_item BEFORE UPDATE OF state ON recovery_items
WHEN NOT (OLD.state=NEW.state OR (OLD.state='planned' AND NEW.state='removing')
 OR (OLD.state='removing' AND NEW.state='removed'))
BEGIN SELECT RAISE(ABORT,'invalid recovery item transition'); END;
CREATE TRIGGER retained_recovery_item BEFORE DELETE ON recovery_items
BEGIN SELECT RAISE(ABORT,'recovery item is retained'); END;

CREATE VIEW recovery_inventory AS
WITH artifacts AS (
 SELECT t.id*8 AS id,t.job_id,t.plan_version,'input' AS category,
        NULL AS location_id,'transfer-'||t.id||'.input' AS folder,
        CASE WHEN t.state='received' THEN t.expected_bytes ELSE NULL END AS recorded_bytes,'transfer:'||t.id AS version,
        t.state AS saved_state
 FROM transfer_files t
 UNION ALL
 SELECT n.id*8+1,n.job_id,n.plan_version,'scratch',NULL,'normalization-'||n.id,
        CASE WHEN n.completed_at IS NOT NULL THEN n.cover_bytes+(SELECT COALESCE(sum(r.output_bytes),0)
          FROM normalization_receipts r WHERE r.run_id=n.id) ELSE NULL END,
        'normalization:'||n.id,CASE WHEN n.completed_at IS NOT NULL THEN 'retained' ELSE 'incomplete' END
 FROM normalization_runs n
 UNION ALL
 SELECT d.id*8+2,d.job_id,d.plan_version,'pending',d.location_id,
        COALESCE(d.prepared_folder,d.relative_folder),NULL,
        'publication:'||d.id,'pending'
 FROM publication_destinations d WHERE d.published_at IS NULL
 UNION ALL
 SELECT r.destination_id*8+CASE WHEN r.state='restored' THEN 4 ELSE 3 END,
        d.job_id,d.plan_version,'retained',d.location_id,r.retained_folder,
        CASE WHEN r.state='restored'
          THEN CASE WHEN a.destination_id IS NOT NULL
            THEN (SELECT sum(json_extract(value,'$.bytes')) FROM json_each(a.current_files))
            ELSE (SELECT sum(bytes) FROM publication_files WHERE destination_id=d.id) END
          ELSE (SELECT sum(bytes) FROM album_replacement_review_files WHERE review_id=r.review_id) END,
        'replacement:'||r.review_id||CASE WHEN r.state='restored' THEN ':displaced' ELSE ':original' END,
        r.state
 FROM publication_replacement_journal r
 JOIN publication_destinations d ON d.id=r.destination_id
 LEFT JOIN album_restore_approvals a ON a.destination_id=d.id
 WHERE r.state!='planned'
)
SELECT a.*,COALESCE(o.state,CASE WHEN s.run_id IS NOT NULL THEN 'pending' ELSE a.saved_state END) AS state,
       o.id AS operation_id,COALESCE(o.last_error,s.last_error) AS last_error,
       (a.category='retained' AND a.saved_state IN ('published','restored')
        AND j.state='complete' AND j.plan_version=a.plan_version
        AND o.id IS NULL
        AND NOT EXISTS(SELECT 1 FROM publication_destinations d
          WHERE d.job_id=a.job_id AND d.plan_version=a.plan_version
          AND (d.verified_at IS NULL OR d.published_at IS NULL OR d.last_error IS NOT NULL))) AS eligible
FROM artifacts a JOIN jobs j ON j.id=a.job_id
LEFT JOIN scratch_cleanup_admissions s ON a.category='scratch' AND s.run_id=(a.id-1)/8
LEFT JOIN recovery_operations o ON o.id=(
 SELECT operation_id FROM recovery_items WHERE artifact_id=a.id ORDER BY operation_id LIMIT 1
);
CREATE INDEX recovery_input_cursor ON transfer_files(id*8);
CREATE INDEX recovery_normalization_cursor ON normalization_runs(id*8+1);
CREATE INDEX recovery_pending_cursor ON publication_destinations(id*8+2) WHERE published_at IS NULL;
CREATE INDEX recovery_retained_cursor ON publication_replacement_journal(
 destination_id*8+CASE WHEN state='restored' THEN 4 ELSE 3 END
);
CREATE INDEX recovery_input_job_cursor ON transfer_files(job_id,id);
CREATE INDEX recovery_normalization_job_cursor ON normalization_runs(job_id,id);
CREATE INDEX recovery_pending_job_cursor ON publication_destinations(job_id,id)
 WHERE published_at IS NULL;
CREATE INDEX recovery_pending_location_cursor ON publication_destinations(location_id,id)
 WHERE published_at IS NULL;
CREATE INDEX recovery_pending_job_location_cursor
 ON publication_destinations(job_id,location_id,id) WHERE published_at IS NULL;
CREATE INDEX recovery_destination_job_cursor ON publication_destinations(job_id,id);
CREATE INDEX recovery_destination_location_cursor ON publication_destinations(location_id,id);
CREATE INDEX recovery_destination_job_location_cursor
 ON publication_destinations(job_id,location_id,id);
