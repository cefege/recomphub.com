CREATE TABLE desktop_login_requests (
    request_hash bytea PRIMARY KEY,
    challenge_hash bytea NOT NULL,
    user_id bigint REFERENCES users(id) ON DELETE CASCADE,
    csrf_hash bytea,
    browser_session_hash bytea,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz
);

CREATE INDEX desktop_login_requests_expiry_idx ON desktop_login_requests(expires_at);
