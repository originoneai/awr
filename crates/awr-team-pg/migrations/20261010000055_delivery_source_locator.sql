BEGIN;

-- A public Git locator is management configuration, never a server checkout,
-- credential, filesystem path or claim that AWR verified the referenced tree.
ALTER TABLE awr_team.delivery_connectors
    ADD COLUMN source_locator_contract TEXT,
    ADD COLUMN source_locator_url TEXT,
    ADD COLUMN source_locator_connector_version BIGINT,
    ADD CONSTRAINT delivery_source_locator_complete CHECK (
        (source_locator_contract IS NULL AND source_locator_url IS NULL AND source_locator_connector_version IS NULL)
        OR
        (source_locator_contract IS NOT NULL
         AND source_locator_url IS NOT NULL
         AND source_locator_connector_version IS NOT NULL
         AND source_locator_contract = 'awr-git-source-v1'
         AND source_locator_connector_version > 0
         AND source_locator_connector_version <= version)
    );

UPDATE awr_team.schema_state SET version=55 WHERE component='awr_team';
COMMIT;
