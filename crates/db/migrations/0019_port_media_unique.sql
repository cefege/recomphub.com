-- README-derived screenshots are attached directly as approved rows by the GitHub scan
-- (recomphub_db::port_media::record_readme_images), distinguished from member uploads by
-- `source`. `position` orders README rows by their place in the README; it is NULL for uploads,
-- which keep ordering by `created_at`.
--
-- `media::enqueue` dedupes a URL globally, so the same uploaded file (same bytes, same
-- `upload:{hash}` source_url) submitted twice to the same port already produces two
-- `port_media` rows pointing at the same media row. Deduplicate before adding the uniqueness
-- constraint below, keeping one row per (port_id, media_id): prefer an approved row over a
-- pending or rejected one, then the oldest row.
DELETE FROM port_media
WHERE id IN (
    SELECT id FROM (
        SELECT id,
               row_number() OVER (
                   PARTITION BY port_id, media_id
                   ORDER BY (status = 'approved') DESC, (status = 'rejected') ASC, id
               ) AS rn
        FROM port_media
    ) ranked
    WHERE rn > 1
);

ALTER TABLE port_media ADD COLUMN source text NOT NULL DEFAULT 'upload'
    CHECK (source IN ('upload', 'readme'));
ALTER TABLE port_media ADD COLUMN position int;

ALTER TABLE port_media ADD CONSTRAINT port_media_port_id_media_id_key UNIQUE (port_id, media_id);
