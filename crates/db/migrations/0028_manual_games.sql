-- Some officially released games are missing from IGDB. Moderators can add them from verified
-- sources; such games have no igdb_id and are never refreshed from IGDB.
ALTER TABLE games ALTER COLUMN igdb_id DROP NOT NULL;
ALTER TABLE games ADD COLUMN catalog_source_url text;
