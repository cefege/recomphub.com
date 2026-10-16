-- Maintenance signals per port, recomputed on each GitHub sync (see recomphub_shared::maintenance).
-- Counts are over the default branch. weekly_commits holds the last 12 weeks, oldest first.
ALTER TABLE ports
    ADD COLUMN last_commit_at          timestamptz,
    ADD COLUMN commits_30d             int NOT NULL DEFAULT 0,
    ADD COLUMN commits_90d             int NOT NULL DEFAULT 0,
    ADD COLUMN commits_365d            int NOT NULL DEFAULT 0,
    ADD COLUMN committers_180d         int NOT NULL DEFAULT 0,
    ADD COLUMN releases_365d           int NOT NULL DEFAULT 0,
    ADD COLUMN weekly_commits          int[] NOT NULL DEFAULT '{}',
    ADD COLUMN maintenance_level       text
        CHECK (maintenance_level IN ('active', 'maintained', 'slowing', 'dormant', 'archived')),
    ADD COLUMN maintenance_score       smallint CHECK (maintenance_score BETWEEN 0 AND 100),
    ADD COLUMN maintenance_computed_at timestamptz;
