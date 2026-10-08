BEGIN;

-- Exact adoption snapshots remain archived when a newer selection is made.
CREATE TABLE awr_team.workstream_artifact_adoptions (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    consumer_work_id TEXT NOT NULL,
    provider_work_id TEXT NOT NULL,
    export_id TEXT NOT NULL,
    export_version BIGINT NOT NULL CHECK (export_version > 0),
    disclosure_sha256 TEXT NOT NULL CHECK (disclosure_sha256 ~ '^[0-9a-f]{64}$'),
    consumer_contract_hash TEXT NOT NULL CHECK (consumer_contract_hash ~ '^[0-9a-f]{64}$'),
    consumer_ownership_version BIGINT NOT NULL CHECK (consumer_ownership_version > 0),
    version BIGINT NOT NULL CHECK (version > 0),
    selected BOOLEAN NOT NULL DEFAULT true,
    adopted_by_actor_id TEXT NOT NULL,
    adopted_by_client_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,export_id) REFERENCES awr_team.workstream_artifact_exports(tenant_id,project_id,id),
    CHECK (consumer_work_id <> provider_work_id),
    UNIQUE (tenant_id,project_id,consumer_work_id,provider_work_id,version)
);
CREATE UNIQUE INDEX workstream_artifact_adoptions_selected ON awr_team.workstream_artifact_adoptions(tenant_id,project_id,consumer_work_id,provider_work_id) WHERE selected;

ALTER TABLE awr_team.workstream_artifact_adoptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.workstream_artifact_adoptions FORCE ROW LEVEL SECURITY;
CREATE POLICY workstream_artifact_adoptions_isolation ON awr_team.workstream_artifact_adoptions
    USING (tenant_id=current_setting('awr.tenant_id',true) AND project_id=current_setting('awr.project_id',true))
    WITH CHECK (tenant_id=current_setting('awr.tenant_id',true) AND project_id=current_setting('awr.project_id',true));

UPDATE awr_team.schema_state SET version=52 WHERE component='awr_team';
COMMIT;
