-- Q53 / #828: immutable identity values have a finite evidence window.
-- Endings remain in the existing audit log. Missing identity never becomes
-- evidence from current configuration. The controller still has reset/probes.
ALTER TABLE tl3_worker_run_models ADD COLUMN retention_id bigint GENERATED ALWAYS AS IDENTITY;
CREATE UNIQUE INDEX tl3_worker_run_models_retention ON tl3_worker_run_models (retention_id);
DROP TRIGGER tl3_worker_run_models_append_only ON tl3_worker_run_models;
CREATE TRIGGER tl3_worker_run_models_immutable BEFORE UPDATE OR TRUNCATE
    ON tl3_worker_run_models FOR EACH STATEMENT EXECUTE FUNCTION tl3_append_only();

-- Match Store::WORKER_IDENTITY_LIMIT. The migration bounds any prior B cache;
-- each later import enforces the same bound in its serialized transaction.
DELETE FROM tl3_worker_run_models
WHERE retention_id <= (SELECT max(retention_id) FROM tl3_worker_run_models) - 65536;
