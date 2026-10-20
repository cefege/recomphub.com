ALTER TABLE submissions
    ADD COLUMN triage jsonb,
    ADD COLUMN triage_revision bigint NOT NULL DEFAULT 0,
    ADD COLUMN triage_attempted_at timestamptz,
    ADD COLUMN triage_error text;

CREATE INDEX submissions_pending_triage_due_idx
    ON submissions (triage_attempted_at NULLS FIRST, created_at, id)
    WHERE status = 'pending'
      AND (kind = 'candidate' OR (source = 'user' AND kind = 'new_port'));
