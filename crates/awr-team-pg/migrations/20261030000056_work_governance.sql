BEGIN;

-- What a source snapshot declares about governance: its effective admission policy and, for every
-- governed work, the declared definition and the summary of its definition report. Written when a
-- snapshot is installed from its package. It describes governance and never decides anything.
CREATE TABLE awr_team.snapshot_governance (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    governance_hash TEXT NOT NULL CHECK (governance_hash ~ '^[0-9a-f]{64}$'),
    policy_json JSONB NOT NULL CHECK (jsonb_typeof(policy_json) = 'object'),
    PRIMARY KEY (tenant_id, project_id, snapshot_id),
    FOREIGN KEY (tenant_id, project_id, snapshot_id)
        REFERENCES awr_team.source_snapshots(tenant_id, project_id, id)
);

CREATE TABLE awr_team.work_governance (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    external_key TEXT NOT NULL,
    governance_json JSONB NOT NULL CHECK (jsonb_typeof(governance_json) = 'object'),
    PRIMARY KEY (tenant_id, project_id, snapshot_id, work_id),
    FOREIGN KEY (tenant_id, project_id, snapshot_id)
        REFERENCES awr_team.snapshot_governance(tenant_id, project_id, snapshot_id),
    FOREIGN KEY (tenant_id, project_id, work_id)
        REFERENCES awr_team.work_items(tenant_id, project_id, id)
);

-- Admission decisions, append-only. A decision binds the exact scope it judged and is attributed to
-- the verified actor (and person, when the actor is linked to one) that recorded it. A snapshot,
-- an import or a restore never writes here.
CREATE TABLE awr_team.work_admissions (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('proposed', 'approved', 'denied')),
    contract_hash TEXT NOT NULL CHECK (contract_hash ~ '^[0-9a-f]{64}$'),
    binding_json JSONB NOT NULL CHECK (jsonb_typeof(binding_json) = 'object'),
    reason TEXT,
    judgment_json JSONB,
    proposal_id TEXT,
    actor_id TEXT NOT NULL,
    person_id TEXT,
    project_revision BIGINT NOT NULL CHECK (project_revision >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    UNIQUE (tenant_id, project_id, request_id),
    FOREIGN KEY (tenant_id, project_id, work_id)
        REFERENCES awr_team.work_items(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, proposal_id)
        REFERENCES awr_team.work_admissions(tenant_id, project_id, id),
    CHECK ((decision = 'denied') = (reason IS NOT NULL)),
    CHECK (decision = 'denied' OR judgment_json IS NOT NULL OR proposal_id IS NOT NULL),
    CHECK (proposal_id IS NULL OR decision = 'approved')
);
CREATE INDEX work_admissions_by_work
    ON awr_team.work_admissions(tenant_id, project_id, work_id, created_at DESC, id DESC);

CREATE FUNCTION awr_team.keep_work_admissions() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'work admission decisions are append-only';
END;
$$;
CREATE TRIGGER keep_work_admissions BEFORE UPDATE OR DELETE ON awr_team.work_admissions
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_work_admissions();

-- Tenant/project isolation (FORCE so NOSUPERUSER NOBYPASSRLS app cannot bypass).
DO $$
DECLARE t TEXT;
BEGIN
    FOREACH t IN ARRAY ARRAY['snapshot_governance', 'work_governance', 'work_admissions']
    LOOP
        EXECUTE format('ALTER TABLE awr_team.%I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE awr_team.%I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I_isolation ON awr_team.%I
             USING (tenant_id = current_setting(''awr.tenant_id'', true)
                AND project_id = current_setting(''awr.project_id'', true))
             WITH CHECK (tenant_id = current_setting(''awr.tenant_id'', true)
                AND project_id = current_setting(''awr.project_id'', true))', t, t);
    END LOOP;
END $$;

UPDATE awr_team.schema_state SET version=56 WHERE component='awr_team';
COMMIT;
