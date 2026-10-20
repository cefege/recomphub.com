-- Durable provider work and reviewable machine video candidates.
CREATE TABLE video_discovery_tasks (
    id uuid PRIMARY KEY,
    port_id bigint NOT NULL REFERENCES ports(id) ON DELETE CASCADE,
    game_id bigint NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    query text NOT NULL,
    provider_task_id text,
    state text NOT NULL CHECK (state IN ('submitted', 'collected', 'failed')),
    submitted_at timestamptz NOT NULL DEFAULT now(),
    collected_at timestamptz,
    last_error text,
    UNIQUE (port_id, query)
);
CREATE INDEX video_discovery_tasks_submitted_idx
    ON video_discovery_tasks (submitted_at)
    WHERE state = 'submitted';

CREATE TABLE video_discovery_candidates (
    id bigserial PRIMARY KEY,
    port_id bigint NOT NULL REFERENCES ports(id) ON DELETE CASCADE,
    game_id bigint NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    youtube_id text NOT NULL CHECK (youtube_id ~ '^[A-Za-z0-9_-]{11}$'),
    url text NOT NULL,
    title text NOT NULL,
    description text NOT NULL DEFAULT '',
    rank integer,
    thumbnail_media_id bigint REFERENCES media(id) ON DELETE SET NULL,
    source text NOT NULL CHECK (source IN ('google', 'readme')),
    decision text NOT NULL CHECK (decision IN ('published', 'pending', 'rejected')),
    decision_reason text,
    reviewer_id bigint REFERENCES users(id) ON DELETE SET NULL,
    reviewed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (port_id, youtube_id)
);
CREATE INDEX video_discovery_candidates_review_idx
    ON video_discovery_candidates (created_at)
    WHERE decision = 'pending';
CREATE INDEX video_discovery_candidates_port_recent_idx
    ON video_discovery_candidates (port_id, created_at DESC);

-- A video can now be associated with distinct ports of one game. Game-level legacy videos
-- retain their previous identity independently of port-attributed rows.
ALTER TABLE videos DROP CONSTRAINT videos_game_id_target_youtube_id_key;
CREATE UNIQUE INDEX videos_game_target_youtube_idx
    ON videos (game_id, target, youtube_id)
    WHERE port_id IS NULL;
CREATE UNIQUE INDEX videos_game_port_target_youtube_idx
    ON videos (game_id, port_id, target, youtube_id)
    WHERE port_id IS NOT NULL;

-- All published renditions use a validated semantic stem and explicit renderer version.
ALTER TABLE media
    ADD COLUMN render_version integer NOT NULL DEFAULT 1,
    ADD COLUMN public_stem text NOT NULL DEFAULT 'media';
UPDATE media
SET public_stem = lower(regexp_replace(kind || '-' || id::text, '[^a-z0-9]+', '-', 'g'));

-- Automated README images wait for render validation and moderator visibility.
ALTER TABLE port_media
    ADD COLUMN source_alt text,
    ADD COLUMN decision_reason text;

-- Replaced direct YouTube and yt-dlp schedules. Existing public videos remain available; only
-- obsolete discovery state is removed.
DROP TABLE port_readme_videos;
DROP TABLE port_video_searches;
DROP TABLE youtube_queries;
DELETE FROM settings WHERE key = 'youtube_daily_searches';
