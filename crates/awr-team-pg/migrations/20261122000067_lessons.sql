BEGIN;

-- Attribution binds the authenticated Agent's person and task session.
CREATE TABLE awr_team.lessons (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    author_person_id TEXT NOT NULL,
    recorded_by_actor_id TEXT NOT NULL,
    recorded_by_client_id TEXT NOT NULL,
    content_json JSONB NOT NULL CHECK (jsonb_typeof(content_json) = 'object'),
    stage TEXT NOT NULL DEFAULT 'candidate' CHECK (stage IN ('candidate','project','team_rule')),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','revoked')),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,session_id) REFERENCES awr_team.sessions(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,author_person_id) REFERENCES awr_team.persons(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,work_id) REFERENCES awr_team.work_items(tenant_id,project_id,id)
);
CREATE INDEX lessons_work ON awr_team.lessons(tenant_id,project_id,work_id,id);

CREATE TABLE awr_team.lesson_feedback (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    lesson_id TEXT NOT NULL,
    person_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (octet_length(reason) BETWEEN 1 AND 2048 AND length(btrim(reason)) > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,lesson_id) REFERENCES awr_team.lessons(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,person_id) REFERENCES awr_team.persons(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,session_id) REFERENCES awr_team.sessions(tenant_id,project_id,id)
);

ALTER TABLE awr_team.lessons ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.lessons FORCE ROW LEVEL SECURITY;
CREATE POLICY lessons_isolation ON awr_team.lessons
    USING (tenant_id = current_setting('awr.tenant_id',true) AND project_id = current_setting('awr.project_id',true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id',true) AND project_id = current_setting('awr.project_id',true));
ALTER TABLE awr_team.lesson_feedback ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.lesson_feedback FORCE ROW LEVEL SECURITY;
CREATE POLICY lesson_feedback_isolation ON awr_team.lesson_feedback
    USING (tenant_id = current_setting('awr.tenant_id',true) AND project_id = current_setting('awr.project_id',true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id',true) AND project_id = current_setting('awr.project_id',true));

UPDATE awr_team.schema_state SET version=67 WHERE component='awr_team';
COMMIT;
