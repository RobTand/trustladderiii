-- #422: TL3's decisions about recalled memories. Each row is the audit trail
-- for one decision. One verified flag hides a memory from TL3's recall; a
-- second independent signal, or Rob, retires it in Hindsight. Undelivered
-- retirements and reverts are the queue to Hindsight, and unannounced hides and
-- retirements the queue to the operations issue. Rejected flags are only kept.
CREATE TABLE tl3_memory_curations (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    operation_id text NOT NULL UNIQUE,
    memory_id text NOT NULL CHECK (memory_id <> '' AND char_length(memory_id) <= 256),
    state text NOT NULL CHECK (state IN ('hidden', 'invalidated', 'valid')),
    verdict text NOT NULL CHECK (verdict IN ('accepted', 'rejected')),
    source text NOT NULL CHECK (source <> '' AND char_length(source) <= 64),
    request text,
    run text,
    claim text CHECK (char_length(claim) <= 1000),
    reason text NOT NULL CHECK (reason <> '' AND char_length(reason) <= 4000),
    evidence jsonb NOT NULL CHECK (octet_length(evidence::text) <= 16384),
    recorded_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    delivered_at timestamptz,
    http_status integer,
    announced_at timestamptz
);
CREATE INDEX tl3_memory_curations_pending ON tl3_memory_curations (sequence)
    WHERE verdict = 'accepted' AND state <> 'hidden' AND delivered_at IS NULL AND http_status IS NULL;
CREATE INDEX tl3_memory_curations_unannounced ON tl3_memory_curations (sequence)
    WHERE verdict = 'accepted' AND state <> 'valid' AND announced_at IS NULL;
CREATE INDEX tl3_memory_curations_memory ON tl3_memory_curations (memory_id, sequence);
