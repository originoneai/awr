BEGIN;

-- A review round binds the governance of its work as it stood when the round opened: the contract,
-- the declared definition, its examples and the policy. A change of any of them after the review
-- needs a new review, and only the work whose governance changed is affected.
ALTER TABLE awr_team.review_rounds
    ADD COLUMN governance_binding TEXT
        CHECK (governance_binding IS NULL OR governance_binding ~ '^[0-9a-f]{64}$');

UPDATE awr_team.schema_state SET version=57 WHERE component='awr_team';
COMMIT;
