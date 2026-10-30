ALTER TABLE tl3_worker_progress ADD COLUMN retirement_blocked boolean NOT NULL DEFAULT false;

CREATE VIEW tl3_retired_workers AS
SELECT e.effect_key
FROM tl3_effects e
WHERE e.state IN ('Confirmed', 'Failed', 'Cancelled')
  AND NOT EXISTS (
      SELECT 1 FROM tl3_world w
      WHERE w.table_name = 'watches' AND w.row_id = e.effect_key
  );

CREATE TABLE tl3_worker_summary (
    node_id text PRIMARY KEY,
    unit text NOT NULL,
    last_progress_ms bigint NOT NULL
);
