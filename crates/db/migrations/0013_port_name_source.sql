-- Where a port's display name came from. 'repo': the name still equals the repo slug, so
-- github-sync may replace it with a name derived from the README. 'derived': github-sync set it.
-- 'manual': anything else, never overwritten.
ALTER TABLE ports ADD COLUMN name_source text NOT NULL DEFAULT 'manual'
    CHECK (name_source IN ('repo', 'derived', 'manual'));

UPDATE ports SET name_source = 'repo'
WHERE regexp_replace(lower(name), '[-_]', '', 'g')
    = regexp_replace(lower(split_part(repo_url, '/', 5)), '[-_]', '', 'g');

-- Target key a game's original platform already covers, keyed by IGDB slug. Mirrors
-- TargetPlatform::from_platform_slug in crates/shared/src/enums.rs; keep the two in step.
CREATE FUNCTION platform_target(slug text) RETURNS text
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE slug
        WHEN 'win' THEN 'windows'
        WHEN 'mac' THEN 'macos'
        WHEN 'linux' THEN 'linux'
        WHEN 'browser' THEN 'web'
        WHEN 'android' THEN 'android'
        WHEN 'iphone' THEN 'ios'
        WHEN 'ipad' THEN 'ios'
        WHEN 'switch' THEN 'switch'
        WHEN 'psp' THEN 'psp'
        WHEN '3ds' THEN 'n3ds'
        WHEN 'wiiu' THEN 'wii_u'
        WHEN 'psvita' THEN 'ps_vita'
    END
$$;
