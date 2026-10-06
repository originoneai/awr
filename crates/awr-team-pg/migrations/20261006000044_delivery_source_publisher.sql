BEGIN;

-- Legacy receipts remain unbound. Only a domain finalizer can bind a new one;
-- observing an external merge or writing source metadata cannot finalize work.
ALTER TABLE awr_team.completion_receipts ADD COLUMN delivery_candidate_digest TEXT
    CHECK (delivery_candidate_digest ~ '^[0-9a-f]{64}$');
ALTER TABLE awr_team.completion_receipts ADD FOREIGN KEY
    (tenant_id,project_id,delivery_candidate_digest)
    REFERENCES awr_team.delivery_candidates(tenant_id,project_id,binding_digest);

-- Physical metadata evolves independently of the immutable contract snapshot.
CREATE TABLE awr_team.delivery_source_cursors (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    source_snapshot_id TEXT NOT NULL,
    metadata_revision BIGINT NOT NULL DEFAULT 0 CHECK (metadata_revision>=0),
    confirmed_fingerprint TEXT NOT NULL CHECK (confirmed_fingerprint ~ '^sha256:[0-9a-f]{64}$'),
    last_fence BIGINT NOT NULL DEFAULT 0 CHECK (last_fence>=0),
    pending_publication_id TEXT,
    PRIMARY KEY (tenant_id,project_id,source_snapshot_id),
    FOREIGN KEY (tenant_id,project_id,source_snapshot_id)
        REFERENCES awr_team.source_snapshots(tenant_id,project_id,id)
);

CREATE TABLE awr_team.delivery_source_publications (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    source_snapshot_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    authority_binding TEXT NOT NULL CHECK (authority_binding ~ '^[0-9a-f]{64}$'),
    read_set_json JSONB NOT NULL,
    candidate_digest TEXT NOT NULL,
    selection_version BIGINT NOT NULL CHECK (selection_version>0),
    metadata_revision BIGINT NOT NULL CHECK (metadata_revision>0),
    note_json JSONB NOT NULL CHECK (octet_length(note_json::text)<=32768),
    filesystem_identity_json JSONB NOT NULL,
    before_fingerprint TEXT NOT NULL CHECK (before_fingerprint ~ '^sha256:[0-9a-f]{64}$'),
    after_fingerprint TEXT NOT NULL CHECK (after_fingerprint ~ '^sha256:[0-9a-f]{64}$'),
    before_bytes BYTEA NOT NULL CHECK (octet_length(before_bytes)<=4194304),
    after_bytes BYTEA NOT NULL CHECK (octet_length(after_bytes)<=4194304),
    projection_json JSONB NOT NULL,
    fence BIGINT NOT NULL CHECK (fence>0),
    expires_at TIMESTAMPTZ NOT NULL,
    phase TEXT NOT NULL DEFAULT 'pending'
        CHECK (phase IN ('pending','source_written','confirmed','conflict','failed')),
    failure_code TEXT CHECK (failure_code IN
        ('source_identity_changed','source_drift','source_unavailable','write_failed','projection_changed','lease_expired','withdrawn')),
    observed_fingerprint TEXT,
    source_written_observed BOOLEAN NOT NULL DEFAULT false,
    confirmation_json JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,source_snapshot_id)
        REFERENCES awr_team.delivery_source_cursors(tenant_id,project_id,source_snapshot_id),
    FOREIGN KEY (tenant_id,project_id,work_id) REFERENCES awr_team.work_items(tenant_id,project_id,id),
    FOREIGN KEY (tenant_id,project_id,candidate_digest)
        REFERENCES awr_team.delivery_candidates(tenant_id,project_id,binding_digest),
    CHECK ((phase='confirmed')=(confirmation_json IS NOT NULL))
);
ALTER TABLE awr_team.delivery_source_cursors ADD FOREIGN KEY
    (tenant_id,project_id,pending_publication_id)
    REFERENCES awr_team.delivery_source_publications(tenant_id,project_id,id);

CREATE INDEX delivery_source_pending ON awr_team.delivery_source_publications
    (tenant_id,project_id,phase,expires_at,id);

DO $$ DECLARE name TEXT; BEGIN
    FOREACH name IN ARRAY ARRAY['delivery_source_cursors','delivery_source_publications'] LOOP
        EXECUTE format('ALTER TABLE awr_team.%I ENABLE ROW LEVEL SECURITY',name);
        EXECUTE format('ALTER TABLE awr_team.%I FORCE ROW LEVEL SECURITY',name);
        EXECUTE format('CREATE POLICY delivery_source_isolation ON awr_team.%I USING
            (tenant_id=current_setting(''awr.tenant_id'',true) AND project_id=current_setting(''awr.project_id'',true))
            WITH CHECK (tenant_id=current_setting(''awr.tenant_id'',true) AND project_id=current_setting(''awr.project_id'',true))',name);
    END LOOP;
END $$;

UPDATE awr_team.schema_state SET version=44 WHERE component='awr_team';
COMMIT;
