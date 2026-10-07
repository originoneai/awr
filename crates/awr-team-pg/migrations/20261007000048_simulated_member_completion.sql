BEGIN;

-- Preserve legacy receipts as recorded. No inference or provenance backfill.
ALTER TABLE awr_team.completion_receipts
    DROP CONSTRAINT completion_receipts_independence_kind_check,
    ADD CONSTRAINT completion_receipts_independence_kind_check CHECK (
        independence_kind IS NULL OR independence_kind IN (
            'team_independent','personal_self_review','ordinary_confirm','unspecified',
            'agent_review','simulated_member_independent')),
    ADD CONSTRAINT completion_receipts_simulated_basis_check CHECK (
        independence_kind IS DISTINCT FROM 'simulated_member_independent' OR COALESCE(
            policy='caller_managed_execution_and_simulated_member_review'
            AND approved_by_json->>'approval_basis'='simulated_member_independent_review'
            AND approved_by_json->>'execution_basis' IN ('caller_asserted_reconciled','caller_asserted_workspace_settled')
            AND approved_by_json->'caller_execution_binding'->>'execution_basis'=approved_by_json->>'execution_basis'
            AND approved_by_json->'human_approval'='false'::jsonb
            AND approved_by_json->'team_independent_acceptance'='false'::jsonb
            AND jsonb_typeof(approved_by_json->'member_review_basis')='object'
            AND approved_by_json->'member_review_basis'->>'codec'='awr-simulated-member-review-v1'
            AND approved_by_json->'member_review_basis'->>'policy'=policy
            AND approved_by_json->'member_review_basis'->>'approval_basis'=approved_by_json->>'approval_basis'
            AND approved_by_json->'member_review_basis'->'human_approval'='false'::jsonb
            AND approved_by_json->'member_review_basis'->'team_independent_acceptance'='false'::jsonb
            AND approved_by_json->'member_review_basis'->'origins'<>'null'::jsonb
            AND awr_team.valid_member_origins(approved_by_json->'member_review_basis'->'origins',TRUE)
            AND evidence_id IS NOT NULL AND execution_id IS NOT NULL
            AND jsonb_typeof(approved_by_json->'review_round_id')='string'
            AND jsonb_typeof(approved_by_json->'review_decision_id')='string', FALSE));

CREATE FUNCTION awr_team.keep_simulated_completion_basis() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE original JSONB;
BEGIN
    IF TG_OP='UPDATE' THEN
        IF (OLD.independence_kind='simulated_member_independent' OR NEW.independence_kind='simulated_member_independent')
            AND (OLD.approved_by_json IS DISTINCT FROM NEW.approved_by_json
                OR OLD.independence_kind IS DISTINCT FROM NEW.independence_kind
                OR OLD.policy IS DISTINCT FROM NEW.policy
                OR OLD.work_id IS DISTINCT FROM NEW.work_id
                OR OLD.scope_id IS DISTINCT FROM NEW.scope_id
                OR OLD.contract_hash IS DISTINCT FROM NEW.contract_hash
                OR OLD.result_digest IS DISTINCT FROM NEW.result_digest
                OR OLD.dependency_binding_hash IS DISTINCT FROM NEW.dependency_binding_hash
                OR OLD.evidence_id IS DISTINCT FROM NEW.evidence_id
                OR OLD.execution_id IS DISTINCT FROM NEW.execution_id
                OR OLD.evidence_bundle_hash IS DISTINCT FROM NEW.evidence_bundle_hash
                OR OLD.approved_by_person_id IS DISTINCT FROM NEW.approved_by_person_id
                OR OLD.submitted_by_person_id IS DISTINCT FROM NEW.submitted_by_person_id
                OR OLD.author_actor_id IS DISTINCT FROM NEW.author_actor_id
                OR OLD.owner_person_id IS DISTINCT FROM NEW.owner_person_id
                OR OLD.executor_actor_id IS DISTINCT FROM NEW.executor_actor_id
                OR OLD.final_submitter_actor_id IS DISTINCT FROM NEW.final_submitter_actor_id
                OR OLD.pr_delivery_id IS DISTINCT FROM NEW.pr_delivery_id) THEN
            RAISE EXCEPTION 'simulated completion basis is immutable';
        END IF;
    ELSIF NEW.independence_kind='simulated_member_independent' THEN
        SELECT d.member_review_basis_json INTO original FROM awr_team.review_decisions d
            JOIN awr_team.review_rounds r ON r.tenant_id=d.tenant_id AND r.project_id=d.project_id AND r.id=d.review_round_id
            WHERE d.tenant_id=NEW.tenant_id AND d.project_id=NEW.project_id
                AND d.id=NEW.approved_by_json->>'review_decision_id'
                AND r.id=NEW.approved_by_json->>'review_round_id'
                AND d.work_id=NEW.work_id AND r.work_id=NEW.work_id
                AND d.bundle_hash=NEW.evidence_bundle_hash AND r.bundle_hash=NEW.evidence_bundle_hash
                AND r.contract_hash=NEW.contract_hash AND r.evidence_id=NEW.evidence_id
                AND r.execution_id=NEW.execution_id AND r.state='approved' AND d.decision='approve';
        IF original IS NULL OR original IS DISTINCT FROM NEW.approved_by_json->'member_review_basis' THEN
            RAISE EXCEPTION 'simulated completion must retain the exact original decision';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER keep_completion_simulated_basis BEFORE INSERT OR UPDATE ON awr_team.completion_receipts
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_simulated_completion_basis();

CREATE UNIQUE INDEX completion_simulated_decision_once ON awr_team.completion_receipts
    (tenant_id,project_id,scope_id,work_id,contract_hash,evidence_id,(approved_by_json->>'review_decision_id'))
    WHERE independence_kind='simulated_member_independent';

UPDATE awr_team.schema_state SET version=48 WHERE component='awr_team';
COMMIT;
