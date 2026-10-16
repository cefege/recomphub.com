-- Donation/funding links for GitHub authors (keyed by lowercase login). Filled by github-sync
-- from FUNDING.yml in the author's port repos or profile repo, from the profile (blog, social
-- accounts, bio) and from GitHub Sponsors. `author_funding_checks` records every login looked
-- up, so logins without links are not re-fetched on every run.

CREATE TABLE author_funding_checks (
    login      text PRIMARY KEY CHECK (login = lower(login)),
    checked_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE author_funding (
    login    text NOT NULL REFERENCES author_funding_checks (login) ON DELETE CASCADE
             CHECK (login = lower(login)),
    platform text NOT NULL CHECK (platform IN (
        'github', 'patreon', 'open_collective', 'ko_fi', 'tidelift', 'community_bridge',
        'liberapay', 'issuehunt', 'polar', 'buy_me_a_coffee', 'thanks_dev', 'paypal', 'boosty',
        'fanbox', 'custom'
    )),
    url      text NOT NULL CHECK (url ~* '^https?://' AND length(url) <= 500),
    source   text NOT NULL CHECK (source IN ('repo', 'profile', 'sponsors')),
    PRIMARY KEY (login, platform, url)
);
