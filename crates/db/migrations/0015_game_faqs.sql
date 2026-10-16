-- Generated FAQ per game, written by describe-games from grounded facts. A game with no row
-- falls back to the deterministic FAQ on the page.
CREATE TABLE game_faqs (
    game_id     bigint PRIMARY KEY REFERENCES games (id) ON DELETE CASCADE,
    items       jsonb NOT NULL CHECK (jsonb_typeof(items) = 'array'),
    source_hash text NOT NULL,
    provider    text NOT NULL,
    model       text NOT NULL,
    generated_at timestamptz NOT NULL DEFAULT now()
);

-- The last failed attempt per game. A game is not retried for the same facts until the
-- retry window passes, so a few failing games cannot use up every run.
CREATE TABLE game_faq_attempts (
    game_id      bigint PRIMARY KEY REFERENCES games (id) ON DELETE CASCADE,
    source_hash  text NOT NULL,
    attempted_at timestamptz NOT NULL DEFAULT now(),
    error        text NOT NULL
);
