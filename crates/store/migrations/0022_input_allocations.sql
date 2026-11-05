-- State-table "Counter headroom": a refused input also rolls back allocated IDs.
-- Keep the old sequences as floors when upgrading an existing database. Reading
-- a sequence does not advance it. Transaction locks serialize all controller allocators;
-- the inserted row, rather than nextval, owns the new ID until commit.
CREATE FUNCTION tl3_transactional_id(relation regclass, column_name name, old_sequence regclass)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE allocated bigint;
BEGIN
    -- Unlike a table-lock upgrade inside INSERT, this cannot deadlock with
    -- another inserter's already-acquired RowExclusiveLock.
    PERFORM pg_advisory_xact_lock((771::bigint << 32) | relation::oid::bigint);
    EXECUTE format('SELECT GREATEST(COALESCE((SELECT MAX(%I) FROM %s), 0) + 1,
        (SELECT last_value + is_called::integer FROM %s))', column_name, relation, old_sequence)
        INTO allocated;
    RETURN allocated;
END
$$;

CREATE OR REPLACE FUNCTION tl3_track_phase() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM tl3_node_phases WHERE node_id = OLD.row_id;
    ELSIF NEW.value->'phase' IS NULL THEN
        DELETE FROM tl3_node_phases WHERE node_id = NEW.row_id;
    ELSIF TG_OP = 'INSERT' OR OLD.value->'phase' IS DISTINCT FROM NEW.value->'phase' THEN
        INSERT INTO tl3_node_phases (node_id, phase, since)
        SELECT NEW.row_id, NEW.value->'phase', committed_at FROM tl3_events WHERE revision = NEW.version
        ON CONFLICT (node_id) DO UPDATE SET phase = EXCLUDED.phase, since = EXCLUDED.since;
        INSERT INTO tl3_phase_history (id, node_id, phase, since) OVERRIDING SYSTEM VALUE
        SELECT tl3_transactional_id('tl3_phase_history', 'id', 'tl3_phase_history_id_seq'),
            NEW.row_id, NEW.value->'phase', committed_at FROM tl3_events WHERE revision = NEW.version;
    END IF;
    RETURN NULL;
END
$$;
