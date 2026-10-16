-- YouTube links found in a port's README (filled by github-sync) and the
-- per-port yt-dlp search schedule (read by the port-videos job).

CREATE TABLE port_readme_videos (
    port_id     bigint NOT NULL REFERENCES ports (id) ON DELETE CASCADE,
    youtube_id  text NOT NULL CHECK (youtube_id ~ '^[A-Za-z0-9_-]{11}$'),
    found_at    timestamptz NOT NULL DEFAULT now(),
    resolved_at timestamptz,
    attempts    integer NOT NULL DEFAULT 0,
    last_error  text,
    PRIMARY KEY (port_id, youtube_id)
);

CREATE INDEX port_readme_videos_pending_idx
    ON port_readme_videos (found_at)
    WHERE resolved_at IS NULL;

CREATE TABLE port_video_searches (
    port_id     bigint PRIMARY KEY REFERENCES ports (id) ON DELETE CASCADE,
    last_run_at timestamptz NOT NULL DEFAULT now(),
    last_error  text
);
