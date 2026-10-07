BEGIN;

-- Domain acceptance and provider observations have different immutable origins.
-- Existing observations keep their inbox and connector bindings. Legacy ordinary
-- completions are not inferred to have a reviewed delivery candidate.
ALTER TABLE awr_team.completion_receipts
    ADD CONSTRAINT completion_source_receipt_work UNIQUE (tenant_id,project_id,id,work_id),
    ADD CONSTRAINT completion_source_receipt_candidate
        UNIQUE (tenant_id,project_id,id,work_id,delivery_candidate_digest);

ALTER TABLE awr_team.delivery_notifications
    ALTER COLUMN inbox_id DROP NOT NULL,
    ADD COLUMN origin TEXT NOT NULL DEFAULT 'adapter_observation',
    ADD COLUMN completion_receipt_id TEXT,
    ADD CONSTRAINT delivery_notification_origin CHECK (
        (origin='adapter_observation' AND inbox_id IS NOT NULL AND completion_receipt_id IS NULL)
        OR (origin='domain_acceptance' AND inbox_id IS NULL AND completion_receipt_id IS NOT NULL)),
    ADD CONSTRAINT delivery_notification_completion
        FOREIGN KEY (tenant_id,project_id,completion_receipt_id,work_id)
        REFERENCES awr_team.completion_receipts(tenant_id,project_id,id,work_id),
    ADD CONSTRAINT delivery_notification_receipt_once UNIQUE (tenant_id,project_id,completion_receipt_id),
    ADD CONSTRAINT delivery_notification_identity_origin UNIQUE (tenant_id,project_id,id,origin),
    ADD CONSTRAINT delivery_notification_identity_receipt
        UNIQUE (tenant_id,project_id,id,origin,completion_receipt_id);

ALTER TABLE awr_team.delivery_sync_intents
    ALTER COLUMN connector_id DROP NOT NULL,
    ALTER COLUMN connector_version DROP NOT NULL,
    ALTER COLUMN generation DROP NOT NULL,
    ADD COLUMN origin TEXT NOT NULL DEFAULT 'adapter_observation',
    ADD COLUMN completion_receipt_id TEXT,
    ADD CONSTRAINT delivery_intent_origin CHECK (
        (origin='adapter_observation' AND completion_receipt_id IS NULL
            AND connector_id IS NOT NULL AND connector_version IS NOT NULL AND generation IS NOT NULL)
        OR (origin='domain_acceptance' AND completion_receipt_id IS NOT NULL
            AND connector_id IS NULL AND connector_version IS NULL AND generation IS NULL)),
    ADD CONSTRAINT delivery_intent_notification_origin
        FOREIGN KEY (tenant_id,project_id,notification_id,origin)
        REFERENCES awr_team.delivery_notifications(tenant_id,project_id,id,origin),
    ADD CONSTRAINT delivery_intent_notification_receipt
        FOREIGN KEY (tenant_id,project_id,notification_id,origin,completion_receipt_id)
        REFERENCES awr_team.delivery_notifications(tenant_id,project_id,id,origin,completion_receipt_id),
    ADD CONSTRAINT delivery_intent_completion_candidate
        FOREIGN KEY (tenant_id,project_id,completion_receipt_id,work_id,candidate_digest)
        REFERENCES awr_team.completion_receipts(tenant_id,project_id,id,work_id,delivery_candidate_digest);

-- Existing forced RLS policies remain in effect on both extended tables.
UPDATE awr_team.schema_state SET version=49 WHERE component='awr_team';
COMMIT;
