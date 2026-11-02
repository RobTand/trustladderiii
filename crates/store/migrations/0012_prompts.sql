-- PRD #327, C5: each role's system prompt as versions with an active pointer.
-- These tables are not world state. tl3d seeds the built-in prompts and records
-- the version each worker run uses; the console adds versions and moves the
-- pointer, through the tl3_prompt_editor grants below and nothing else.
CREATE TABLE tl3_prompt_versions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    role text NOT NULL,
    body text NOT NULL CHECK (body <> '' AND octet_length(body) <= 262144),
    builtin boolean NOT NULL DEFAULT false,
    author text NOT NULL CHECK (author <> ''),
    note text NOT NULL CHECK (note <> '' AND char_length(note) <= 500),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (role, id)
);
CREATE TABLE tl3_prompt_active (
    role text PRIMARY KEY,
    version bigint NOT NULL,
    -- The newest built-in version decided on, by a deploy that activated it or
    -- by Rob. A newer built-in version is an update the console shows.
    builtin_seen bigint NOT NULL,
    activated_by text NOT NULL CHECK (activated_by <> ''),
    activated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (role, version) REFERENCES tl3_prompt_versions (role, id),
    FOREIGN KEY (role, builtin_seen) REFERENCES tl3_prompt_versions (role, id)
);
-- A run keeps the version it started with, through retries of the same run.
CREATE TABLE tl3_prompt_runs (
    run_key text NOT NULL,
    role text NOT NULL,
    version bigint NOT NULL,
    started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (run_key, role),
    FOREIGN KEY (role, version) REFERENCES tl3_prompt_versions (role, id)
);

-- Roles are cluster-wide and a deploy may grant the console's login before
-- tl3d migrates, so either side creates this role and the login joins it.
DO $role$ BEGIN
    CREATE ROLE tl3_prompt_editor NOLOGIN;
EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
END $role$;
GRANT INSERT (role, body, author, note) ON tl3_prompt_versions TO tl3_prompt_editor;
GRANT UPDATE (version, builtin_seen, activated_by, activated_at) ON tl3_prompt_active TO tl3_prompt_editor;
