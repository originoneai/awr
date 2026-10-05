BEGIN;

-- Assignment is opt-in. Existing memberships retain their original powers.
ALTER TABLE awr_team.project_memberships
    ADD COLUMN assignment_grant BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE awr_team.schema_state SET version = 39 WHERE component = 'awr_team';

COMMIT;
