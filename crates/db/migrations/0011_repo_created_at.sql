-- When each project's GitHub repository was created. Filled by github-sync; NULL until then.
ALTER TABLE ports ADD COLUMN repo_created_at timestamptz;
