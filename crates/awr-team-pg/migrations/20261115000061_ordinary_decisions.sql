BEGIN;

-- Ordinary questions retain the existing work-level wait and its admission guards.
-- Legacy waits keep null decision metadata and preserve their original behavior.
ALTER TABLE awr_team.wait_items
    ADD COLUMN decision_key TEXT,
    ADD COLUMN decision_baseline_digest TEXT,
    ADD COLUMN decision_json JSONB,
    ADD COLUMN decision_version BIGINT,
    ADD CONSTRAINT wait_decision_metadata CHECK (
        (decision_key IS NULL AND decision_baseline_digest IS NULL
            AND decision_json IS NULL AND decision_version IS NULL)
        OR (decision_key IS NOT NULL AND decision_baseline_digest IS NOT NULL
            AND decision_json IS NOT NULL AND decision_version IS NOT NULL
            AND length(decision_key) BETWEEN 1 AND 128
            AND decision_baseline_digest ~ '^[0-9a-f]{64}$'
            AND jsonb_typeof(decision_json) = 'object' AND decision_version >= 1)
    );

CREATE UNIQUE INDEX wait_decision_request_identity
    ON awr_team.wait_items(tenant_id, project_id, work_id, decision_key, decision_baseline_digest)
    WHERE decision_key IS NOT NULL;
CREATE INDEX wait_decision_catalog
    ON awr_team.wait_items(tenant_id, project_id, id DESC)
    WHERE decision_key IS NOT NULL;
CREATE INDEX wait_decision_work_history
    ON awr_team.wait_items(tenant_id, project_id, work_id, id DESC)
    WHERE decision_key IS NOT NULL;

CREATE FUNCTION awr_team.keep_ordinary_decision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.decision_key IS NULL THEN
        IF TG_OP = 'UPDATE' AND NEW.decision_key IS NOT NULL THEN
            RAISE EXCEPTION 'legacy waits cannot acquire decision authority';
        END IF;
        RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
    END IF;
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'ordinary decision history is retained';
    END IF;
    IF OLD.state <> 'open' OR NEW.state NOT IN ('replied', 'cancelled')
        OR NEW.decision_version IS DISTINCT FROM OLD.decision_version + 1
        OR NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.project_id IS DISTINCT FROM OLD.project_id
        OR NEW.id IS DISTINCT FROM OLD.id
        OR NEW.decision_key IS DISTINCT FROM OLD.decision_key
        OR NEW.decision_baseline_digest IS DISTINCT FROM OLD.decision_baseline_digest
        OR NEW.question IS DISTINCT FROM OLD.question
        OR NEW.session_id IS DISTINCT FROM OLD.session_id
        OR NEW.work_id IS DISTINCT FROM OLD.work_id
        OR NEW.created_at IS DISTINCT FROM OLD.created_at
        OR (NEW.decision_json - 'resolution') IS DISTINCT FROM (OLD.decision_json - 'resolution')
        OR jsonb_typeof(NEW.decision_json->'resolution') IS DISTINCT FROM 'object'
    THEN
        RAISE EXCEPTION 'ordinary decisions require one versioned attributed resolution';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER keep_ordinary_decision BEFORE UPDATE OR DELETE ON awr_team.wait_items
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_ordinary_decision();

UPDATE awr_team.schema_state SET version=61 WHERE component='awr_team';
COMMIT;
