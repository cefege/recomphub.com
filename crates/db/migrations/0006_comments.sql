-- Port comments and star ratings. A user may post many comments on a port but holds
-- at most one rating per port; the rating is independent of any comment.

CREATE TABLE comments (
    id            bigserial PRIMARY KEY,
    port_id       bigint NOT NULL REFERENCES ports (id) ON DELETE CASCADE,
    user_id       bigint NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    body          text NOT NULL CHECK (char_length(body) BETWEEN 1 AND 2000),
    tested_target text CHECK (tested_target IN (
        'windows', 'macos', 'linux', 'web', 'android', 'ios',
        'switch', 'ps_vita', 'psp', 'n3ds', 'wii_u'
    )),
    created_at    timestamptz NOT NULL DEFAULT now(),
    edited_at     timestamptz,
    hidden        boolean NOT NULL DEFAULT false,
    hidden_by     bigint REFERENCES users (id) ON DELETE SET NULL,
    hidden_reason text CHECK (char_length(hidden_reason) <= 500)
);

CREATE INDEX comments_port_created_idx ON comments (port_id, created_at);
CREATE INDEX comments_user_created_idx ON comments (user_id, created_at);

CREATE TABLE port_ratings (
    port_id    bigint NOT NULL REFERENCES ports (id) ON DELETE CASCADE,
    user_id    bigint NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    stars      smallint NOT NULL CHECK (stars BETWEEN 1 AND 5),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (port_id, user_id)
);

CREATE INDEX port_ratings_user_idx ON port_ratings (user_id);
