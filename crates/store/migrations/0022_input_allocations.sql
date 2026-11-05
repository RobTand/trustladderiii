-- State-table "Counter headroom": a refused input also rolls back allocated IDs.
-- Keep the old sequences as floors when upgrading an existing database. Reading
-- a sequence does not advance it. A locked high-water row preserves committed
-- IDs even when their business rows are later deleted; rollback undoes its update.
CREATE TABLE tl3_input_allocations (
    relation regclass NOT NULL,
    column_name name NOT NULL,
    last_id bigint NOT NULL,
    PRIMARY KEY (relation, column_name)
);

CREATE FUNCTION tl3_transactional_id(relation regclass, column_name name, old_sequence regclass)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE allocated bigint;
BEGIN
    -- The conflict update locks the allocator row, without upgrading any lock
    -- on the business table. Concurrent read-committed allocators see its latest ID.
    EXECUTE format('SELECT GREATEST(COALESCE((SELECT MAX(%I) FROM %s), 0) + 1,
        (SELECT last_value + is_called::integer FROM %s))', column_name, relation, old_sequence)
        INTO allocated;
    INSERT INTO tl3_input_allocations AS counter VALUES (relation, column_name, allocated)
    ON CONFLICT ON CONSTRAINT tl3_input_allocations_pkey
        DO UPDATE SET last_id = GREATEST(counter.last_id + 1, EXCLUDED.last_id)
    RETURNING last_id INTO allocated;
    RETURN allocated;
END
$$;

-- Ordinary SQL inserts must share the Rust allocator: an identity sequence that
-- Rust no longer advances would otherwise issue an already committed ID. Replace
-- each identity with the transactional default, preserving its sequence's floor.
DO $$
DECLARE
    allocation record;
    sequence_name text;
    sequence_value bigint;
    sequence_called boolean;
BEGIN
    FOR allocation IN SELECT * FROM (VALUES
        ('tl3_intern', 'id'),
        ('tl3_postmortems', 'id'),
        ('tl3_postmortem_recommendations', 'id'),
        ('tl3_code_grades', 'id'),
        ('tl3_phase_history', 'id'),
        ('tl3_memory_outbox', 'sequence')
    ) AS columns(relation, column_name)
    LOOP
        sequence_name := allocation.relation || '_' || allocation.column_name || '_seq';
        EXECUTE format('SELECT last_value, is_called FROM %I', sequence_name)
            INTO sequence_value, sequence_called;
        EXECUTE format('ALTER TABLE %I ALTER COLUMN %I DROP IDENTITY',
            allocation.relation, allocation.column_name);
        EXECUTE format('CREATE SEQUENCE %I AS bigint', sequence_name);
        PERFORM setval(sequence_name::regclass, sequence_value, sequence_called);
        EXECUTE format('ALTER SEQUENCE %I OWNED BY %I.%I', sequence_name,
            allocation.relation, allocation.column_name);
        EXECUTE format('ALTER TABLE %I ALTER COLUMN %I SET DEFAULT
            tl3_transactional_id(%L::regclass, %L::name, %L::regclass)',
            allocation.relation, allocation.column_name,
            allocation.relation, allocation.column_name, sequence_name);
    END LOOP;
END
$$;

-- Preserve direct SQL inserts used by store tooling as well as the Rust API.
ALTER TABLE tl3_world ALTER COLUMN arrival SET DEFAULT
    tl3_transactional_id('tl3_world', 'arrival', 'tl3_world_arrival_seq');

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
