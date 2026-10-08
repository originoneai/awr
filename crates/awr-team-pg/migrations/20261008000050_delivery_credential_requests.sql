BEGIN;

-- Preserve legacy receipts without guessing their original request or outcome.
-- New outcomes recover a past result; they never grant current execution rights.
ALTER TABLE awr_team.delivery_credential_receipts
    ADD COLUMN request_hash TEXT,
    ADD COLUMN result_json JSONB,
    ADD CONSTRAINT delivery_credential_request_binding CHECK (
        (request_hash IS NULL AND result_json IS NULL)
        OR (
            request_hash IS NOT NULL AND result_json IS NOT NULL
            AND request_hash ~ '^[0-9a-f]{64}$'
            AND jsonb_typeof(result_json) = 'object'
            AND result_json ? 'id'
            AND jsonb_typeof(result_json->'id') = 'string'
            AND result_json->>'id' = subject_id
        )
    );

UPDATE awr_team.schema_state SET version=50 WHERE component='awr_team';

COMMIT;
