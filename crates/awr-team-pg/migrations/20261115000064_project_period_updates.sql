BEGIN;

-- A period update is an attributed human judgment, not a generated progress sentence.
-- Versions are immutable; the separate current pointer retains every earlier version unchanged.
CREATE TABLE awr_team.project_period_updates (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    period_kind TEXT NOT NULL CHECK (period_kind IN ('week', 'month')),
    period_key TEXT NOT NULL CHECK (
        char_length(period_key) BETWEEN 1 AND 64
        AND period_key !~ '[[:cntrl:]]'
        AND ((period_kind = 'week' AND period_key ~ '^[0-9]{4}-W(0[1-9]|[1-4][0-9]|5[0-3])$')
            OR (period_kind = 'month' AND period_key ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'))
    ),
    starts_on DATE NOT NULL,
    ends_on DATE NOT NULL CHECK (ends_on >= starts_on),
    status TEXT NOT NULL CHECK (status IN ('ok', 'risk', 'off')),
    summary TEXT NOT NULL CHECK (
        char_length(summary) BETWEEN 1 AND 200
        AND char_length(btrim(summary)) > 0
        AND summary !~ '[[:cntrl:]]'
    ),
    author_person_id TEXT NOT NULL,
    author_display_name TEXT NOT NULL,
    recorded_by_actor_id TEXT NOT NULL,
    recorded_by_client_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version >= 1),
    supersedes_update_id TEXT,
    fact_snapshot_ref TEXT,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    UNIQUE (tenant_id, project_id, period_kind, period_key, version),
    UNIQUE (tenant_id, project_id, period_kind, period_key, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id),
    FOREIGN KEY (tenant_id, project_id, period_kind, period_key, supersedes_update_id)
        REFERENCES awr_team.project_period_updates(tenant_id, project_id, period_kind, period_key, id)
);

CREATE INDEX project_period_update_history
    ON awr_team.project_period_updates(tenant_id, project_id, period_kind, period_key, version DESC);

CREATE TABLE awr_team.project_period_update_current (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    period_kind TEXT NOT NULL,
    period_key TEXT NOT NULL,
    update_id TEXT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, period_kind, period_key),
    UNIQUE (tenant_id, project_id, update_id),
    FOREIGN KEY (tenant_id, project_id, period_kind, period_key, update_id)
        REFERENCES awr_team.project_period_updates(tenant_id, project_id, period_kind, period_key, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

ALTER TABLE awr_team.project_period_updates ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.project_period_updates FORCE ROW LEVEL SECURITY;
CREATE POLICY project_period_updates_isolation ON awr_team.project_period_updates
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

ALTER TABLE awr_team.project_period_update_current ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.project_period_update_current FORCE ROW LEVEL SECURITY;
CREATE POLICY project_period_update_current_isolation ON awr_team.project_period_update_current
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

CREATE FUNCTION awr_team.keep_project_period_update_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'project period update history is retained';
END;
$$;
CREATE TRIGGER keep_project_period_update_history
    BEFORE UPDATE OR DELETE ON awr_team.project_period_updates
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_project_period_update_history();

UPDATE awr_team.schema_state SET version=64 WHERE component='awr_team';
COMMIT;
