BEGIN;

-- Definition retirement does not create runtime completion or acceptance facts.
ALTER TABLE awr_team.work_contracts
    DROP CONSTRAINT work_contracts_definition_state_check;
ALTER TABLE awr_team.work_contracts
    ADD CONSTRAINT work_contracts_definition_state_check
    CHECK (definition_state IN ('draft', 'enabled', 'archived', 'cancelled'));

UPDATE awr_team.schema_state SET version=59 WHERE component='awr_team';
COMMIT;
