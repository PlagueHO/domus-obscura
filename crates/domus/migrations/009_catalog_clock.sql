ALTER TABLE catalog_gate ADD COLUMN delay_seconds INTEGER NOT NULL DEFAULT 0
    CHECK(delay_seconds BETWEEN 0 AND 4294967295);

-- The old schema did not retain the original delay. Require acknowledgment
-- instead of deriving a shorter interval from a possibly changed wall clock.
UPDATE catalog_gate SET lease_until_ms=0, next_request_ms=9223372036854775807,
    delay_seconds=4294967295 WHERE lease_until_ms<>0 OR next_request_ms<>0;
