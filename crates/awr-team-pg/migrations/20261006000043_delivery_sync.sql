BEGIN;

-- Provider-neutral observations are descriptions, never review or completion receipts.
CREATE TABLE awr_team.delivery_connectors (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    scope_id TEXT NOT NULL CHECK (scope_id='main'),
    workstream_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    resource TEXT NOT NULL,
    principal_actor_id TEXT NOT NULL,
    principal_client_id TEXT NOT NULL,
    fact_source TEXT NOT NULL CHECK (fact_source IN ('caller_declared','operator_recorded','adapter_observation')),
    version BIGINT NOT NULL CHECK (version>0),
    coordinator_epoch TEXT NOT NULL,
    enabled BOOLEAN NOT NULL,
    inspection_generation BIGINT NOT NULL DEFAULT 0 CHECK (inspection_generation>=0),
    configured_by_actor_id TEXT NOT NULL,
    PRIMARY KEY (tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,work_id) REFERENCES awr_team.work_items(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,principal_actor_id) REFERENCES awr_team.actors(tenant_id,id)
);

CREATE TABLE awr_team.delivery_candidates (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    binding_digest TEXT NOT NULL CHECK (binding_digest ~ '^[0-9a-f]{64}$'),
    work_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    candidate_version TEXT NOT NULL,
    body_json JSONB NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,binding_digest),
    UNIQUE (tenant_id,project_id,work_id,candidate_id,candidate_version),
    FOREIGN KEY (tenant_id,project_id,work_id) REFERENCES awr_team.work_items(tenant_id,project_id,id)
);

CREATE TABLE awr_team.delivery_selections (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    scope_id TEXT NOT NULL CHECK (scope_id='main'),
    work_id TEXT NOT NULL,
    binding_digest TEXT NOT NULL,
    selection_version BIGINT NOT NULL CHECK (selection_version>0),
    source_snapshot_id TEXT NOT NULL,
    ownership_version BIGINT NOT NULL CHECK (ownership_version>0),
    actor_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    claim_id TEXT NOT NULL,
    fence BIGINT NOT NULL CHECK (fence>0),
    PRIMARY KEY (tenant_id,project_id,scope_id,work_id),
    FOREIGN KEY (tenant_id,project_id,binding_digest) REFERENCES awr_team.delivery_candidates(tenant_id,project_id,binding_digest),
    FOREIGN KEY (tenant_id,project_id,claim_id) REFERENCES awr_team.claims(tenant_id,project_id,id)
);

CREATE TABLE awr_team.delivery_inspections (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    connector_version BIGINT NOT NULL CHECK (connector_version>0),
    generation BIGINT NOT NULL CHECK (generation>0),
    binding_digest TEXT NOT NULL,
    selection_version BIGINT NOT NULL CHECK (selection_version>0),
    source_snapshot_id TEXT NOT NULL,
    ownership_version BIGINT NOT NULL CHECK (ownership_version>0),
    coordinator_epoch TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    authority_binding TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    UNIQUE (tenant_id,project_id,connector_id,generation),
    FOREIGN KEY (tenant_id,project_id,connector_id) REFERENCES awr_team.delivery_connectors(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,binding_digest) REFERENCES awr_team.delivery_candidates(tenant_id,project_id,binding_digest)
);

CREATE TABLE awr_team.delivery_inbox (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    inspection_id TEXT NOT NULL,
    input_digest TEXT NOT NULL CHECK (input_digest ~ '^[0-9a-f]{64}$'),
    state TEXT NOT NULL CHECK (state IN ('applied','superseded')),
    receipt_json JSONB NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    UNIQUE (tenant_id,project_id,connector_id,event_id),
    FOREIGN KEY (tenant_id,project_id,inspection_id) REFERENCES awr_team.delivery_inspections(tenant_id,project_id,id)
);

CREATE TABLE awr_team.delivery_facts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    inbox_id TEXT NOT NULL,
    slot TEXT NOT NULL,
    envelope_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id,project_id,id),
    UNIQUE (tenant_id,project_id,inbox_id,slot),
    FOREIGN KEY (tenant_id,project_id,inbox_id) REFERENCES awr_team.delivery_inbox(tenant_id,project_id,id)
);

CREATE TABLE awr_team.delivery_fact_heads (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    slot TEXT NOT NULL,
    generation BIGINT NOT NULL CHECK (generation>0),
    fact_id TEXT NOT NULL,
    PRIMARY KEY (tenant_id,project_id,connector_id,slot),
    FOREIGN KEY (tenant_id,project_id,fact_id) REFERENCES awr_team.delivery_facts(tenant_id,project_id,id)
);

-- Separate from execution dispatch: these intents advertise facts only.
CREATE TABLE awr_team.delivery_notifications (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    inbox_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','delivered','superseded','failed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    UNIQUE (tenant_id,project_id,inbox_id),
    FOREIGN KEY (tenant_id,project_id,inbox_id) REFERENCES awr_team.delivery_inbox(tenant_id,project_id,id)
);

CREATE TABLE awr_team.delivery_sync_requests (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    result_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,actor_id,client_id,request_id)
);

CREATE INDEX delivery_inbox_inspection ON awr_team.delivery_inbox(tenant_id,project_id,inspection_id);
CREATE INDEX delivery_notifications_pending ON awr_team.delivery_notifications(tenant_id,project_id,state,created_at,id);

DO $$ DECLARE name TEXT; BEGIN
    FOREACH name IN ARRAY ARRAY['delivery_connectors','delivery_candidates','delivery_selections',
        'delivery_inspections','delivery_inbox','delivery_facts','delivery_fact_heads',
        'delivery_notifications','delivery_sync_requests'] LOOP
        EXECUTE format('ALTER TABLE awr_team.%I ENABLE ROW LEVEL SECURITY',name);
        EXECUTE format('ALTER TABLE awr_team.%I FORCE ROW LEVEL SECURITY',name);
        EXECUTE format('CREATE POLICY delivery_isolation ON awr_team.%I USING
            (tenant_id=current_setting(''awr.tenant_id'',true) AND project_id=current_setting(''awr.project_id'',true))
            WITH CHECK (tenant_id=current_setting(''awr.tenant_id'',true) AND project_id=current_setting(''awr.project_id'',true))',name);
    END LOOP;
END $$;

UPDATE awr_team.schema_state SET version=43 WHERE component='awr_team';
COMMIT;
