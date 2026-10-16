-- Best-selling list entries from Wikipedia, one row per (list, game, sales year).
CREATE TABLE sales_signals (
    id                    bigserial PRIMARY KEY,
    game_id               bigint NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    list_title            text NOT NULL,
    scope                 text NOT NULL CHECK (scope IN ('platform', 'overall', 'year')),
    platform_slug         text,
    sales_year            int,
    rank                  int,
    units_sold_millions   double precision NOT NULL,
    listed_title          text NOT NULL,
    source_url            text NOT NULL,
    fetched_at            timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX sales_signals_list_game_year_idx
    ON sales_signals (list_title, game_id, COALESCE(sales_year, 0));
CREATE INDEX sales_signals_game_idx ON sales_signals (game_id);

-- Derived from the best of each game's sales_signals rows; rewritten by the sales sync.
ALTER TABLE games ADD COLUMN sales_score int NOT NULL DEFAULT 0;

-- Ordering key everywhere games are ranked by importance. popularity is overwritten
-- by every IGDB sync, so the sales signal lives beside it rather than inside it.
ALTER TABLE games ADD COLUMN importance int
    GENERATED ALWAYS AS (popularity + sales_score) STORED;

CREATE INDEX games_importance_idx ON games (importance DESC, id);
