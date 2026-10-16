-- Sign-in providers: GitHub for members, Cloudflare Access for the admin. Identities are
-- keyed by (provider, subject); no account is ever merged by email. Google identities are
-- not supported, so their rows are removed. Their user rows remain but cannot sign in.
DELETE FROM user_identities WHERE provider NOT IN ('github', 'cloudflare');
ALTER TABLE user_identities DROP CONSTRAINT user_identities_provider_check;
ALTER TABLE user_identities
    ADD CONSTRAINT user_identities_provider_check
    CHECK (provider IN ('github', 'cloudflare'));
