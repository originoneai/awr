use crate::error::PgResult;
use tokio_postgres::Client;

pub struct Bootstrap;

impl Bootstrap {
    pub async fn grant_app(client: &Client, app_role: &str) -> PgResult<()> {
        let ident = quote_ident(app_role);
        client
            .batch_execute(&format!(
                "GRANT USAGE ON SCHEMA awr_team TO {ident};
                 GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA awr_team TO {ident};
                 REVOKE UPDATE, DELETE ON awr_team.events FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.source_snapshots FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.execution_receipts FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.evidence FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.review_decisions FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.completion_receipts FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.workstream_catalogs FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.workstream_snapshot_ownership FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.responsibility_events FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.responsibility_receipts FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.agent_authorization_receipts FROM {ident};
                 -- Governance: what a snapshot declares is immutable and decisions are append-only.
                 REVOKE UPDATE, DELETE ON awr_team.work_admissions FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.snapshot_governance FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.work_governance FROM {ident};
                 REVOKE ALL ON awr_team.access_changes FROM {ident};
                 REVOKE ALL ON awr_team.history_migrations FROM {ident};
                 REVOKE ALL ON awr_team.backup_operations FROM {ident};
                 REVOKE ALL ON awr_team.operator_quarantines FROM {ident};
                 REVOKE ALL ON awr_team.execution_attributions FROM {ident};
                 REVOKE ALL ON awr_team.schema_state FROM {ident};
                 -- Project-admin MCP receipts (TMCP-012): insert/select only.
                 GRANT SELECT, INSERT ON awr_team.project_access_changes TO {ident};
                 REVOKE UPDATE, DELETE ON awr_team.project_access_changes FROM {ident};
                 -- Planning history/approvals/publish receipts (TMCP-021): append-only.
                 REVOKE UPDATE, DELETE ON awr_team.planning_candidate_history FROM {ident};
                 REVOKE UPDATE, DELETE ON awr_team.planning_approvals FROM {ident};
                 -- Publish receipts: UPDATE allowed only so TMCP-022 can clear
                 -- writeback_pending and attach activation metadata; DELETE stays revoked.
                 REVOKE DELETE ON awr_team.planning_publish_receipts FROM {ident};
                 GRANT SELECT, INSERT, UPDATE ON awr_team.planning_publish_receipts TO {ident};
                 GRANT SELECT, INSERT, UPDATE ON awr_team.planning_writeback_journals TO {ident};
                 REVOKE DELETE ON awr_team.planning_writeback_journals FROM {ident};
                 GRANT SELECT, INSERT ON awr_team.planning_activation_receipts TO {ident};
                 GRANT SELECT, INSERT ON awr_team.planning_command_receipts TO {ident};
                 REVOKE UPDATE, DELETE ON awr_team.planning_activation_receipts FROM {ident};
                 -- The app role must read the schema version (check_schema at
                 -- the command entry) but must never modify it (CR #36 P2-1).
                 GRANT SELECT ON awr_team.schema_state TO {ident};"
            ))
            .await?;
        Ok(())
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
