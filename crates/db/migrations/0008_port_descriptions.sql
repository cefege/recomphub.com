-- Generated port descriptions. github-sync stores the repository snapshot it already fetches
-- (sources) during the weekly scan. The worker describe-ports command turns each snapshot into
-- one generated description. A description is stale when its source_hash no longer matches the
-- sources and the known facts, so it is regenerated then.

CREATE TABLE port_description_sources (
    port_id               bigint PRIMARY KEY REFERENCES ports (id) ON DELETE CASCADE,
    repo_description      text,
    topics                text[] NOT NULL DEFAULT '{}',
    readme                text,
    release_name          text,
    release_notes         text,
    content_hash          text NOT NULL,
    fetched_at            timestamptz NOT NULL DEFAULT now(),
    describe_attempted_at timestamptz,
    describe_error        text
);

CREATE TABLE port_descriptions (
    port_id         bigint PRIMARY KEY REFERENCES ports (id) ON DELETE CASCADE,
    summary         text NOT NULL,
    technique       text NOT NULL CHECK (technique IN (
                        'static_recompilation', 'decompilation_port', 'source_port',
                        'engine_reimplementation', 'emulator_wrapper', 'other')),
    technique_note  text NOT NULL,
    features        text[] NOT NULL DEFAULT '{}',
    requirements    text[] NOT NULL DEFAULT '{}',
    limitations     text[] NOT NULL DEFAULT '{}',
    setup           text[] NOT NULL DEFAULT '{}',
    model           text NOT NULL,
    source_hash     text NOT NULL,
    generated_at    timestamptz NOT NULL DEFAULT now()
);
