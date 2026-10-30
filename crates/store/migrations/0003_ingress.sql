CREATE TABLE tl3_ingress (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source text NOT NULL CHECK (source <> ''),
    delivery_key text NOT NULL CHECK (delivery_key <> ''),
    payload jsonb NOT NULL,
    consumed boolean NOT NULL DEFAULT false,
    UNIQUE (source, delivery_key)
);
CREATE INDEX tl3_ingress_pending ON tl3_ingress (sequence) WHERE NOT consumed;

CREATE TABLE tl3_ingress_cursors (
    source text PRIMARY KEY CHECK (source <> ''),
    cursor jsonb NOT NULL
);
