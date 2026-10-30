CREATE SEQUENCE tl3_world_arrival_seq;
ALTER TABLE tl3_world ADD COLUMN arrival bigint;
-- Earlier versions stored no order within a transaction. Preserve revision order.
WITH ordered AS (
    SELECT table_name, row_id, row_number() OVER (ORDER BY version, table_name, row_id) AS arrival
    FROM tl3_world
)
UPDATE tl3_world AS world SET arrival = ordered.arrival FROM ordered
WHERE world.table_name = ordered.table_name AND world.row_id = ordered.row_id;
SELECT setval('tl3_world_arrival_seq', COALESCE(MAX(arrival), 1), COUNT(*) > 0) FROM tl3_world;
ALTER TABLE tl3_world ALTER COLUMN arrival SET DEFAULT nextval('tl3_world_arrival_seq');
ALTER TABLE tl3_world ALTER COLUMN arrival SET NOT NULL;
ALTER SEQUENCE tl3_world_arrival_seq OWNED BY tl3_world.arrival;
CREATE UNIQUE INDEX tl3_world_arrival ON tl3_world (arrival);
