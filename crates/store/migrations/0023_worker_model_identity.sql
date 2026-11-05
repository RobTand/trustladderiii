-- Q53 / #828: immutable dispatch identity belongs with the retained ending.
CREATE TABLE tl3_worker_run_models (
    run_key text NOT NULL,
    attempt bigint NOT NULL,
    model text NOT NULL,
    family text NOT NULL,
    PRIMARY KEY (run_key, attempt),
    FOREIGN KEY (run_key, attempt) REFERENCES tl3_worker_runs (run_key, attempt)
);
CREATE TRIGGER tl3_worker_run_models_append_only BEFORE UPDATE OR DELETE OR TRUNCATE
    ON tl3_worker_run_models FOR EACH STATEMENT EXECUTE FUNCTION tl3_append_only();

-- Fold identities from an earlier #828 build before its transient rows retire.
WITH identities AS MATERIALIZED (
    SELECT row_id::jsonb AS attempt_id, value
    FROM tl3_world WHERE table_name = 'model_attempts'
)
INSERT INTO tl3_worker_run_models (run_key, attempt, model, family)
SELECT ending.run_key, ending.attempt, ending.model,
    COALESCE(identity.value->'families'->>ending.model,
        CASE WHEN identity.value->>'model' = ending.model THEN identity.value->>'family' END)
FROM identities identity JOIN tl3_worker_runs ending
  ON identity.attempt_id->>0 = ending.run_key
 AND (identity.attempt_id->>1)::bigint = ending.attempt
WHERE COALESCE(identity.value->'families'->>ending.model,
    CASE WHEN identity.value->>'model' = ending.model THEN identity.value->>'family' END) IS NOT NULL;

DO $grant$
DECLARE login record;
BEGIN
    FOR login IN SELECT rolname FROM pg_catalog.pg_roles
        WHERE rolcanlogin AND NOT rolsuper AND rolname <> current_user
          AND rolconfig @> ARRAY['default_transaction_read_only=on']
          AND has_table_privilege(oid, 'public.tl3_world', 'SELECT')
    LOOP
        EXECUTE format('GRANT SELECT ON tl3_worker_run_models TO %I', login.rolname);
    END LOOP;
END
$grant$;
