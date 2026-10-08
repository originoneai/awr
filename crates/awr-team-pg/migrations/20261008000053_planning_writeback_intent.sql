BEGIN;

-- Legacy journals retain unknown provenance. Never backfill original requests.
ALTER TABLE awr_team.planning_writeback_journals
    ADD COLUMN intent_hash TEXT,
    ADD COLUMN intent_json JSONB,
    ADD COLUMN dependency_work_ids JSONB NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE awr_team.planning_writeback_journals ADD CONSTRAINT planning_intent_pair
    CHECK ((intent_hash IS NULL) = (intent_json IS NULL));

CREATE INDEX planning_writeback_pending
    ON awr_team.planning_writeback_journals (tenant_id, project_id, request_id)
    WHERE phase IN ('validated', 'source_written', 'pg_activating');

UPDATE awr_team.schema_state SET version=53 WHERE component='awr_team';
COMMIT;
