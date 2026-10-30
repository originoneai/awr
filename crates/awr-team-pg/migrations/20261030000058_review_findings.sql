BEGIN;

-- Findings of a review, append-only. A finding is recorded once, with the severity the record gives
-- it (blocking findings return the work, the rest are deferred records) and the basis for that, and
-- may be resolved once by the decision that deals with it. Both are attributed to the verified
-- reviewer.
CREATE TABLE awr_team.review_findings (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    review_round_id TEXT NOT NULL,
    round_index INTEGER NOT NULL CHECK (round_index >= 1),
    finding_key TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('record', 'resolve')),
    severity TEXT CHECK (severity IN ('blocking', 'non_blocking')),
    disposition TEXT CHECK (disposition IN ('returns_work', 'deferred')),
    basis TEXT,
    input_json JSONB NOT NULL CHECK (jsonb_typeof(input_json) = 'object'),
    resolution TEXT,
    reviewer_actor_id TEXT NOT NULL,
    reviewer_person_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    UNIQUE (tenant_id, project_id, work_id, finding_key, action),
    FOREIGN KEY (tenant_id, project_id, work_id)
        REFERENCES awr_team.work_items(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, review_round_id)
        REFERENCES awr_team.review_rounds(tenant_id, project_id, id),
    CHECK (
        (action = 'record' AND severity IS NOT NULL AND disposition IS NOT NULL
            AND basis IS NOT NULL AND resolution IS NULL
            AND (severity = 'blocking') = (disposition = 'returns_work'))
        OR (action = 'resolve' AND severity IS NULL AND disposition IS NULL
            AND basis IS NULL AND resolution IS NOT NULL)
    )
);
CREATE INDEX review_findings_by_work
    ON awr_team.review_findings(tenant_id, project_id, work_id, created_at, id);

CREATE FUNCTION awr_team.keep_review_findings() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'review findings are append-only';
END;
$$;
CREATE TRIGGER keep_review_findings BEFORE UPDATE OR DELETE ON awr_team.review_findings
    FOR EACH ROW EXECUTE FUNCTION awr_team.keep_review_findings();

ALTER TABLE awr_team.review_findings ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.review_findings FORCE ROW LEVEL SECURITY;
CREATE POLICY review_findings_isolation ON awr_team.review_findings
    USING (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=58 WHERE component='awr_team';
COMMIT;
