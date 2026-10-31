ALTER TABLE tl3_memory_outbox ADD COLUMN parked_at timestamptz;
UPDATE tl3_memory_outbox SET parked_at = CURRENT_TIMESTAMP WHERE parked_reason IS NOT NULL;
CREATE INDEX tl3_memory_http_expiry ON tl3_memory_outbox (parked_at) WHERE parked_reason = 'http';
