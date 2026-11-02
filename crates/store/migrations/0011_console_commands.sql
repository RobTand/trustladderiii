-- PRD #327, C3: console commands. The console's login can read every table
-- but write none. tl3_console_command is its one write path: it files a typed
-- command into ingress as the `console` source, records it as pending, and
-- wakes tl3d. tl3d stays the only writer of the world and the event log.
CREATE TABLE public.tl3_console_commands (
    id text PRIMARY KEY,
    node_id bigint NOT NULL CHECK (node_id >= 0),
    action jsonb NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    status text NOT NULL DEFAULT 'pending',
    reason text,
    revision bigint,
    settled_at timestamptz
);
CREATE INDEX tl3_console_commands_received ON public.tl3_console_commands (received_at DESC);
CREATE INDEX tl3_console_commands_pending ON public.tl3_console_commands (id) WHERE status = 'pending';

-- A SECURITY DEFINER function runs with the owner's rights, so it resolves
-- names only in pg_catalog and names TL3's tables in full.
CREATE FUNCTION public.tl3_console_command(command_id text, node bigint, action jsonb) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
BEGIN
    IF command_id IS NULL
        OR command_id !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' THEN
        RAISE EXCEPTION 'console command ids are lowercase UUIDs';
    END IF;
    IF node IS NULL OR node < 0 OR action IS NULL OR jsonb_typeof(action) <> 'object' THEN
        RAISE EXCEPTION 'console commands name a request and an action object';
    END IF;
    -- A resent id files nothing, whatever the caps below say now.
    IF EXISTS (SELECT 1 FROM public.tl3_console_commands WHERE id = command_id) THEN
        RETURN false;
    END IF;
    -- Ingress, this table and tl3d's append-only event log all keep the action.
    IF octet_length(action::text) > 65536 THEN
        RAISE EXCEPTION 'console commands are at most 65,536 bytes'
            USING ERRCODE = 'string_data_right_truncation';
    END IF;
    IF (SELECT count(*) FROM public.tl3_console_commands WHERE status = 'pending') >= 1000 THEN
        RAISE EXCEPTION '1,000 console commands are waiting for tl3d; file more once it applies them'
            USING ERRCODE = 'configuration_limit_exceeded';
    END IF;
    INSERT INTO public.tl3_console_commands (id, node_id, action) VALUES (command_id, node, action)
    ON CONFLICT (id) DO NOTHING;
    IF NOT FOUND THEN
        RETURN false;
    END IF;
    INSERT INTO public.tl3_ingress (source, delivery_key, event_name, payload)
    VALUES ('console', command_id, 'command',
            convert_to(jsonb_build_object('id', command_id, 'node', node, 'action', action)::text, 'UTF8'));
    PERFORM pg_notify('tl3_ingress', 'console');
    PERFORM pg_notify('tl3_changes', 'tl3_console_commands');
    RETURN true;
END
$$;
REVOKE ALL ON FUNCTION public.tl3_console_command(text, bigint, jsonb) FROM PUBLIC;

-- tl3d settles a command in the commit keyed `console:<id>`; the outcome is in
-- its result. The trigger runs inside that commit. It must never fail it.
CREATE FUNCTION public.tl3_console_outcome() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog, pg_temp AS $$
BEGIN
    UPDATE public.tl3_console_commands
    SET status = CASE WHEN NEW.result->'console'->>'status' = 'applied' THEN 'applied' ELSE 'refused' END,
        reason = NEW.result->'console'->>'reason',
        revision = NEW.revision,
        settled_at = NEW.committed_at
    WHERE id = substr(NEW.event_key, 9);
    RETURN NULL;
END
$$;
CREATE TRIGGER tl3_events_console AFTER INSERT ON public.tl3_events
    FOR EACH ROW WHEN (starts_with(NEW.event_key, 'console:')) EXECUTE FUNCTION public.tl3_console_outcome();

-- Logins that grant_reader set up before this migration may file commands too.
DO $grant$
DECLARE login record;
BEGIN
    FOR login IN SELECT rolname FROM pg_catalog.pg_roles
        WHERE rolcanlogin AND NOT rolsuper AND rolname <> current_user
          AND rolconfig @> ARRAY['default_transaction_read_only=on']
          AND has_table_privilege(oid, 'public.tl3_world', 'SELECT')
    LOOP
        EXECUTE format('GRANT EXECUTE ON FUNCTION public.tl3_console_command(text, bigint, jsonb) TO %I', login.rolname);
    END LOOP;
END
$grant$;
