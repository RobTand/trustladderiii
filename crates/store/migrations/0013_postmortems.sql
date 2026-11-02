-- #403 (Q32): owed postmortems, their results and code grades. These tables are
-- not world state. Owed records and grades are written in the controller commit
-- that makes them owed, so a crash can't lose one; the runner writes results.
CREATE TABLE tl3_postmortems (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    node_id text NOT NULL,
    -- close, question:<id>, incident:<id> or rob:<console command id>
    trigger text NOT NULL CHECK (trigger <> ''),
    reason text NOT NULL CHECK (reason IN
        ('needed_fix', 'retry', 'escalation', 'closed_without_merge', 'sampled', 'rob_asked')),
    sample_share double precision CHECK (sample_share BETWEEN 0 AND 1),
    repo text NOT NULL,
    issue bigint NOT NULL,
    title text NOT NULL,
    status text NOT NULL DEFAULT 'owed' CHECK (status IN ('owed', 'running', 'done', 'failed')),
    revision bigint NOT NULL REFERENCES tl3_events (revision),
    owed_at timestamptz NOT NULL,
    started_at timestamptz,
    finished_at timestamptz,
    failed_at timestamptz,
    failure text,
    run_key text,
    rubric_version integer,
    model text,
    cause text CHECK (cause IN ('tl3', 'outside')),
    summary text,
    timeline jsonb,
    process_failures jsonb,
    grades jsonb,
    notes text,
    search tsvector GENERATED ALWAYS AS (
        to_tsvector('english', title || ' ' || coalesce(summary, '') || ' ' || coalesce(notes, ''))
    ) STORED,
    UNIQUE (node_id, trigger)
);
CREATE INDEX tl3_postmortems_status ON tl3_postmortems (status, owed_at);
CREATE INDEX tl3_postmortems_search ON tl3_postmortems USING gin (search);

-- At most three per postmortem. The fingerprint is stable across postmortems,
-- so Q32's curator can count how many point at the same change.
CREATE TABLE tl3_postmortem_recommendations (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    postmortem_id bigint NOT NULL REFERENCES tl3_postmortems (id),
    fingerprint text NOT NULL,
    rank smallint NOT NULL CHECK (rank BETWEEN 1 AND 3),
    phase text NOT NULL,
    kind text NOT NULL,
    target text NOT NULL,
    operation text NOT NULL CHECK (operation IN ('add', 'remove', 'replace')),
    before text NOT NULL,
    after text NOT NULL,
    evidence text NOT NULL,
    effect text NOT NULL,
    search tsvector GENERATED ALWAYS AS (
        to_tsvector('english', target || ' ' || before || ' ' || after || ' ' || evidence || ' ' || effect)
    ) STORED,
    UNIQUE (postmortem_id, rank)
);
CREATE INDEX tl3_postmortem_recommendations_fingerprint ON tl3_postmortem_recommendations (fingerprint);
CREATE INDEX tl3_postmortem_recommendations_search ON tl3_postmortem_recommendations USING gin (search);

-- Append-only: a later fact adds a grade and never changes one.
CREATE TABLE tl3_code_grades (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    node_id text NOT NULL,
    trigger text NOT NULL,
    rubric_version integer NOT NULL,
    dimension text NOT NULL,
    grade smallint CHECK (grade BETWEEN 1 AND 5),
    fact jsonb NOT NULL,
    revision bigint NOT NULL REFERENCES tl3_events (revision),
    recorded_at timestamptz NOT NULL,
    UNIQUE (node_id, trigger, rubric_version, dimension)
);
CREATE INDEX tl3_code_grades_node ON tl3_code_grades (node_id);
CREATE TRIGGER tl3_code_grades_append_only BEFORE UPDATE OR DELETE OR TRUNCATE
    ON tl3_code_grades FOR EACH STATEMENT EXECUTE FUNCTION tl3_append_only();

-- Every phase a node enters, from now on. Earlier phases left no record.
CREATE TABLE tl3_phase_history (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    node_id text NOT NULL,
    phase jsonb NOT NULL,
    since timestamptz NOT NULL
);
CREATE INDEX tl3_phase_history_node ON tl3_phase_history (node_id, since);
CREATE TRIGGER tl3_phase_history_append_only BEFORE UPDATE OR DELETE OR TRUNCATE
    ON tl3_phase_history FOR EACH STATEMENT EXECUTE FUNCTION tl3_append_only();
-- In-flight nodes start from their current phase's entry time.
INSERT INTO tl3_phase_history (node_id, phase, since)
SELECT node_id, phase, since FROM tl3_node_phases;
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
        INSERT INTO tl3_phase_history (node_id, phase, since)
        SELECT NEW.row_id, NEW.value->'phase', committed_at FROM tl3_events WHERE revision = NEW.version;
    END IF;
    RETURN NULL;
END
$$;

-- Settings the console may show and, through tl3_settings_editor, change (#333).
CREATE TABLE tl3_settings (
    key text PRIMARY KEY,
    value jsonb NOT NULL,
    updated_by text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    -- CASE keeps a non-number from reaching the cast: it fails as a check violation.
    CHECK (key <> 'postmortem_sample_share' OR CASE WHEN jsonb_typeof(value) = 'number'
        THEN (value #>> '{}')::double precision BETWEEN 0 AND 1 ELSE false END)
);
-- Q32: a random share of clean merges is also judged.
INSERT INTO tl3_settings (key, value, updated_by) VALUES ('postmortem_sample_share', '0.1', 'migration');
DO $role$ BEGIN
    CREATE ROLE tl3_settings_editor NOLOGIN;
EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
END $role$;
GRANT UPDATE (value, updated_by, updated_at) ON tl3_settings TO tl3_settings_editor;

CREATE TRIGGER tl3_postmortems_changes AFTER INSERT OR UPDATE OR DELETE ON tl3_postmortems
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();
CREATE TRIGGER tl3_postmortem_recommendations_changes AFTER INSERT OR UPDATE OR DELETE ON tl3_postmortem_recommendations
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();
CREATE TRIGGER tl3_code_grades_changes AFTER INSERT OR UPDATE OR DELETE ON tl3_code_grades
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();
CREATE TRIGGER tl3_settings_changes AFTER INSERT OR UPDATE OR DELETE ON tl3_settings
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();

-- Logins that grant_reader set up before this migration read and edit these too.
-- Default privileges already cover SELECT; the grant states it.
DO $grant$
DECLARE login record;
BEGIN
    FOR login IN SELECT rolname FROM pg_catalog.pg_roles
        WHERE rolcanlogin AND NOT rolsuper AND rolname <> current_user
          AND rolconfig @> ARRAY['default_transaction_read_only=on']
          AND has_table_privilege(oid, 'public.tl3_world', 'SELECT')
    LOOP
        EXECUTE format('GRANT SELECT ON tl3_postmortems, tl3_postmortem_recommendations, tl3_code_grades, tl3_phase_history, tl3_settings TO %I', login.rolname);
        EXECUTE format('GRANT tl3_settings_editor TO %I', login.rolname);
    END LOOP;
END
$grant$;
