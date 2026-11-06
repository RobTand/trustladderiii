-- #845/Q27: full declared authorization scope is independent of kernel locks.
-- No backfill: absent rows retain the legacy footprint behavior until planning.
ALTER TABLE tl3_world ADD CONSTRAINT tl3_declared_scope_files CHECK (
    table_name <> 'declared_scopes' OR (
        jsonb_typeof(value) = 'array'
        AND NOT jsonb_path_exists(value, '$[*] ? (@.type() != "string")')
    )
);
