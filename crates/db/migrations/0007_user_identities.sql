-- Sign-in identities: a user can hold one identity per provider, keyed by the
-- provider's stable subject (GitHub user id, Google `sub`), never by email.
ALTER TABLE users ADD COLUMN display_name text;
ALTER TABLE users ADD COLUMN github_login text;
UPDATE users
   SET display_name = COALESCE(NULLIF(btrim(name), ''), login),
       github_login = login;
ALTER TABLE users ALTER COLUMN display_name SET NOT NULL;

CREATE TABLE user_identities (
    provider   text NOT NULL CHECK (provider IN ('github', 'google')),
    subject    text NOT NULL,
    user_id    bigint NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    login      text,
    email      text,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, subject),
    UNIQUE (user_id, provider)
);

INSERT INTO user_identities (provider, subject, user_id, login, created_at)
SELECT 'github', github_id::text, id, login, created_at FROM users;

-- The GitHub-only columns are superseded by user_identities and display_name. They stay,
-- nullable and unused, so the previous release can still read users during a rollback;
-- a later migration drops them.
ALTER TABLE users ALTER COLUMN github_id DROP NOT NULL;
ALTER TABLE users ALTER COLUMN login DROP NOT NULL;

CREATE INDEX users_github_login_idx ON users (lower(github_login)) WHERE github_login IS NOT NULL;
