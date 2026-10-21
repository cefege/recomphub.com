-- Background media processing is durable across worker restarts and bounded source failures.
ALTER TABLE media
    ADD COLUMN next_attempt_at timestamptz;
