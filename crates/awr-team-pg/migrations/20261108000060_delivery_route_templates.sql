BEGIN;

-- Templates expose an existing mapping for future same-workstream reuse.
-- They neither carry credentials nor change any existing task mapping.
CREATE TABLE awr_team.delivery_route_templates (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    workstream_id TEXT NOT NULL,
    source_connector_id TEXT NOT NULL,
    source_connector_version BIGINT NOT NULL CHECK (source_connector_version > 0),
    version BIGINT NOT NULL CHECK (version > 0),
    coordinator_epoch TEXT NOT NULL,
    enabled BOOLEAN NOT NULL,
    configured_by_actor_id TEXT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, source_connector_id)
        REFERENCES awr_team.delivery_connectors(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, configured_by_actor_id)
        REFERENCES awr_team.actors(tenant_id, id)
);
CREATE INDEX delivery_route_templates_by_stream
    ON awr_team.delivery_route_templates(tenant_id, project_id, workstream_id, id);
ALTER TABLE awr_team.delivery_route_templates ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.delivery_route_templates FORCE ROW LEVEL SECURITY;
CREATE POLICY delivery_route_template_isolation ON awr_team.delivery_route_templates
    USING (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=60 WHERE component='awr_team';
COMMIT;
