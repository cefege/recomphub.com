-- Generic setup text cannot identify which system its steps apply to. Existing descriptions
-- remain readable while describe-ports regenerates repository-backed desktop guides.
ALTER TABLE port_descriptions
    DROP COLUMN setup,
    ADD COLUMN installation jsonb
        CHECK (installation IS NULL OR jsonb_typeof(installation) = 'object');
