CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE platforms (
    id           serial PRIMARY KEY,
    igdb_id      int NOT NULL UNIQUE,
    slug         text UNIQUE,
    name         text NOT NULL,
    abbreviation text,
    family       text,
    generation   int
);

CREATE TABLE media (
    id         bigserial PRIMARY KEY,
    kind       text NOT NULL CHECK (kind IN ('cover', 'screenshot', 'avatar', 'video_thumb', 'og')),
    source_url text NOT NULL,
    hash       text NOT NULL,
    widths     int[] NOT NULL DEFAULT '{}',
    width      int,
    height     int,
    status     text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'ready', 'failed')),
    attempts   int NOT NULL DEFAULT 0,
    error      text,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (source_url, kind)
);

CREATE TABLE users (
    id              bigserial PRIMARY KEY,
    github_id       bigint NOT NULL UNIQUE,
    login           text NOT NULL,
    name            text,
    avatar_media_id bigint REFERENCES media (id) ON DELETE SET NULL,
    role            text NOT NULL DEFAULT 'user' CHECK (role IN ('user', 'moderator', 'admin')),
    approved_count  int NOT NULL DEFAULT 0,
    banned          boolean NOT NULL DEFAULT false,
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE games (
    id                bigserial PRIMARY KEY,
    igdb_id           bigint NOT NULL UNIQUE,
    slug              text NOT NULL UNIQUE,
    name              text NOT NULL,
    alt_names         text[] NOT NULL DEFAULT '{}',
    summary           text,
    first_release_date date,
    developer         text,
    publisher         text,
    genres            text[] NOT NULL DEFAULT '{}',
    popularity        int NOT NULL DEFAULT 0,
    cover_media_id    bigint REFERENCES media (id) ON DELETE SET NULL,
    og_media_id       bigint REFERENCES media (id) ON DELETE SET NULL,
    igdb_synced_at    timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    port_count        int NOT NULL DEFAULT 0,
    decomp_count      int NOT NULL DEFAULT 0,
    targets           text[] NOT NULL DEFAULT '{}',
    best_playability  text,
    want_count        int NOT NULL DEFAULT 0,
    search            tsvector
);
CREATE INDEX games_search_idx ON games USING gin (search);
CREATE INDEX games_name_trgm_idx ON games USING gin (name gin_trgm_ops);

CREATE TABLE game_platforms (
    game_id     bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    platform_id int NOT NULL REFERENCES platforms (id) ON DELETE CASCADE,
    PRIMARY KEY (game_id, platform_id)
);

CREATE TABLE game_media (
    game_id  bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    media_id bigint NOT NULL REFERENCES media (id) ON DELETE CASCADE,
    position int NOT NULL,
    PRIMARY KEY (game_id, media_id, position)
);

CREATE TABLE ports (
    id                    bigserial PRIMARY KEY,
    game_id               bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    slug                  text NOT NULL,
    name                  text NOT NULL,
    kind                  text NOT NULL CHECK (kind IN ('static_recomp', 'decomp_port', 'source_port', 'reimplementation', 'fan_remake', 'native_layer', 'decompilation')),
    source_platform_id    int REFERENCES platforms (id) ON DELETE SET NULL,
    repo_url              text NOT NULL UNIQUE,
    github_repo_id        bigint UNIQUE,
    homepage_url          text,
    description           text,
    import_description    text,
    requirements          text,
    toolchain             text,
    license               text,
    playability           text NOT NULL DEFAULT 'unknown' CHECK (playability IN ('complete', 'playable', 'in_progress', 'unknown')),
    latest_version        text,
    latest_release_url    text,
    latest_release_at     timestamptz,
    latest_is_prerelease  boolean,
    pushed_at             timestamptz,
    archived              boolean NOT NULL DEFAULT false,
    github_stars          int NOT NULL DEFAULT 0,
    github_forks          int NOT NULL DEFAULT 0,
    declared_platforms    text[] NOT NULL DEFAULT '{}',
    detected_platforms    text[] NOT NULL DEFAULT '{}',
    ai_usage              text NOT NULL DEFAULT 'unknown' CHECK (ai_usage IN ('none', 'assisted', 'primarily_ai', 'unknown')),
    ai_tools              text[] NOT NULL DEFAULT '{}',
    ai_source             text CHECK (ai_source IS NULL OR ai_source IN ('declared', 'detected', 'imported')),
    ai_evidence           jsonb NOT NULL DEFAULT '[]',
    star_count            int NOT NULL DEFAULT 0,
    status                text NOT NULL DEFAULT 'published' CHECK (status IN ('published', 'hidden')),
    sources               text[] NOT NULL DEFAULT '{}',
    locked_fields         text[] NOT NULL DEFAULT '{}',
    github_synced_at      timestamptz,
    ai_scanned_at         timestamptz,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    UNIQUE (game_id, slug)
);
CREATE INDEX ports_status_idx ON ports (status, game_id);

CREATE FUNCTION port_platforms(p ports) RETURNS text[]
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE WHEN cardinality(p.declared_platforms) > 0
                THEN p.declared_platforms
                ELSE p.detected_platforms END
$$;

CREATE TABLE port_authors (
    port_id         bigint NOT NULL REFERENCES ports (id) ON DELETE CASCADE,
    position        int NOT NULL,
    github_login    text,
    name            text NOT NULL,
    avatar_media_id bigint REFERENCES media (id) ON DELETE SET NULL,
    role            text NOT NULL CHECK (role IN ('owner', 'contributor', 'credited')),
    commits         int,
    PRIMARY KEY (port_id, position)
);
CREATE INDEX port_authors_login_idx ON port_authors (lower(github_login));

CREATE TABLE sessions (
    id_hash    bytea PRIMARY KEY,
    user_id    bigint NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE port_stars (
    user_id    bigint NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    port_id    bigint NOT NULL REFERENCES ports (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, port_id)
);

CREATE TABLE game_wants (
    user_id    bigint NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    game_id    bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, game_id)
);

CREATE TABLE submissions (
    id              bigserial PRIMARY KEY,
    kind            text NOT NULL CHECK (kind IN ('new_port', 'edit_port', 'new_game', 'report', 'candidate')),
    status          text NOT NULL CHECK (status IN ('pending', 'approved', 'auto_approved', 'rejected', 'reverted')),
    submitter_id    bigint REFERENCES users (id) ON DELETE SET NULL,
    source          text NOT NULL CHECK (source IN ('user', 'discovery:github', 'discovery:reddit', 'import:recompendium', 'import:portsdr')),
    payload         jsonb NOT NULL,
    before          jsonb,
    target_port_id  bigint REFERENCES ports (id) ON DELETE SET NULL,
    target_game_id  bigint REFERENCES games (id) ON DELETE SET NULL,
    reviewer_id     bigint REFERENCES users (id) ON DELETE SET NULL,
    review_note     text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    reviewed_at     timestamptz
);
CREATE INDEX submissions_status_idx ON submissions (status, created_at);
CREATE UNIQUE INDEX submissions_candidate_pending_idx
    ON submissions ((payload->>'repo_url'))
    WHERE kind = 'candidate' AND status = 'pending';

CREATE TABLE videos (
    id            bigserial PRIMARY KEY,
    game_id       bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    port_id       bigint REFERENCES ports (id) ON DELETE SET NULL,
    target        text NOT NULL CHECK (target IN ('windows', 'macos', 'linux', 'web', 'android', 'ios', 'switch', 'ps_vita', 'psp', 'n3ds', 'wii_u', 'any')),
    youtube_id    text NOT NULL,
    title         text NOT NULL,
    channel       text,
    published_at  timestamptz,
    thumb_media_id bigint REFERENCES media (id) ON DELETE SET NULL,
    hidden        boolean NOT NULL DEFAULT false,
    fetched_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (game_id, target, youtube_id)
);

CREATE TABLE youtube_queries (
    game_id     bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    target      text NOT NULL,
    last_run_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (game_id, target)
);

CREATE TABLE game_hits_daily (
    game_id bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    day     date NOT NULL,
    hits    int NOT NULL DEFAULT 0,
    PRIMARY KEY (game_id, day)
);

CREATE TABLE hit_dedupe (
    day          date NOT NULL,
    visitor_hash bytea NOT NULL,
    game_id      bigint NOT NULL,
    PRIMARY KEY (day, visitor_hash, game_id)
);

CREATE TABLE daily_salts (
    day  date PRIMARY KEY,
    salt bytea NOT NULL
);

CREATE TABLE game_live_daily (
    game_id        bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    day            date NOT NULL,
    viewer_minutes int NOT NULL DEFAULT 0,
    peak_viewers   int NOT NULL DEFAULT 0,
    PRIMARY KEY (game_id, day)
);

CREATE TABLE site_live_daily (
    day         date PRIMARY KEY,
    peak_online int NOT NULL DEFAULT 0
);

CREATE TABLE activity (
    id         bigserial PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now(),
    kind       text NOT NULL CHECK (kind IN ('port_added', 'release', 'starred', 'wanted', 'game_added')),
    game_id    bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    port_id    bigint REFERENCES ports (id) ON DELETE CASCADE,
    data       jsonb NOT NULL DEFAULT '{}',
    actor_hash bytea
);
CREATE UNIQUE INDEX activity_actor_dedupe_idx
    ON activity (kind, game_id, coalesce(port_id, 0), actor_hash)
    WHERE actor_hash IS NOT NULL;

CREATE TABLE settings (
    key   text PRIMARY KEY,
    value jsonb NOT NULL
);

INSERT INTO settings (key, value) VALUES
    ('catalog_target_size', '5000'),
    ('trust_threshold', '3'),
    ('youtube_daily_searches', '90'),
    ('submission_daily_limit', '30'),
    ('live_minutes_per_point', '5');

CREATE FUNCTION refresh_game_rollup(gid bigint) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE games g SET
        port_count = (
            SELECT count(*) FROM ports p
            WHERE p.game_id = gid AND p.status = 'published' AND p.kind <> 'decompilation'
        ),
        decomp_count = (
            SELECT count(*) FROM ports p
            WHERE p.game_id = gid AND p.status = 'published' AND p.kind = 'decompilation'
        ),
        targets = COALESCE((
            SELECT array_agg(DISTINCT u.t ORDER BY u.t)
            FROM ports p, LATERAL unnest(port_platforms(p)) AS u(t)
            WHERE p.game_id = gid AND p.status = 'published'
        ), '{}'),
        best_playability = (
            SELECT p.playability FROM ports p
            WHERE p.game_id = gid AND p.status = 'published' AND p.kind <> 'decompilation'
            ORDER BY CASE p.playability
                WHEN 'complete' THEN 0 WHEN 'playable' THEN 1
                WHEN 'in_progress' THEN 2 ELSE 3 END
            LIMIT 1
        ),
        want_count = (SELECT count(*) FROM game_wants w WHERE w.game_id = gid)
    WHERE g.id = gid;
END;
$$;

CREATE MATERIALIZED VIEW game_score_ranking AS
WITH params AS (
    SELECT COALESCE(
        (SELECT (s.value #>> '{}')::numeric FROM settings s WHERE s.key = 'live_minutes_per_point'),
        5) AS per
),
windows AS (
    SELECT w AS window_days FROM unnest(ARRAY[1, 7, 30, 365]) AS w
),
cur AS (
    SELECT win.window_days, x.game_id, SUM(x.hits) AS hits, SUM(x.vm) AS vm
    FROM windows win
    CROSS JOIN LATERAL (
        SELECT h.game_id, h.hits::bigint AS hits, 0::bigint AS vm
        FROM game_hits_daily h
        WHERE h.day > current_date - win.window_days
        UNION ALL
        SELECT l.game_id, 0::bigint, l.viewer_minutes::bigint
        FROM game_live_daily l
        WHERE l.day > current_date - win.window_days
    ) x
    GROUP BY win.window_days, x.game_id
),
prev AS (
    SELECT win.window_days, x.game_id, SUM(x.hits) AS hits, SUM(x.vm) AS vm
    FROM windows win
    CROSS JOIN LATERAL (
        SELECT h.game_id, h.hits::bigint AS hits, 0::bigint AS vm
        FROM game_hits_daily h
        WHERE h.day > current_date - win.window_days - 1 AND h.day < current_date
        UNION ALL
        SELECT l.game_id, 0::bigint, l.viewer_minutes::bigint
        FROM game_live_daily l
        WHERE l.day > current_date - win.window_days - 1 AND l.day < current_date
    ) x
    GROUP BY win.window_days, x.game_id
),
scored AS (
    SELECT c.window_days, c.game_id, c.hits, c.vm,
           (c.hits + c.vm / p.per) AS score,
           rank() OVER (PARTITION BY c.window_days ORDER BY (c.hits + c.vm / p.per) DESC) AS rnk
    FROM cur c CROSS JOIN params p
),
prev_scored AS (
    SELECT c.window_days, c.game_id,
           rank() OVER (PARTITION BY c.window_days ORDER BY (c.hits + c.vm / p.per) DESC) AS prnk
    FROM prev c CROSS JOIN params p
)
SELECT s.window_days::int AS window_days,
       s.game_id,
       s.hits::int AS hits,
       s.vm::int AS viewer_minutes,
       s.score::numeric(12, 1) AS score,
       s.rnk::int AS rank,
       ps.prnk::int AS prev_rank
FROM scored s
LEFT JOIN prev_scored ps ON ps.window_days = s.window_days AND ps.game_id = s.game_id
WITH DATA;

CREATE UNIQUE INDEX game_score_ranking_key ON game_score_ranking (window_days, game_id);
CREATE INDEX game_score_ranking_rank ON game_score_ranking (window_days, rank);
