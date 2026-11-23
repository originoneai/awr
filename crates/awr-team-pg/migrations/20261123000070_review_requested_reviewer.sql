BEGIN;

-- A requested recipient is routing metadata, separate from an actual decision.
ALTER TABLE awr_team.review_rounds ADD COLUMN requested_reviewer JSONB;
ALTER TABLE awr_team.review_rounds ADD CONSTRAINT review_requested_reviewer_shape CHECK (
    requested_reviewer IS NULL OR (
    (requested_reviewer = '{"kind":"role","role":"reviewer"}'::jsonb) OR
    (jsonb_typeof(requested_reviewer)='object'
     AND requested_reviewer->>'kind'='member'
     AND jsonb_typeof(requested_reviewer->'person_id')='string'
     AND length(requested_reviewer->>'person_id') BETWEEN 1 AND 128
     AND requested_reviewer - 'kind' - 'person_id' = '{}'::jsonb)) IS TRUE
);

UPDATE awr_team.schema_state SET version=70 WHERE component='awr_team';
COMMIT;
