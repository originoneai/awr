BEGIN;

-- Nullable provenance prevents historical runs from acquiring a new policy.
ALTER TABLE awr_team.executions
    ADD COLUMN settlement_policy_json JSONB,
    ADD COLUMN admission_mode TEXT
        CHECK (admission_mode IS NULL OR admission_mode IN ('caller_managed','reference_write_v1')),
    ADD COLUMN admission_lease_version BIGINT
        CHECK (admission_lease_version IS NULL OR admission_lease_version > 0),
    ADD COLUMN terminal_reported BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN workspace_effects_settled BOOLEAN NOT NULL DEFAULT false,
    ADD CONSTRAINT executions_settlement_policy_check CHECK (
        settlement_policy_json IS NULL OR (
            jsonb_typeof(settlement_policy_json)='object'
            AND settlement_policy_json - 'mode' - 'workspace_id'='{}'::jsonb
            AND settlement_policy_json->>'mode'='independent_workspace_v1'
            AND jsonb_typeof(settlement_policy_json->'workspace_id')='string'
            AND settlement_policy_json->>'workspace_id' ~ '^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$'
        ) IS TRUE
    ),
    ADD CONSTRAINT executions_admission_provenance_check CHECK (
        (admission_mode IS NULL)=(admission_lease_version IS NULL)
    ),
    ADD CONSTRAINT executions_workspace_settlement_check CHECK (
        NOT workspace_effects_settled OR (
            settlement_policy_json IS NOT NULL
            AND admission_mode='caller_managed'
            AND admission_lease_version IS NOT NULL
            AND terminal_reported
            AND state IN ('succeeded','failed','cancelled')
        ) IS TRUE
    );

-- Keep the old reconciled tuple, and require an explicit verified binding for
-- the new lower-trust workspace tuple. Neither tuple implies human approval.
ALTER TABLE awr_team.completion_receipts
    DROP CONSTRAINT completion_receipts_agent_basis_check,
    ADD CONSTRAINT completion_receipts_agent_basis_check CHECK (
        independence_kind IS DISTINCT FROM 'agent_review' OR (
            policy='caller_managed_execution_and_agent_review'
            AND approved_by_json->>'approval_basis'='agent_review'
            AND approved_by_json->>'human_approval'='false'
            AND approved_by_json->>'team_independent_acceptance'='false'
            AND (
                approved_by_json->>'execution_basis'='caller_asserted_reconciled'
                OR (
                    approved_by_json->>'execution_basis'='caller_asserted_workspace_settled'
                    AND approved_by_json->'caller_execution_binding'->>'execution_basis'='caller_asserted_workspace_settled'
                    AND approved_by_json->'caller_execution_binding'->>'settlement_mode'='independent_workspace_v1'
                    AND approved_by_json->'caller_execution_binding'->>'terminal_reported'='true'
                    AND approved_by_json->'caller_execution_binding'->>'artifact_verified'='true'
                    AND approved_by_json->'caller_execution_binding'->>'effects_settled'='true'
                )
            )
        ) IS TRUE
    );

UPDATE awr_team.schema_state SET version=41 WHERE component='awr_team';
COMMIT;
