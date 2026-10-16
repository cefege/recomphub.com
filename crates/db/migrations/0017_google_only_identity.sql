-- Google is the only sign-in method. GitHub and Cloudflare Access identities are removed;
-- their user rows stay (with their comments, ratings and submissions) but cannot sign in.
DELETE FROM user_identities WHERE provider <> 'google';
ALTER TABLE user_identities DROP CONSTRAINT user_identities_provider_check;
ALTER TABLE user_identities
    ADD CONSTRAINT user_identities_provider_check CHECK (provider = 'google');
