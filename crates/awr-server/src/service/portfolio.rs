//! Shared member-scoped portfolio read for MCP and the browser entry.

use super::StateData;
use awr_team_pg::{PgError, PortfolioOverviewRequest};
use serde_json::Value;

/// Resolve the serving instance's registered projects for the authenticated
/// tenant and return the store's reply unchanged on both transport surfaces.
pub(super) async fn overview(
    state: &StateData,
    tenant: &str,
    bearer: &str,
    args: Value,
) -> Result<Value, PgError> {
    let request: PortfolioOverviewRequest =
        serde_json::from_value(args).map_err(|_| PgError::invalid_command_fields())?;
    let registered: Vec<String> = state
        .projects
        .values()
        .filter(|registered| registered.tenant_id == tenant)
        .map(|registered| registered.project_id.clone())
        .collect();
    state
        .store
        .portfolio_overview(tenant, bearer, &registered, request)
        .await
}
