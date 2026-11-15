BEGIN;

ALTER TABLE awr_team.planning_candidates DROP CONSTRAINT planning_candidates_state_check;
ALTER TABLE awr_team.planning_candidates ADD CONSTRAINT planning_candidates_state_check
    CHECK (state IN ('drafting', 'approved', 'published', 'superseded', 'rejected'));
ALTER TABLE awr_team.planning_command_receipts DROP CONSTRAINT planning_command_receipts_op_check;
ALTER TABLE awr_team.planning_command_receipts ADD CONSTRAINT planning_command_receipts_op_check
    CHECK (op IN ('planning.propose', 'planning.edit_draft', 'planning.approve',
        'planning.reject', 'planning.publish', 'planning.activate'));

CREATE TABLE awr_team.planning_rejections (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    id TEXT NOT NULL,
    candidate_digest TEXT NOT NULL CHECK (candidate_digest ~ '^[0-9a-f]{64}$'),
    draft_revision INT NOT NULL CHECK (draft_revision >= 1),
    reviewer_person_id TEXT NOT NULL,
    reviewer_actor_id TEXT NOT NULL,
    reviewer_client_id TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (length(btrim(reason)) > 0 AND octet_length(reason) <= 4096),
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, candidate_id),
    UNIQUE (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, candidate_id)
        REFERENCES awr_team.planning_candidates(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, request_id)
        REFERENCES awr_team.planning_command_receipts(tenant_id, project_id, request_id)
);

ALTER TABLE awr_team.planning_rejections ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_rejections FORCE ROW LEVEL SECURITY;
CREATE POLICY planning_rejections_isolation ON awr_team.planning_rejections
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

CREATE FUNCTION awr_team.keep_planning_rejection_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'planning rejection history is retained';
END;
$$;
CREATE TRIGGER keep_planning_rejection_history BEFORE UPDATE OR DELETE ON awr_team.planning_rejections
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_planning_rejection_history();

UPDATE awr_team.schema_state SET version=63 WHERE component='awr_team';
COMMIT;
