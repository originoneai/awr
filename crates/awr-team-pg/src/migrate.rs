use crate::error::{PgError, PgResult};
use tokio_postgres::Client;

pub const EXPECTED_SCHEMA_VERSION: i32 = 39;
const MIGRATIONS: &[(&str, i32)] = &[
    (include_str!("../migrations/20260917000001_init.sql"), 1),
    (
        include_str!("../migrations/20260918000002_session_wait.sql"),
        2,
    ),
    (
        include_str!("../migrations/20260918000003_graph_resources.sql"),
        3,
    ),
    (
        include_str!("../migrations/20260918000004_execution_protocol.sql"),
        4,
    ),
    (
        include_str!("../migrations/20260918000005_review_completion.sql"),
        5,
    ),
    (
        include_str!("../migrations/20260918000006_import_restore.sql"),
        6,
    ),
    (
        include_str!("../migrations/20260919000007_completion_integrity.sql"),
        7,
    ),
    (
        include_str!("../migrations/20260920000008_execution_result_binding.sql"),
        8,
    ),
    (
        include_str!("../migrations/20260920000009_import_integrity.sql"),
        9,
    ),
    (
        include_str!("../migrations/20260921000010_workstreams.sql"),
        10,
    ),
    (
        include_str!("../migrations/20260921000011_workstream_claims.sql"),
        11,
    ),
    (
        include_str!("../migrations/20260921000012_workstream_executions.sql"),
        12,
    ),
    (
        include_str!("../migrations/20260921000013_workstream_execution_authority.sql"),
        13,
    ),
    (
        include_str!("../migrations/20260921000014_operator_access.sql"),
        14,
    ),
    (
        include_str!("../migrations/20260922000015_history_migrations.sql"),
        15,
    ),
    (
        include_str!("../migrations/20260922000016_backup_operations.sql"),
        16,
    ),
    (
        include_str!("../migrations/20260922000017_operator_quarantines.sql"),
        17,
    ),
    (
        include_str!("../migrations/20260922000018_execution_attributions.sql"),
        18,
    ),
    (
        include_str!("../migrations/20260922000019_responsibility.sql"),
        19,
    ),
    (
        include_str!("../migrations/20260922000020_project_admin_access.sql"),
        20,
    ),
    (
        include_str!("../migrations/20260922000021_planning_drafts.sql"),
        21,
    ),
    (
        include_str!("../migrations/20260922000022_execution_resource_bounds.sql"),
        22,
    ),
    (
        include_str!("../migrations/20260922000023_agent_authorization.sql"),
        23,
    ),
    (
        include_str!("../migrations/20260922000024_team_handoff.sql"),
        24,
    ),
    (
        include_str!("../migrations/20260923000025_review_person_independence.sql"),
        25,
    ),
    (
        include_str!("../migrations/20260923000026_delivery_deps.sql"),
        26,
    ),
    (
        include_str!("../migrations/20260923000027_selective_invalidation.sql"),
        27,
    ),
    (
        include_str!("../migrations/20260923000028_planning_writeback.sql"),
        28,
    ),
    (
        include_str!("../migrations/20260923000029_planning_mcp_ops.sql"),
        29,
    ),
    (
        include_str!("../migrations/20260923000030_pr_delivery_review.sql"),
        30,
    ),
    (
        include_str!("../migrations/20260923000031_ops_audit.sql"),
        31,
    ),
    (
        include_str!("../migrations/20260925000032_project_credentials.sql"),
        32,
    ),
    (
        include_str!("../migrations/20260925000033_request_audit.sql"),
        33,
    ),
    (
        include_str!("../migrations/20260926000034_session_feedback.sql"),
        34,
    ),
    (
        include_str!("../migrations/20260926000035_agent_review.sql"),
        35,
    ),
    (
        include_str!("../migrations/20260926000036_agent_completion.sql"),
        36,
    ),
    (
        include_str!("../migrations/20261005000037_member_identity.sql"),
        37,
    ),
    (
        include_str!("../migrations/20261005000038_business_roles.sql"),
        38,
    ),
    (
        include_str!("../migrations/20261005000039_assignment_grant.sql"),
        39,
    ),
];

pub async fn migrate(client: &Client) -> PgResult<()> {
    let current = schema_version(client).await?;
    if let Some(version) = current {
        if version > EXPECTED_SCHEMA_VERSION {
            return Err(PgError::SchemaIncompatible(format!(
                "schema version {version}, expected {EXPECTED_SCHEMA_VERSION}"
            )));
        }
        if version == EXPECTED_SCHEMA_VERSION {
            return Ok(());
        }
    }
    for (sql, version) in MIGRATIONS {
        if current.unwrap_or(0) >= *version {
            continue;
        }
        client.batch_execute(sql).await?;
    }
    check_schema(client).await
}

async fn schema_version(client: &Client) -> PgResult<Option<i32>> {
    match client
        .query_opt(
            "SELECT version FROM awr_team.schema_state WHERE component='awr_team'",
            &[],
        )
        .await
    {
        Ok(Some(row)) => Ok(Some(row.get(0))),
        Ok(None) => Ok(None),
        // Only an absent schema/table means "not initialized". Other read
        // errors (for example revoked SELECT) must surface as themselves,
        // not be misreported as a missing database (CR #36 P2-1).
        Err(error) => {
            if error
                .as_db_error()
                .map(|db| *db.code() == tokio_postgres::error::SqlState::UNDEFINED_TABLE)
                .unwrap_or(false)
            {
                Ok(None)
            } else {
                Err(PgError::Db(error))
            }
        }
    }
}

pub async fn check_schema(client: &Client) -> PgResult<()> {
    match schema_version(client).await? {
        Some(version) if version == EXPECTED_SCHEMA_VERSION => Ok(()),
        Some(version) => Err(PgError::SchemaIncompatible(format!(
            "schema version {version}, expected {EXPECTED_SCHEMA_VERSION}"
        ))),
        None => Err(PgError::SchemaIncompatible(
            "schema_state missing; refuse to treat an empty or half-migrated database as ready"
                .into(),
        )),
    }
}
