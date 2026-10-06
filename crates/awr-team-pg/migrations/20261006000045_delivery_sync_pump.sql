BEGIN;

-- Notification delivery and confirmed source synchronization are distinct facts.
CREATE TABLE awr_team.delivery_sync_intents (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    notification_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('refresh','source')),
    work_id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    connector_version BIGINT NOT NULL CHECK (connector_version>0),
    generation BIGINT NOT NULL CHECK (generation>0),
    candidate_digest TEXT NOT NULL,
    selection_version BIGINT NOT NULL CHECK (selection_version>0),
    read_set_json JSONB NOT NULL CHECK (octet_length(read_set_json::text)<=4096),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','leased','succeeded','superseded','blocked')),
    fence BIGINT NOT NULL DEFAULT 0 CHECK (fence>=0),
    worker_id TEXT,
    actor_id TEXT,
    client_id TEXT,
    authority_binding TEXT CHECK (authority_binding ~ '^[0-9a-f]{64}$'),
    expires_at TIMESTAMPTZ,
    publication_id TEXT,
    attempts BIGINT NOT NULL DEFAULT 0 CHECK (attempts>=0),
    retry_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    failure_code TEXT CHECK (failure_code IN ('source_conflict','source_unavailable','source_failed','preconditions_changed','withdrawn')),
    result_json JSONB CHECK (octet_length(result_json::text)<=32768),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    UNIQUE (tenant_id,project_id,notification_id,kind),
    UNIQUE (tenant_id,project_id,publication_id),
    FOREIGN KEY (tenant_id,project_id,notification_id)
        REFERENCES awr_team.delivery_notifications(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,work_id) REFERENCES awr_team.work_items(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,connector_id) REFERENCES awr_team.delivery_connectors(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,candidate_digest) REFERENCES awr_team.delivery_candidates(tenant_id,project_id,binding_digest),
    FOREIGN KEY (tenant_id,project_id,publication_id) REFERENCES awr_team.delivery_source_publications(tenant_id,project_id,id),
    CHECK (kind='source' OR publication_id IS NULL),
    CHECK (state<>'leased' OR (fence>0 AND worker_id IS NOT NULL AND actor_id IS NOT NULL
        AND client_id IS NOT NULL AND authority_binding IS NOT NULL AND expires_at IS NOT NULL))
);

-- Recover existing pending notifications from their immutable inspection/source
-- bindings, never from a newly invented permission or current snapshot.
INSERT INTO awr_team.delivery_sync_intents
    (tenant_id,project_id,id,notification_id,kind,work_id,connector_id,connector_version,
     generation,candidate_digest,selection_version,read_set_json,created_at)
SELECT n.tenant_id,n.project_id,md5(jsonb_build_array(n.tenant_id,n.project_id,n.id,k.kind)::text),
    n.id,k.kind,n.work_id,i.connector_id,x.connector_version,x.generation,x.binding_digest,x.selection_version,
    jsonb_build_object('work_id',n.work_id,'workstream_id',o.workstream_id,
        'coordinator_epoch',x.coordinator_epoch,'source_snapshot_id',x.source_snapshot_id,
        'authority_version',w.value->>'authority_version','ownership_version',x.ownership_version::text,
        'contract_hash',c.body_json->'binding'->>'contract_hash'),n.created_at
FROM awr_team.delivery_notifications n
JOIN awr_team.delivery_inbox i ON (i.tenant_id,i.project_id,i.id)=(n.tenant_id,n.project_id,n.inbox_id)
JOIN awr_team.delivery_inspections x ON (x.tenant_id,x.project_id,x.id)=(i.tenant_id,i.project_id,i.inspection_id)
JOIN awr_team.delivery_candidates c ON (c.tenant_id,c.project_id,c.binding_digest)=(x.tenant_id,x.project_id,x.binding_digest)
JOIN awr_team.workstream_snapshot_ownership o ON (o.tenant_id,o.project_id,o.snapshot_id,o.work_id)=(n.tenant_id,n.project_id,x.source_snapshot_id,n.work_id)
JOIN awr_team.workstream_catalogs cat ON (cat.tenant_id,cat.project_id,cat.snapshot_id)=(o.tenant_id,o.project_id,o.snapshot_id)
CROSS JOIN LATERAL jsonb_array_elements(cat.catalog_json->'workstreams') w(value)
CROSS JOIN (VALUES ('refresh'),('source')) k(kind)
WHERE n.state='pending' AND i.state='applied' AND w.value->>'id'=o.workstream_id;

CREATE INDEX delivery_sync_intents_pending ON awr_team.delivery_sync_intents
    (tenant_id,project_id,state,retry_at,created_at,id);
ALTER TABLE awr_team.delivery_sync_intents ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.delivery_sync_intents FORCE ROW LEVEL SECURITY;
CREATE POLICY delivery_pump_isolation ON awr_team.delivery_sync_intents USING
    (tenant_id=current_setting('awr.tenant_id',true) AND project_id=current_setting('awr.project_id',true))
    WITH CHECK (tenant_id=current_setting('awr.tenant_id',true) AND project_id=current_setting('awr.project_id',true));

UPDATE awr_team.schema_state SET version=45 WHERE component='awr_team';
COMMIT;
