BEGIN;

-- Historical or aggregate recovery barriers stay unattributed.
ALTER TABLE awr_team.execution_receipts
    ADD CONSTRAINT execution_receipts_recovery_identity
    UNIQUE (tenant_id, project_id, execution_id, id);
ALTER TABLE awr_team.work_runtime
    ADD COLUMN recovery_execution_id TEXT,
    ADD COLUMN recovery_receipt_id TEXT,
    ADD CONSTRAINT work_recovery_cause_pair CHECK (
        (recovery_execution_id IS NULL) = (recovery_receipt_id IS NULL)
        AND (recovery_blocked OR recovery_execution_id IS NULL)
    ),
    ADD CONSTRAINT work_recovery_cause_receipt FOREIGN KEY (
        tenant_id, project_id, recovery_execution_id, recovery_receipt_id
    ) REFERENCES awr_team.execution_receipts(tenant_id, project_id, execution_id, id);

-- A second writer can add a cause without changing true to false. Invalidate
-- provenance on every explicit barrier write, including true-to-true updates.
-- The attributed reporter binds its receipt separately in the same transaction.
CREATE FUNCTION awr_team.invalidate_execution_recovery_cause() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.recovery_execution_id := NULL;
    NEW.recovery_receipt_id := NULL;
    RETURN NEW;
END;
$$;
CREATE TRIGGER invalidate_execution_recovery_cause
BEFORE UPDATE OF recovery_blocked ON awr_team.work_runtime
FOR EACH ROW EXECUTE FUNCTION awr_team.invalidate_execution_recovery_cause();

UPDATE awr_team.schema_state SET version=42 WHERE component='awr_team';
COMMIT;
