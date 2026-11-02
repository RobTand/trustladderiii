-- #422: TL3's decisions about recalled memories. Each row is the audit trail
-- for one decision and, while undelivered, the queue that sends it to
-- Hindsight. Rejected flags are kept with their reason and never sent.
CREATE TABLE tl3_memory_curations (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    operation_id text NOT NULL UNIQUE,
    memory_id text NOT NULL CHECK (memory_id <> '' AND char_length(memory_id) <= 256),
    state text NOT NULL CHECK (state IN ('invalidated', 'valid')),
    verdict text NOT NULL CHECK (verdict IN ('accepted', 'rejected')),
    source text NOT NULL CHECK (source <> '' AND char_length(source) <= 64),
    request text,
    run text,
    reason text NOT NULL CHECK (reason <> '' AND char_length(reason) <= 4000),
    evidence jsonb NOT NULL CHECK (octet_length(evidence::text) <= 16384),
    recorded_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    delivered_at timestamptz,
    http_status integer
);
CREATE INDEX tl3_memory_curations_pending ON tl3_memory_curations (sequence)
    WHERE verdict = 'accepted' AND delivered_at IS NULL AND http_status IS NULL;
CREATE INDEX tl3_memory_curations_memory ON tl3_memory_curations (memory_id, sequence);
