ALTER TABLE tl3_memory_outbox
    ADD COLUMN parked_reason text CHECK (parked_reason IN ('capacity', 'http')),
    ADD COLUMN http_status integer CHECK (http_status BETWEEN 400 AND 499);
CREATE INDEX tl3_memory_pending ON tl3_memory_outbox (sequence) WHERE parked_reason IS NULL;
CREATE TABLE tl3_memory_health (
    singleton boolean PRIMARY KEY CHECK (singleton),
    rejected_status integer NOT NULL CHECK (rejected_status IN (401, 403))
);
UPDATE tl3_memory_outbox SET parked_reason = 'capacity' WHERE sequence IN (
    SELECT sequence FROM tl3_memory_outbox ORDER BY sequence DESC OFFSET 10000
);
