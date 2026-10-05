BEGIN;

-- Legacy rows stay unspecified. Actor/model labels cannot infer provenance.
ALTER TABLE awr_team.persons ADD COLUMN member_identity JSONB;
ALTER TABLE awr_team.persons ADD CONSTRAINT persons_member_identity_shape CHECK (
    member_identity IS NULL OR (
        jsonb_typeof(member_identity) = 'object'
        AND member_identity ? 'kind'
        AND jsonb_typeof(member_identity->'kind') = 'string'
        AND member_identity->>'kind' IN ('human', 'simulated_member')
        AND member_identity - 'kind' - 'controller_ref' = '{}'::jsonb
        AND (NOT member_identity ? 'controller_ref' OR (
            jsonb_typeof(member_identity->'controller_ref') = 'string'
            AND octet_length(member_identity->>'controller_ref') BETWEEN 1 AND 128
            AND member_identity->>'controller_ref' !~ '[[:cntrl:]]'
        ))
    )
);

UPDATE awr_team.schema_state SET version=37 WHERE component='awr_team';
COMMIT;
