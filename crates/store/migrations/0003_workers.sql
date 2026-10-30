CREATE TABLE tl3_worker_materials (
    node_id text PRIMARY KEY,
    inputs jsonb NOT NULL
);

CREATE TABLE tl3_worker_progress (
    run_key text PRIMARY KEY,
    node_id text NOT NULL,
    epoch bigint NOT NULL,
    attempt bigint NOT NULL,
    output_offset bigint NOT NULL,
    unit text NOT NULL,
    last_progress_ms bigint NOT NULL
);
CREATE INDEX tl3_worker_progress_node ON tl3_worker_progress (node_id, last_progress_ms DESC);
