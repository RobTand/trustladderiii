-- PRD #327, change feed: readers LISTEN on tl3_changes instead of polling.
-- Postgres delivers a notification at commit and folds identical payloads
-- from one transaction into one.
CREATE FUNCTION tl3_notify_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('tl3_changes', TG_TABLE_NAME);
    RETURN NULL;
END
$$;
-- Every controller commit advances tl3_meta.
CREATE TRIGGER tl3_meta_changes AFTER UPDATE ON tl3_meta
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();
CREATE TRIGGER tl3_worker_progress_changes AFTER INSERT OR UPDATE OR DELETE ON tl3_worker_progress
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();
CREATE TRIGGER tl3_worker_summary_changes AFTER INSERT OR UPDATE OR DELETE ON tl3_worker_summary
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();

-- When each node entered its current phase, stamped with the commit's time.
-- The trigger runs inside the controller's commit, so tl3d stays the only writer.
CREATE TABLE tl3_node_phases (
    node_id text PRIMARY KEY,
    phase jsonb NOT NULL,
    since timestamptz NOT NULL
);
CREATE FUNCTION tl3_track_phase() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM tl3_node_phases WHERE node_id = OLD.row_id;
    ELSIF NEW.value->'phase' IS NULL THEN
        DELETE FROM tl3_node_phases WHERE node_id = NEW.row_id;
    ELSIF TG_OP = 'INSERT' OR OLD.value->'phase' IS DISTINCT FROM NEW.value->'phase' THEN
        INSERT INTO tl3_node_phases (node_id, phase, since)
        SELECT NEW.row_id, NEW.value->'phase', committed_at FROM tl3_events WHERE revision = NEW.version
        ON CONFLICT (node_id) DO UPDATE SET phase = EXCLUDED.phase, since = EXCLUDED.since;
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER tl3_world_phase AFTER INSERT OR UPDATE ON tl3_world
    FOR EACH ROW WHEN (NEW.table_name = 'nodes') EXECUTE FUNCTION tl3_track_phase();
CREATE TRIGGER tl3_world_phase_removed AFTER DELETE ON tl3_world
    FOR EACH ROW WHEN (OLD.table_name = 'nodes') EXECUTE FUNCTION tl3_track_phase();
-- Earlier phase changes left no record. A node's last change is the closest known time.
INSERT INTO tl3_node_phases (node_id, phase, since)
SELECT world.row_id, world.value->'phase', events.committed_at
FROM tl3_world world JOIN tl3_events events ON events.revision = world.version
WHERE world.table_name = 'nodes' AND world.value->'phase' IS NOT NULL;
