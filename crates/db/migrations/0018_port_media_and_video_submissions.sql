-- Member contributions to a port's own gallery: uploaded screenshots (always moderated, even
-- from a signed-in account) and submitted YouTube links (moderated the same way a hidden video
-- already works, just inverted: new rows start hidden from the public until approved).

ALTER TABLE media DROP CONSTRAINT media_kind_check;
ALTER TABLE media ADD CONSTRAINT media_kind_check
    CHECK (kind = ANY (ARRAY['cover', 'screenshot', 'avatar', 'video_thumb', 'og', 'port_screenshot']));

CREATE TABLE port_media (
    id bigserial PRIMARY KEY,
    port_id bigint NOT NULL REFERENCES ports(id) ON DELETE CASCADE,
    media_id bigint NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    uploaded_by bigint REFERENCES users(id) ON DELETE SET NULL,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected')),
    reviewed_by bigint REFERENCES users(id) ON DELETE SET NULL,
    reviewed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX port_media_port_status_idx ON port_media (port_id, status);
CREATE INDEX port_media_pending_idx ON port_media (created_at) WHERE status = 'pending';

ALTER TABLE videos ADD COLUMN pending boolean NOT NULL DEFAULT false;
CREATE INDEX videos_pending_idx ON videos (fetched_at) WHERE pending;
