BEGIN;

-- AWR-PPD-062: the name a client registered for itself (OAuth dynamic registration
-- `client_name`) at the moment the member approved a connection with this credential.
-- The server bounds a registration to 120 characters; the column keeps the same bound
-- and refuses control characters, so only reduced display text can be stored.
-- The name is self-reported and unverified: audit reads disclose it under the same
-- directory-read gate as actor names, and it takes part in no authorization,
-- independence check or identity merge. Two clients may report the same name and stay
-- separate credentials. Credentials issued before this migration, and credentials that
-- never approved a connection, keep NULL.
ALTER TABLE awr_team.credentials ADD COLUMN client_name text
    CHECK (client_name IS NULL OR (
        char_length(client_name) BETWEEN 1 AND 120 AND client_name !~ '[[:cntrl:]]'));

UPDATE awr_team.schema_state SET version=68 WHERE component='awr_team';
COMMIT;
