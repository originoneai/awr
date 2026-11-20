BEGIN;

-- portfolio.overview reads member-visible projects in one batched statement.
-- Every domain table's isolation policy pins a transaction to one project, so
-- the tables this read aggregates add one permissive SELECT policy: rows are
-- readable when the transaction set both markers, tenant and portfolio_read.
-- Both are transaction-local; only the portfolio read sets them, after the
-- same transaction resolved the member, and every statement it runs then
-- filters project_id = ANY(visible) itself. With the marker unset each table
-- behaves exactly as before. No write path changes: the policies are
-- SELECT-only.
CREATE POLICY projects_portfolio_read ON awr_team.projects
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY project_memberships_portfolio_read ON awr_team.project_memberships
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY workstream_modes_portfolio_read ON awr_team.workstream_modes
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY workstream_catalogs_portfolio_read ON awr_team.workstream_catalogs
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY workstream_grants_portfolio_read ON awr_team.workstream_grants
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY agent_authorizations_portfolio_read ON awr_team.agent_authorizations
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY person_agent_bindings_portfolio_read ON awr_team.person_agent_bindings
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY persons_portfolio_read ON awr_team.persons
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY work_contracts_portfolio_read ON awr_team.work_contracts
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY workstream_snapshot_ownership_portfolio_read
    ON awr_team.workstream_snapshot_ownership
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY work_runtime_portfolio_read ON awr_team.work_runtime
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY sessions_portfolio_read ON awr_team.sessions
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY events_portfolio_read ON awr_team.events
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY checkpoints_portfolio_read ON awr_team.checkpoints
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY wait_items_portfolio_read ON awr_team.wait_items
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY review_rounds_portfolio_read ON awr_team.review_rounds
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY project_period_updates_portfolio_read ON awr_team.project_period_updates
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

CREATE POLICY project_period_update_current_portfolio_read
    ON awr_team.project_period_update_current
    FOR SELECT USING (
        current_setting('awr.portfolio_read', true) = 'on'
        AND tenant_id = current_setting('awr.tenant_id', true));

UPDATE awr_team.schema_state SET version=66 WHERE component='awr_team';
COMMIT;
