CREATE TABLE tl3_memory_outbox (
    sequence bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    operation_id text PRIMARY KEY,
    item jsonb NOT NULL CHECK (octet_length(item::text) <= 32768)
);
