BEGIN;

ALTER TABLE awr_team.ops_audit_records DROP CONSTRAINT ops_audit_records_target_kind_check;
ALTER TABLE awr_team.ops_audit_records ADD CONSTRAINT ops_audit_records_target_kind_check
    CHECK (target_kind IN ('work', 'change', 'member', 'request', 'candidate', 'delivery',
        'access_plan', 'suggestion', 'review', 'completion', 'directory'));

CREATE TABLE awr_team.project_directories (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version > 0),
    plan_json JSONB NOT NULL CHECK (jsonb_typeof(plan_json) = 'object'),
    provenance JSONB NOT NULL CHECK (jsonb_typeof(provenance) = 'object'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

CREATE TABLE awr_team.project_directory_changes (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    result_json JSONB NOT NULL CHECK (jsonb_typeof(result_json) = 'object'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, actor_id, client_id, request_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

CREATE FUNCTION awr_team.keep_directory_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'project directory change history is retained';
END;
$$;
CREATE TRIGGER keep_directory_history BEFORE UPDATE OR DELETE ON awr_team.project_directory_changes
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_directory_history();

UPDATE awr_team.schema_state SET version=62 WHERE component='awr_team';
COMMIT;
