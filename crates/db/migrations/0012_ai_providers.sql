-- Admin-managed, ordered AI providers for server jobs. Credentials are sealed
-- (AES-256-GCM, key from SECRETS_KEY) and never leave the server.
CREATE TABLE ai_providers (
    id bigserial PRIMARY KEY,
    position integer NOT NULL,
    kind text NOT NULL CHECK (kind IN ('anthropic_api', 'openai_api', 'anthropic_oauth', 'openai_codex_oauth')),
    model text NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    account_label text NOT NULL,
    sealed bytea NOT NULL,
    last_error text,
    last_error_at timestamptz,
    last_used_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT ai_providers_position_key UNIQUE (position) DEFERRABLE INITIALLY DEFERRED
);

-- In-flight OAuth logins. `sealed` holds the PKCE verifier / device code state.
CREATE TABLE ai_oauth_pending (
    id text PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('anthropic_oauth', 'openai_codex_oauth')),
    flow text NOT NULL CHECK (flow IN ('paste', 'device')),
    model text NOT NULL,
    sealed bytea NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Which provider kind produced each stored description.
ALTER TABLE port_descriptions ADD COLUMN provider text;
