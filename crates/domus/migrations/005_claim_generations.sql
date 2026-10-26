ALTER TABLE jobs ADD COLUMN claim_generation INTEGER NOT NULL DEFAULT 0
    CHECK(claim_generation >= 0);
CREATE INDEX location_scan_history ON scans(location_id, id DESC);
