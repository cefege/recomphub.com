-- Public image identities are semantic, stable, and independent of renderer internals.
ALTER TABLE media
    ADD COLUMN public_filename text UNIQUE
    CHECK (
        public_filename IS NULL
        OR public_filename ~ '^[a-z0-9]+(-[a-z0-9]+)*-[1-9][0-9]*[.]avif$'
    );
