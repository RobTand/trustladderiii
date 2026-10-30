CREATE TABLE tl3_meta (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    revision bigint NOT NULL CHECK (revision >= 0),
    epoch bigint NOT NULL CHECK (epoch >= 0)
);
INSERT INTO tl3_meta (revision, epoch) VALUES (0, 0);

CREATE TABLE tl3_events (
    event_key text PRIMARY KEY CHECK (event_key <> ''),
    revision bigint NOT NULL UNIQUE CHECK (revision > 0),
    epoch bigint NOT NULL CHECK (epoch >= 0),
    payload jsonb NOT NULL,
    result jsonb NOT NULL,
    committed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE FUNCTION tl3_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'TL3 audit records are append-only';
END
$$;
CREATE TRIGGER tl3_events_append_only BEFORE UPDATE OR DELETE OR TRUNCATE
    ON tl3_events FOR EACH STATEMENT EXECUTE FUNCTION tl3_append_only();

CREATE TABLE tl3_world (
    table_name text NOT NULL CHECK (table_name <> ''),
    row_id text NOT NULL CHECK (row_id <> ''),
    value jsonb NOT NULL,
    version bigint NOT NULL REFERENCES tl3_events (revision),
    PRIMARY KEY (table_name, row_id)
);

CREATE TYPE tl3_effect_state AS ENUM (
    'Intended', 'Sent', 'Uncertain', 'Confirmed', 'Failed', 'Cancelling', 'Cancelled'
);
CREATE TABLE tl3_effects (
    effect_key text PRIMARY KEY CHECK (effect_key <> ''),
    epoch bigint NOT NULL CHECK (epoch >= 0),
    kind text NOT NULL CHECK (kind <> ''),
    payload jsonb NOT NULL,
    state tl3_effect_state NOT NULL DEFAULT 'Intended',
    created_revision bigint NOT NULL REFERENCES tl3_events (revision),
    updated_revision bigint NOT NULL REFERENCES tl3_events (revision)
);
CREATE INDEX tl3_effects_pending ON tl3_effects (created_revision, effect_key)
    WHERE state IN ('Intended', 'Sent', 'Uncertain', 'Cancelling');

-- docs/state-table.md, "The effect machine". The API and direct SQL share this guard.
CREATE FUNCTION tl3_guard_effect() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.effect_key <> OLD.effect_key OR NEW.kind <> OLD.kind
        OR NEW.payload <> OLD.payload OR NEW.created_revision <> OLD.created_revision THEN
        RAISE EXCEPTION 'effect identity and payload are immutable';
    END IF;
    IF NEW.epoch <> OLD.epoch AND NOT (
        OLD.state = 'Intended' AND NEW.state = 'Intended' AND NEW.epoch > OLD.epoch
    ) THEN
        RAISE EXCEPTION 'only Intended effects can move to a new epoch';
    END IF;
    IF NEW.state <> OLD.state AND NOT (
        (OLD.state = 'Intended' AND NEW.state IN ('Sent', 'Cancelled')) OR
        (OLD.state = 'Sent' AND NEW.state IN ('Confirmed', 'Intended', 'Failed', 'Uncertain', 'Cancelling')) OR
        (OLD.state = 'Uncertain' AND NEW.state IN ('Confirmed', 'Intended', 'Cancelling')) OR
        (OLD.state = 'Cancelling' AND NEW.state IN ('Confirmed', 'Cancelled'))
    ) THEN
        RAISE EXCEPTION 'illegal effect transition: % -> %', OLD.state, NEW.state;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER tl3_effect_guard BEFORE UPDATE ON tl3_effects
    FOR EACH ROW EXECUTE FUNCTION tl3_guard_effect();

CREATE TABLE tl3_credit (
    node_id text NOT NULL CHECK (node_id <> ''),
    reason text NOT NULL CHECK (reason <> ''),
    root_id text NOT NULL CHECK (root_id <> ''),
    amount bigint NOT NULL,
    revision bigint NOT NULL REFERENCES tl3_events (revision),
    PRIMARY KEY (node_id, reason)
);
CREATE TRIGGER tl3_credit_append_only BEFORE UPDATE OR DELETE OR TRUNCATE
    ON tl3_credit FOR EACH STATEMENT EXECUTE FUNCTION tl3_append_only();

CREATE TYPE tl3_intern_kind AS ENUM ('Repo', 'Commit', 'Path', 'Issue', 'PullRequest');
CREATE TABLE tl3_intern (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK (id > 0),
    kind tl3_intern_kind NOT NULL,
    scope text NOT NULL,
    value text NOT NULL CHECK (value <> ''),
    UNIQUE (kind, scope, value)
);
