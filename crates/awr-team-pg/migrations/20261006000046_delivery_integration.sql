BEGIN;

-- Original authority references are private service data, never bearer tokens.
CREATE TABLE awr_team.delivery_integration_intents (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    request_json JSONB NOT NULL,
    eligibility_json JSONB NOT NULL,
    eligibility_digest TEXT NOT NULL CHECK (eligibility_digest ~ '^[0-9a-f]{64}$'),
    issuer_credential_id TEXT NOT NULL,
    issuer_secret_hash TEXT NOT NULL CHECK (issuer_secret_hash ~ '^sha256:[0-9a-f]{64}$'),
    issuer_authority_binding TEXT NOT NULL CHECK (issuer_authority_binding ~ '^[0-9a-f]{64}$'),
    state TEXT NOT NULL CHECK (state IN ('prepared','leased','dispatched','unknown','confirmed','rejected')),
    lease_id TEXT,
    lease_actor_id TEXT,
    lease_client_id TEXT,
    lease_authority_binding TEXT,
    lease_expires_at TIMESTAMPTZ,
    dispatched_at TIMESTAMPTZ,
    dispatch_receipt_json JSONB,
    confirmation_fact_id TEXT,
    confirmation_current BOOLEAN,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,work_id) REFERENCES awr_team.work_items(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,connector_id) REFERENCES awr_team.delivery_connectors(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,confirmation_fact_id) REFERENCES awr_team.delivery_facts(tenant_id,project_id,id),
    CHECK (state NOT IN ('dispatched','unknown','confirmed') OR dispatched_at IS NOT NULL),
    CHECK (state!='confirmed' OR confirmation_fact_id IS NOT NULL),
    CHECK (state!='leased' OR (lease_id IS NOT NULL AND lease_actor_id IS NOT NULL
        AND lease_client_id IS NOT NULL AND lease_authority_binding IS NOT NULL
        AND lease_expires_at IS NOT NULL))
);

-- Scope by tenant and logical target, including contention across projects.
-- RLS conceals another project's owner; conflicting acquisition only reports
-- a blocked target and cannot disclose or release its pending effect.
CREATE TABLE awr_team.delivery_integration_target_guards (
    tenant_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    reference TEXT NOT NULL,
    project_id TEXT NOT NULL,
    intent_id TEXT NOT NULL,
    PRIMARY KEY (tenant_id,resource,reference),
    UNIQUE (tenant_id,project_id,intent_id),
    FOREIGN KEY (tenant_id,project_id,intent_id)
        REFERENCES awr_team.delivery_integration_intents(tenant_id,project_id,id)
);

CREATE INDEX delivery_integration_work ON awr_team.delivery_integration_intents
    (tenant_id,project_id,work_id,created_at,id);

DO $$ DECLARE name TEXT; BEGIN
    FOREACH name IN ARRAY ARRAY['delivery_integration_intents','delivery_integration_target_guards'] LOOP
        EXECUTE format('ALTER TABLE awr_team.%I ENABLE ROW LEVEL SECURITY',name);
        EXECUTE format('ALTER TABLE awr_team.%I FORCE ROW LEVEL SECURITY',name);
        EXECUTE format('CREATE POLICY delivery_integration_isolation ON awr_team.%I USING
            (tenant_id=current_setting(''awr.tenant_id'',true) AND project_id=current_setting(''awr.project_id'',true))
            WITH CHECK (tenant_id=current_setting(''awr.tenant_id'',true) AND project_id=current_setting(''awr.project_id'',true))',name);
    END LOOP;
END $$;

UPDATE awr_team.schema_state SET version=46 WHERE component='awr_team';
COMMIT;
