-- #403 slice B: the MLflow assessments a postmortem's judges logged on the
-- node's traces, so the console can link each grade to its evidence.
ALTER TABLE tl3_postmortems ADD COLUMN assessment_ids jsonb;

-- A run that fails for a reason worth retrying makes its postmortem owed
-- again, at most twice, behind postmortems owed since (#486).
ALTER TABLE tl3_postmortems ADD COLUMN attempts integer NOT NULL DEFAULT 0
    CHECK (attempts BETWEEN 0 AND 2);
DROP INDEX tl3_postmortems_status;
CREATE INDEX tl3_postmortems_status ON tl3_postmortems (status, attempts, owed_at);

-- Every claim a runner made, and how its run ended: the daily claim limit
-- counts them, and failed runs in a row pause the runner (#486).
CREATE TABLE tl3_postmortem_claims (
    run_key text PRIMARY KEY,
    postmortem_id bigint NOT NULL REFERENCES tl3_postmortems (id),
    revision bigint NOT NULL REFERENCES tl3_events (revision),
    claimed_at timestamptz NOT NULL,
    outcome text CHECK (outcome IN ('done', 'failed', 'released'))
);
CREATE INDEX tl3_postmortem_claims_claimed_at ON tl3_postmortem_claims (claimed_at);
CREATE INDEX tl3_postmortem_claims_revision ON tl3_postmortem_claims (revision);
CREATE TRIGGER tl3_postmortem_claims_changes AFTER INSERT OR UPDATE OR DELETE ON tl3_postmortem_claims
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();

-- Logins that grant_reader set up before this migration read it too.
DO $grant$
DECLARE login record;
BEGIN
    FOR login IN SELECT rolname FROM pg_catalog.pg_roles
        WHERE rolcanlogin AND NOT rolsuper AND rolname <> current_user
          AND rolconfig @> ARRAY['default_transaction_read_only=on']
          AND has_table_privilege(oid, 'public.tl3_world', 'SELECT')
    LOOP
        EXECUTE format('GRANT SELECT ON tl3_postmortem_claims TO %I', login.rolname);
    END LOOP;
END
$grant$;
