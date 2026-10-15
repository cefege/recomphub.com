-- Reviewed seed list imported by `worker import-curated`.
ALTER TABLE submissions DROP CONSTRAINT submissions_source_check;
ALTER TABLE submissions ADD CONSTRAINT submissions_source_check CHECK (source IN (
    'user', 'discovery:github', 'discovery:reddit', 'import:recompendium', 'import:portsdr', 'import:curated'
));
