-- Private, account-scoped catalog game bookmarks. These do not affect public game metrics.
CREATE TABLE game_favorites (
    user_id    bigint NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    game_id    bigint NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, game_id)
);

CREATE INDEX game_favorites_game_id_idx ON game_favorites (game_id);
