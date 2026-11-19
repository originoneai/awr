BEGIN;

-- Project observation settings are structured data. A missing time_zone means UTC.
ALTER TABLE awr_team.projects
    ADD COLUMN settings_json JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(settings_json) = 'object');

-- PPD-010 rows retain a null snapshot. New rows carry their facts and resolved
-- window in the same immutable history row, so later task changes cannot rewrite them.
ALTER TABLE awr_team.project_period_updates
    ADD COLUMN fact_snapshot_json JSONB,
    ADD COLUMN fact_window_start_unix_ms BIGINT,
    ADD COLUMN fact_window_end_unix_ms BIGINT,
    ADD COLUMN fact_time_zone TEXT,
    ADD CONSTRAINT project_period_update_fact_snapshot_complete CHECK (
        (fact_snapshot_ref IS NULL
            AND fact_snapshot_json IS NULL
            AND fact_window_start_unix_ms IS NULL
            AND fact_window_end_unix_ms IS NULL
            AND fact_time_zone IS NULL)
        OR
        (fact_snapshot_ref IS NOT NULL
            AND jsonb_typeof(fact_snapshot_json) = 'object'
            AND fact_window_start_unix_ms IS NOT NULL
            AND fact_window_end_unix_ms > fact_window_start_unix_ms
            AND fact_time_zone IS NOT NULL
            AND char_length(fact_time_zone) BETWEEN 1 AND 128)
    );

UPDATE awr_team.schema_state SET version=65 WHERE component='awr_team';
COMMIT;
