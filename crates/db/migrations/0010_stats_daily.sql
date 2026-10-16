-- Daily platform coverage history for /stats. `source` records whether a row was observed
-- by the daily snapshot or reconstructed from creation dates by the backfill.
CREATE TABLE stats_daily (
    day         date NOT NULL,
    metric      text NOT NULL CHECK (metric IN ('ports', 'ported_games', 'catalog_games', 'target_ports', 'target_games')),
    key         text NOT NULL,
    value       bigint NOT NULL,
    source      text NOT NULL CHECK (source IN ('snapshot', 'backfill')),
    captured_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (day, metric, key)
);

CREATE INDEX stats_daily_metric_key_idx ON stats_daily (metric, key, day);
