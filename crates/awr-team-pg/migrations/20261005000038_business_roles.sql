BEGIN;

-- NULL retains legacy grants. Declared duties are ceilings, never new grants.
ALTER TABLE awr_team.project_memberships
    ADD COLUMN business_roles JSONB,
    ADD CONSTRAINT membership_business_roles_shape CHECK (
        business_roles IS NULL OR CASE
            WHEN jsonb_typeof(business_roles) = 'array' THEN
                jsonb_array_length(business_roles) BETWEEN 1 AND 6
                AND business_roles <@ '["observer","developer","reviewer","supervisor","deliverer","administrator"]'::jsonb
            ELSE false
        END
    );

COMMENT ON COLUMN awr_team.project_memberships.business_roles IS
    'Explicit business duty ceilings; NULL preserves legacy permission policy. Changes must increment membership_version.';

UPDATE awr_team.schema_state SET version = 38 WHERE component = 'awr_team';

COMMIT;
