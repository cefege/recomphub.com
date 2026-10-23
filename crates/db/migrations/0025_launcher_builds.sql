CREATE TABLE launcher_builds (
    recipe_id text PRIMARY KEY,
    port_id bigint NOT NULL REFERENCES ports(id) ON DELETE CASCADE,
    manifest jsonb NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    checked_at timestamptz NOT NULL
);

CREATE INDEX launcher_builds_port_id_idx ON launcher_builds(port_id);
