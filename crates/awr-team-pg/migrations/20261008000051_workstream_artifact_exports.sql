BEGIN;

-- Ordinary authenticated disclosure is separate from legacy adoption credentials.
-- Immutable manifests/proofs survive revocation; they never grant execution.
CREATE TABLE awr_team.workstream_artifact_exports (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    provider_work_id TEXT NOT NULL,
    consumer_work_id TEXT NOT NULL,
    receipt_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    disclosure_sha256 TEXT NOT NULL CHECK (disclosure_sha256 ~ '^[0-9a-f]{64}$'),
    manifest_json JSONB NOT NULL CHECK (jsonb_typeof(manifest_json)='object'),
    proof_json JSONB NOT NULL CHECK (jsonb_typeof(proof_json)='object'),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','revoked')),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    published_by_actor_id TEXT NOT NULL,
    published_by_client_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    revoked_by_actor_id TEXT,
    revoked_by_client_id TEXT,
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id) REFERENCES awr_team.projects(tenant_id,id),
    FOREIGN KEY (tenant_id,project_id,receipt_id) REFERENCES awr_team.completion_receipts(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,artifact_id) REFERENCES awr_team.artifacts(tenant_id,project_id,id),
    CHECK (provider_work_id <> consumer_work_id),
    CHECK ((status='active' AND revoked_at IS NULL AND revoked_by_actor_id IS NULL AND revoked_by_client_id IS NULL)
        OR (status='revoked' AND revoked_at IS NOT NULL AND revoked_by_actor_id IS NOT NULL AND revoked_by_client_id IS NOT NULL))
);
CREATE UNIQUE INDEX workstream_artifact_exports_active ON awr_team.workstream_artifact_exports(tenant_id,project_id,disclosure_sha256) WHERE status='active';
CREATE INDEX workstream_artifact_exports_consumer ON awr_team.workstream_artifact_exports(tenant_id,project_id,consumer_work_id,id);

ALTER TABLE awr_team.workstream_artifact_exports ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.workstream_artifact_exports FORCE ROW LEVEL SECURITY;
CREATE POLICY workstream_artifact_exports_isolation ON awr_team.workstream_artifact_exports
    USING (tenant_id=current_setting('awr.tenant_id',true) AND project_id=current_setting('awr.project_id',true))
    WITH CHECK (tenant_id=current_setting('awr.tenant_id',true) AND project_id=current_setting('awr.project_id',true));

UPDATE awr_team.schema_state SET version=51 WHERE component='awr_team';
COMMIT;
