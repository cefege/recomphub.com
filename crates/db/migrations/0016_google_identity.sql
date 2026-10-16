-- Members can sign in with Google as well as GitHub. Google identities are keyed by the
-- OpenID `sub`; accounts are never merged by email.
ALTER TABLE user_identities DROP CONSTRAINT user_identities_provider_check;
ALTER TABLE user_identities
    ADD CONSTRAINT user_identities_provider_check
    CHECK (provider IN ('google', 'github', 'cloudflare'));
