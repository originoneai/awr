BEGIN;

-- No historical backfill: a current binding cannot establish a past origin.
CREATE FUNCTION awr_team.valid_member_origin(o JSONB) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
    SELECT o IS NULL OR COALESCE(
        jsonb_typeof(o)='object'
        AND o ?& ARRAY['codec','actor_id','client_id','actor_kind','actor_membership_version',
            'member_id','member_actor_kind','member_membership_version','binding_id','member_identity']
        AND o - ARRAY['codec','actor_id','client_id','actor_kind','actor_membership_version',
            'member_id','member_actor_kind','member_membership_version','binding_id','member_identity']='{}'::jsonb
        AND o->>'codec'='awr-member-origin-v1'
        AND jsonb_typeof(o->'actor_id')='string' AND length(o->>'actor_id') BETWEEN 1 AND 128
        AND jsonb_typeof(o->'client_id')='string' AND length(o->>'client_id') BETWEEN 1 AND 128
        AND jsonb_typeof(o->'member_id')='string' AND length(o->>'member_id') BETWEEN 1 AND 128
        AND o->>'actor_kind' IN ('agent','human') AND o->>'member_actor_kind' IN ('agent','human')
        AND jsonb_typeof(o->'actor_membership_version')='string' AND o->>'actor_membership_version' ~ '^[1-9][0-9]*$'
        AND jsonb_typeof(o->'member_membership_version')='string' AND o->>'member_membership_version' ~ '^[1-9][0-9]*$'
        AND ((o->>'actor_kind'='agent' AND jsonb_typeof(o->'binding_id')='string' AND length(o->>'binding_id') BETWEEN 1 AND 128)
            OR (o->>'actor_kind'='human' AND o->'binding_id'='null'::jsonb AND o->>'member_id'=o->>'actor_id'))
        AND (o->'member_identity'='null'::jsonb OR (
            jsonb_typeof(o->'member_identity')='object'
            AND o->'member_identity' ? 'kind'
            AND (o->'member_identity') - ARRAY['kind','controller_ref']='{}'::jsonb
            AND ((o->'member_identity'->>'kind'='simulated_member' AND o->>'member_actor_kind'='agent')
                OR (o->'member_identity'->>'kind'='human' AND o->>'member_actor_kind'='human'))
            AND (NOT o->'member_identity' ? 'controller_ref' OR (
                jsonb_typeof(o->'member_identity'->'controller_ref')='string'
                AND length(o->'member_identity'->>'controller_ref') BETWEEN 1 AND 128))
        )), FALSE);
$$;

CREATE FUNCTION awr_team.valid_member_origins(o JSONB, with_opener BOOLEAN) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
    SELECT o IS NULL OR COALESCE(
        jsonb_typeof(o)='object' AND o ?& ARRAY['codec','executor','submitter']
        AND o->>'codec'=CASE WHEN with_opener THEN 'awr-member-review-origins-v1' ELSE 'awr-member-evidence-origins-v1' END
        AND o - CASE WHEN with_opener THEN ARRAY['codec','executor','submitter','opener'] ELSE ARRAY['codec','executor','submitter'] END='{}'::jsonb
        AND o->'executor'<>'null'::jsonb AND awr_team.valid_member_origin(o->'executor')
        AND o->'executor'->>'actor_kind'='agent' AND o->'executor'->'member_identity'->>'kind'='simulated_member'
        AND o->'submitter'<>'null'::jsonb AND awr_team.valid_member_origin(o->'submitter')
        AND (NOT with_opener OR (o ? 'opener' AND o->'opener'<>'null'::jsonb AND awr_team.valid_member_origin(o->'opener'))), FALSE);
$$;

ALTER TABLE awr_team.executions ADD COLUMN executor_origin_json JSONB,
    ADD CONSTRAINT executions_member_origin_check CHECK (awr_team.valid_member_origin(executor_origin_json)
        AND (executor_origin_json IS NULL OR COALESCE(
            executor_origin_json->>'actor_id'=executor_actor_id AND executor_origin_json->>'client_id'=executor_client_id
            AND executor_origin_json->>'actor_kind'='agent'
            AND executor_origin_json->'member_identity'->>'kind'='simulated_member', FALSE)));
ALTER TABLE awr_team.evidence ADD COLUMN member_origins_json JSONB,
    ADD CONSTRAINT evidence_member_origins_check CHECK (awr_team.valid_member_origins(member_origins_json,FALSE)
        AND (member_origins_json IS NULL OR COALESCE(member_origins_json->'submitter'->>'actor_id'=created_by AND execution_id IS NOT NULL,FALSE)));
ALTER TABLE awr_team.review_rounds ADD COLUMN member_origins_json JSONB,
    ADD CONSTRAINT review_rounds_member_origins_check CHECK (awr_team.valid_member_origins(member_origins_json,TRUE)
        AND (member_origins_json IS NULL OR COALESCE(
            member_origins_json->'opener'->>'actor_id'=author_actor_id
            AND member_origins_json->'opener'->>'client_id'=author_client_id
            AND member_origins_json->'opener'->>'member_id'=author_person_id,FALSE)));

ALTER TABLE awr_team.review_decisions ADD COLUMN member_review_basis_json JSONB,
    DROP CONSTRAINT review_decisions_independence_kind_check,
    DROP CONSTRAINT review_decisions_approval_basis_check,
    ADD CONSTRAINT review_decisions_independence_kind_check CHECK (independence_kind IN (
        'team_independent','personal_self_review','agent_review','unspecified','simulated_member_independent')),
    ADD CONSTRAINT review_decisions_approval_basis_check CHECK (approval_basis IN (
        'human_independent_review','human_author_self_review','agent_review','unspecified','simulated_member_independent_review')),
    ADD CONSTRAINT review_decisions_simulated_basis_check CHECK (
        (independence_kind='simulated_member_independent')=(approval_basis='simulated_member_independent_review')
        AND (member_review_basis_json IS NOT NULL)=(independence_kind='simulated_member_independent')
        AND (member_review_basis_json IS NULL OR COALESCE(
            jsonb_typeof(member_review_basis_json)='object'
            AND member_review_basis_json ?& ARRAY['codec','policy','origins','reviewer','authority','approval_basis','human_approval','team_independent_acceptance']
            AND member_review_basis_json - ARRAY['codec','policy','origins','reviewer','authority','approval_basis','human_approval','team_independent_acceptance']='{}'::jsonb
            AND member_review_basis_json->>'codec'='awr-simulated-member-review-v1'
            AND member_review_basis_json->>'policy'='caller_managed_execution_and_simulated_member_review'
            AND member_review_basis_json->>'approval_basis'=approval_basis
            AND member_review_basis_json->'human_approval'='false'::jsonb
            AND member_review_basis_json->'team_independent_acceptance'='false'::jsonb
            AND member_review_basis_json->'origins'<>'null'::jsonb
            AND awr_team.valid_member_origins(member_review_basis_json->'origins',TRUE)
            AND member_review_basis_json->'reviewer'<>'null'::jsonb
            AND awr_team.valid_member_origin(member_review_basis_json->'reviewer')
            AND member_review_basis_json->'reviewer'->>'actor_kind'='agent'
            AND member_review_basis_json->'reviewer'->'member_identity'->>'kind'='simulated_member'
            AND member_review_basis_json->'reviewer'->>'actor_id'=reviewer_actor_id
            AND member_review_basis_json->'reviewer'->>'client_id'=reviewer_client_id
            AND member_review_basis_json->'reviewer'->>'member_id'=reviewer_person_id
            AND jsonb_typeof(member_review_basis_json->'authority')='object'
            AND member_review_basis_json->'authority' ?& ARRAY['delegation_id','membership_version','workstream_grant_versions','snapshot_id']
            AND (member_review_basis_json->'authority') - ARRAY['delegation_id','membership_version','workstream_grant_versions','snapshot_id']='{}'::jsonb
            AND jsonb_typeof(member_review_basis_json->'authority'->'delegation_id')='string'
            AND length(member_review_basis_json->'authority'->>'delegation_id') BETWEEN 1 AND 128
            AND member_review_basis_json->'authority'->>'membership_version'=member_review_basis_json->'reviewer'->>'actor_membership_version'
            AND jsonb_typeof(member_review_basis_json->'authority'->'workstream_grant_versions')='object'
            AND jsonb_typeof(member_review_basis_json->'authority'->'snapshot_id')='string',FALSE)));

CREATE FUNCTION awr_team.keep_member_origins() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='executions' THEN
        IF OLD.executor_origin_json IS DISTINCT FROM NEW.executor_origin_json THEN
            RAISE EXCEPTION 'executor origin is immutable';
        END IF;
    ELSIF TG_TABLE_NAME='review_decisions' THEN
        IF OLD.member_review_basis_json IS DISTINCT FROM NEW.member_review_basis_json THEN
            RAISE EXCEPTION 'member review basis is immutable';
        END IF;
    ELSE
        IF OLD.member_origins_json IS DISTINCT FROM NEW.member_origins_json THEN
            RAISE EXCEPTION 'member origins are immutable';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER keep_execution_member_origin BEFORE UPDATE ON awr_team.executions
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_member_origins();
CREATE TRIGGER keep_evidence_member_origins BEFORE UPDATE ON awr_team.evidence
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_member_origins();
CREATE TRIGGER keep_round_member_origins BEFORE UPDATE ON awr_team.review_rounds
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_member_origins();
CREATE TRIGGER keep_decision_member_basis BEFORE UPDATE ON awr_team.review_decisions
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_member_origins();

-- The decision carries the exact immutable round origins, not a new resolution.
CREATE FUNCTION awr_team.check_simulated_decision_origins() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE original JSONB; origin JSONB;
BEGIN
    IF NEW.member_review_basis_json IS NOT NULL THEN
        SELECT member_origins_json INTO original FROM awr_team.review_rounds
            WHERE tenant_id=NEW.tenant_id AND project_id=NEW.project_id AND id=NEW.review_round_id
                AND work_id=NEW.work_id AND bundle_hash=NEW.bundle_hash;
        IF original IS NULL OR original IS DISTINCT FROM NEW.member_review_basis_json->'origins' THEN
            RAISE EXCEPTION 'member review origins must match the exact round';
        END IF;
        FOREACH origin IN ARRAY ARRAY[original->'executor',original->'submitter',original->'opener'] LOOP
            IF origin->>'member_id'=NEW.reviewer_person_id OR origin->>'actor_id'=NEW.reviewer_actor_id
                OR origin->>'client_id'=NEW.reviewer_client_id THEN
                RAISE EXCEPTION 'member cannot review an original contribution';
            END IF;
        END LOOP;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER check_decision_member_origins BEFORE INSERT OR UPDATE ON awr_team.review_decisions
    FOR EACH ROW EXECUTE FUNCTION awr_team.check_simulated_decision_origins();

UPDATE awr_team.schema_state SET version=47 WHERE component='awr_team';
COMMIT;
