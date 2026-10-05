BEGIN;

-- Historical event payloads do not contain the complete original requests.
-- Keep their receipts intact and explicitly unverifiable; never guess a digest.
ALTER TABLE awr_team.responsibility_receipts ADD COLUMN request_hash TEXT
    CHECK (request_hash IS NULL OR request_hash ~ '^[0-9a-f]{64}$');

UPDATE awr_team.schema_state SET version=40 WHERE component='awr_team';
COMMIT;
