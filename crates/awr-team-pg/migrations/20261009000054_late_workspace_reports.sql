BEGIN;

-- Admit the new explicit policy without changing any historical run or receipt.
ALTER TABLE awr_team.executions
    DROP CONSTRAINT executions_settlement_policy_check,
    ADD CONSTRAINT executions_settlement_policy_check CHECK (
        settlement_policy_json IS NULL OR (
            jsonb_typeof(settlement_policy_json)='object'
            AND settlement_policy_json - 'mode' - 'workspace_id'='{}'::jsonb
            AND settlement_policy_json->>'mode' IN ('independent_workspace_v1','independent_workspace_v2')
            AND jsonb_typeof(settlement_policy_json->'workspace_id')='string'
            AND settlement_policy_json->>'workspace_id' ~ '^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$'
        ) IS TRUE
    );

-- Retain the exact caller-artifact/independent-review requirements for both modes.
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
                    AND approved_by_json->'caller_execution_binding'->>'settlement_mode' IN ('independent_workspace_v1','independent_workspace_v2')
                    AND approved_by_json->'caller_execution_binding'->>'terminal_reported'='true'
                    AND approved_by_json->'caller_execution_binding'->>'artifact_verified'='true'
                    AND approved_by_json->'caller_execution_binding'->>'effects_settled'='true'
                )
            )
        ) IS TRUE
    );

UPDATE awr_team.schema_state SET version=54 WHERE component='awr_team';
COMMIT;
