-- Worker completion evidence outlives progress rows and attempt directories.
-- Effect acknowledgement times are deliberately not used here.
CREATE TABLE tl3_worker_runs (
    run_key text NOT NULL,
    attempt bigint NOT NULL CHECK (attempt >= 0),
    node_id text NOT NULL,
    role text NOT NULL,
    facts jsonb NOT NULL,
    started_ms bigint GENERATED ALWAYS AS ((facts->>'started_ms')::bigint) STORED NOT NULL,
    finished_ms bigint GENERATED ALWAYS AS ((facts->>'finished_ms')::bigint) STORED,
    outcome text GENERATED ALWAYS AS (facts->>'outcome') STORED NOT NULL
        CHECK (outcome IN ('succeeded', 'failed', 'quota', 'stopped', 'crashed')),
    release text GENERATED ALWAYS AS (facts->>'release') STORED,
    model text GENERATED ALWAYS AS (facts->>'model') STORED NOT NULL,
    input_tokens bigint GENERATED ALWAYS AS ((facts->>'input_tokens')::bigint) STORED CHECK (input_tokens >= 0),
    cached_tokens bigint GENERATED ALWAYS AS ((facts->>'cached_tokens')::bigint) STORED CHECK (cached_tokens >= 0),
    output_tokens bigint GENERATED ALWAYS AS ((facts->>'output_tokens')::bigint) STORED CHECK (output_tokens >= 0),
    session_id text GENERATED ALWAYS AS (facts->>'session_id') STORED,
    trace_id text GENERATED ALWAYS AS (facts->>'trace_id') STORED,
    failure_origin text GENERATED ALWAYS AS (facts->'failure'->>'origin') STORED,
    failure_class text GENERATED ALWAYS AS (facts->'failure'->>'class') STORED,
    failure_status integer GENERATED ALWAYS AS ((facts->'failure'->>'status')::integer) STORED,
    failure_request_id text GENERATED ALWAYS AS (facts->'failure'->>'request_id') STORED,
    PRIMARY KEY (run_key, attempt)
);
CREATE INDEX tl3_worker_runs_node ON tl3_worker_runs (node_id, started_ms DESC);
CREATE TRIGGER tl3_worker_runs_append_only BEFORE UPDATE OR DELETE OR TRUNCATE
    ON tl3_worker_runs FOR EACH STATEMENT EXECUTE FUNCTION tl3_append_only();
CREATE TRIGGER tl3_worker_runs_changes AFTER INSERT ON tl3_worker_runs
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();

DO $grant$
DECLARE login record;
BEGIN
    FOR login IN SELECT rolname FROM pg_catalog.pg_roles
        WHERE rolcanlogin AND NOT rolsuper AND rolname <> current_user
          AND rolconfig @> ARRAY['default_transaction_read_only=on']
          AND has_table_privilege(oid, 'public.tl3_world', 'SELECT')
    LOOP
        EXECUTE format('GRANT SELECT ON tl3_worker_runs TO %I', login.rolname);
    END LOOP;
END
$grant$;
