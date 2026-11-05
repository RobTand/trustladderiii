-- Q27/#766: dispatch evidence is indexed by approval operation and effect key.
-- Audit events remain append-only. No history replay runs during Store::open.
CREATE TABLE tl3_dispatch_evidence (
    effect_key text PRIMARY KEY REFERENCES tl3_effects(effect_key),
    operation jsonb NOT NULL,
    sends bigint NOT NULL DEFAULT 0,
    legacy_sends bigint NOT NULL DEFAULT 0
);
CREATE INDEX tl3_dispatch_operation ON tl3_dispatch_evidence(operation, effect_key);

CREATE FUNCTION tl3_approval_operation(payload jsonb) RETURNS jsonb
LANGUAGE sql IMMUTABLE STRICT AS $$
    SELECT jsonb_build_object('repository', payload->'repository',
        'issue', payload->'issue', 'generation', payload->'generation',
        'head_sha', payload->'head_sha',
        'output', jsonb_build_object('Effect', jsonb_build_object(
            'node', payload->'output'->'Effect'->'node', 'kind', 'PostApproval')))
$$;

CREATE FUNCTION tl3_record_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.state = 'Sent' AND NEW.payload->'output'->'Effect'->>'kind' = 'PostApproval' THEN
        IF TG_OP = 'INSERT' OR OLD.state <> 'Sent' THEN
            INSERT INTO tl3_dispatch_evidence(effect_key, operation, sends)
                VALUES (NEW.effect_key, tl3_approval_operation(NEW.payload), 1)
                ON CONFLICT (effect_key) DO UPDATE SET sends = tl3_dispatch_evidence.sends + 1;
        END IF;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER tl3_record_dispatch AFTER INSERT OR UPDATE ON tl3_effects
    FOR EACH ROW EXECUTE FUNCTION tl3_record_dispatch();

CREATE TABLE tl3_dispatch_migration (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    cursor bigint NOT NULL DEFAULT 0,
    through_revision bigint NOT NULL
);
INSERT INTO tl3_dispatch_migration(through_revision)
    SELECT COALESCE(MAX(revision), 0) FROM tl3_events;
