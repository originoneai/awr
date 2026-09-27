//! Schema-owner operator access CLI (bootstrap, recovery, and credential file install).
//! Daily project member/role/credential changes after the first admin use the
//! TMCP-012 MCP/HTTP `access.*` business entry (`ProjectAccessStore`), not this
//! owner connection.

use awr_team_pg::{
    AccessPlan, AgentAuthorizationIssuePlan, AgentProvisionPlan, AgentRenewPlan,
    ExecutionAttributionPlan, OperatorAccess, OperatorAgent, OperatorBackup,
    OperatorExecutionAttribution, OperatorHistory, OperatorQuarantine, OperatorRecovery, PgError,
};
use clap::Subcommand;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimExplainDocument {
    project_id: String,
    work_item_id: String,
    task_workstream_id: Option<String>,
    candidate_person: awr_core::PersonId,
    authorization: Option<awr_core::AgentAuthorization>,
    now_ms: i64,
    is_project_member: bool,
    membership_version: u64,
    assignment_policy: String,
    assignment_policy_allows: bool,
    required_resources: Vec<String>,
    available_resource_ids: BTreeSet<String>,
    required_host_capabilities: Vec<String>,
    verified_host_capabilities: BTreeSet<String>,
    host_id: String,
    dependencies_satisfied: bool,
    task: awr_core::TaskResponsibility,
    requested_executor: awr_core::ExecutionInstance,
}

#[derive(Subcommand)]
pub enum AccessCommand {
    /// Preview an initial person-Agent binding and scoped delegation (owner only).
    AgentPreview {
        #[arg(long)]
        input: PathBuf,
    },
    /// Apply the reviewed initial delegation atomically; never rewrites actors.
    AgentApply {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
    },
    /// Recover the redacted receipt for an initial Agent provisioning request.
    AgentOutcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },
    /// Preview a finite Agent authorization for an additional independent scope.
    AgentAuthorizePreview {
        #[arg(long)]
        input: PathBuf,
    },
    /// Issue the reviewed additional authorization without changing access.
    AgentAuthorizeApply {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
    },
    /// Recover an additional Agent authorization result before an exact retry.
    AgentAuthorizeOutcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },
    /// Preview a finite-lived successor for an expired Agent authorization.
    AgentRenewPreview {
        #[arg(long)]
        input: PathBuf,
    },
    /// Issue the reviewed successor without changing the existing binding.
    AgentRenewApply {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
    },
    /// Recover an Agent authorization renewal result before an exact retry.
    AgentRenewOutcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },
    /// Compare live access, responsibility and delegation with a retained plan.
    AgentInspect {
        #[arg(long)]
        input: PathBuf,
    },
    /// Generate a bearer into a new local file; print only registration metadata.
    Token {
        #[arg(long)]
        credential_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect one actor/client's project access through the schema-owner connection.
    Inspect {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        actor_id: String,
        #[arg(long)]
        client_id: String,
    },
    /// Preview an access plan without changing policy.
    Preview {
        #[arg(long)]
        input: PathBuf,
    },
    /// Apply the exact reviewed plan; a stale state digest is rejected.
    Apply {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
    },
    /// Inspect an original request after a timeout before retrying it exactly.
    Outcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },
    /// Owner-only read-only recovery diagnostics for an enabled workstream project.
    /// Never restores, migrates history, or clears recovery blocks.
    RecoveryInspect {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
    },
    /// Preview unattributed history attribution for an enabled project (no writes).
    HistoryPreview {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
    },
    /// Apply the exact reviewed history-migration plan digests.
    HistoryApply {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
    },
    /// Inspect a history-migration request outcome before retrying it exactly.
    HistoryOutcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },
    /// Preview active-claim / unattributed-execution recovery (owner only; no writes).
    QuarantinePreview {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        /// `release` (default) or `quarantine` when ownership cannot attribute.
        #[arg(long, default_value = "release")]
        claim_disposition: String,
    },
    /// Apply the exact reviewed claim/execution recovery plan digests.
    QuarantineApply {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
        #[arg(long, default_value = "release")]
        claim_disposition: String,
    },
    /// Inspect a claim/execution recovery request outcome before retrying.
    QuarantineOutcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },

    /// Preview explicit execution attribution with reviewed executor_client_id (owner only; no writes).
    ExecutionAttributionPreview {
        #[arg(long)]
        input: PathBuf,
    },
    /// Apply the exact reviewed execution-attribution plan digests.
    ExecutionAttributionApply {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
    },
    /// Inspect an execution-attribution request outcome before retrying.
    ExecutionAttributionOutcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },
    /// Record a versioned enabled-project logical backup manifest (owner only).
    BackupCreate {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
    },
    /// Inspect a recorded enabled-project backup manifest.
    BackupInspect {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        backup_id: String,
    },
    /// Preview guarded fencing restore against a backup (no writes).
    BackupRestorePreview {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        backup_id: String,
    },
    /// Apply verified fencing restore using exact preview digests.
    BackupRestoreApply {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        backup_id: String,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
    },
    /// Inspect a backup restore-apply request outcome before retrying.
    BackupRestoreOutcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },
    /// Preview bounded logical rebuild from a backup manifest (owner only; no writes).
    BackupRebuildPreview {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        backup_id: String,
    },
    /// Apply digest-gated ownership/work-inventory rebuild using exact preview digests.
    BackupRebuildApply {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        backup_id: String,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_state: String,
        #[arg(long)]
        expected_plan: String,
    },
    /// Inspect a backup rebuild-apply request outcome before retrying.
    BackupRebuildOutcome {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_id: String,
    },

    /// Inspect one agent authorization (owner connection; no second admin plane).
    AuthorizationInspect {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        authorization_id: String,
    },
    /// List agent authorizations for a project, optionally filtered.
    AuthorizationList {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        responsible_person_id: Option<String>,
        #[arg(long)]
        subject_id: Option<String>,
        #[arg(long, default_value_t = true)]
        active_only: bool,
    },
    /// Revoke an agent authorization with an explicit person actor.
    AuthorizationRevoke {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        request_key: String,
        #[arg(long)]
        authorization_id: String,
        #[arg(long)]
        revoked_by: String,
        #[arg(long)]
        reason: String,
    },
    /// Explain claim eligibility factors without performing a claim.
    ClaimExplain {
        #[arg(long)]
        input: PathBuf,
    },
    /// Inspect a confirmed Team handoff and its duty projection (owner connection).
    HandoffInspect {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        handoff_id: String,
        #[arg(long, default_value_t = 0)]
        now_ms: i64,
    },
    /// List open handoff ids for a work item (owner connection).
    HandoffListOpen {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        work_id: String,
    },
}

pub type Error = (&'static str, &'static str);
fn pg_error(e: PgError) -> Error {
    match e {
        PgError::Forbidden => ("Forbidden", "schema-owner access or requested scope denied"),
        PgError::Protocol(_) => ("InvalidInput", "invalid access plan, identity or bounds"),
        PgError::PreconditionsChanged => (
            "PreconditionsChanged",
            "access changed or plan conflicts with current identity; preview again",
        ),
        PgError::IdempotencyConflict => (
            "IdempotencyConflict",
            "request ID is bound to a different intent",
        ),
        PgError::Unsupported(_) => (
            "Unsupported",
            "operation unsupported for this project state or refused by restore plan",
        ),
        PgError::RestoreIncomplete => (
            "RestoreIncomplete",
            "backup missing, inventory incomplete, or physical/logical artifacts not verified",
        ),
        PgError::ProjectNotAvailable => (
            "ProjectNotAvailable",
            "project is not active or not available for operator backup/restore",
        ),
        _ => (
            "Unavailable",
            "operator operation did not return a confirmed result; inspect its original request before retrying",
        ),
    }
}

fn agent_plan(path: &PathBuf) -> Result<AgentProvisionPlan, Error> {
    let file = std::fs::File::open(path).map_err(|_| ("InvalidInput", "cannot open Agent plan"))?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| ("InvalidInput", "cannot read Agent plan"))?;
    if bytes.len() > 65536 {
        return Err(("InvalidInput", "Agent plan exceeds 64 KiB"));
    }
    serde_json::from_slice(&bytes).map_err(|_| ("InvalidInput", "invalid Agent plan JSON"))
}

fn agent_renew_plan(path: &PathBuf) -> Result<AgentRenewPlan, Error> {
    let file = std::fs::File::open(path)
        .map_err(|_| ("InvalidInput", "cannot open Agent renewal plan"))?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| ("InvalidInput", "cannot read Agent renewal plan"))?;
    if bytes.len() > 65536 {
        return Err(("InvalidInput", "Agent renewal plan exceeds 64 KiB"));
    }
    serde_json::from_slice(&bytes).map_err(|_| ("InvalidInput", "invalid Agent renewal plan JSON"))
}

fn agent_authorization_issue_plan(path: &PathBuf) -> Result<AgentAuthorizationIssuePlan, Error> {
    let file = std::fs::File::open(path)
        .map_err(|_| ("InvalidInput", "cannot open Agent authorization plan"))?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| ("InvalidInput", "cannot read Agent authorization plan"))?;
    if bytes.len() > 65536 {
        return Err(("InvalidInput", "Agent authorization plan exceeds 64 KiB"));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ("InvalidInput", "invalid Agent authorization plan JSON"))
}

fn attribution_plan(path: &PathBuf) -> Result<ExecutionAttributionPlan, Error> {
    let file =
        std::fs::File::open(path).map_err(|_| ("InvalidInput", "cannot open attribution plan"))?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| ("InvalidInput", "cannot read attribution plan"))?;
    if bytes.len() > 65536 {
        return Err(("InvalidInput", "attribution plan exceeds 64 KiB"));
    }
    serde_json::from_slice(&bytes).map_err(|_| ("InvalidInput", "invalid attribution plan JSON"))
}

fn plan(path: &PathBuf) -> Result<AccessPlan, Error> {
    let file =
        std::fs::File::open(path).map_err(|_| ("InvalidInput", "cannot open access plan"))?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| ("InvalidInput", "cannot read access plan"))?;
    if bytes.len() > 65536 {
        return Err(("InvalidInput", "access plan exceeds 64 KiB"));
    }
    serde_json::from_slice(&bytes).map_err(|_| ("InvalidInput", "invalid access plan JSON"))
}

fn token(id: &str, output: &PathBuf) -> Result<Value, Error> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(("InvalidInput", "invalid credential ID"));
    }
    let mut random = [0u8; 32];
    getrandom::fill(&mut random)
        .map_err(|_| ("Unavailable", "operating-system randomness unavailable"))?;
    let secret = random
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let bearer = format!("awr1.{id}.{secret}");
    let hash = awr_team_pg::workstream_credential_hash(&bearer).map_err(pg_error)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(output).map_err(|_| {
        (
            "InvalidInput",
            "cannot create credential file; existing paths are never overwritten",
        )
    })?;
    file.write_all(format!("{bearer}\n").as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| {
            (
                "Unavailable",
                "credential file write failed; nothing was registered",
            )
        })?;
    Ok(json!({"credential_id":id,"secret_hash":hash,"credential_file":output,"registered":false}))
}

pub async fn run(command: AccessCommand) -> Result<Value, Error> {
    if let AccessCommand::Token {
        credential_id,
        output,
    } = &command
    {
        return token(credential_id, output);
    }
    let url = std::env::var("AWR_TEAM_DATABASE_URL").map_err(|_| {
        (
            "InvalidInput",
            "AWR_TEAM_DATABASE_URL is required for operator access",
        )
    })?;
    let mut client = awr_team_pg::connect(&url).await.map_err(pg_error)?;
    match command {
        AccessCommand::Token { .. } => unreachable!(),
        AccessCommand::AgentPreview { input } => {
            OperatorAgent::preview(&mut client, &agent_plan(&input)?).await
        }
        AccessCommand::AgentInspect { input } => {
            OperatorAgent::inspect(&mut client, &agent_plan(&input)?).await
        }
        AccessCommand::AgentApply {
            input,
            request_id,
            expected_state,
            expected_plan,
        } => {
            OperatorAgent::apply(
                &mut client,
                &agent_plan(&input)?,
                &request_id,
                &expected_state,
                &expected_plan,
            )
            .await
        }
        AccessCommand::AgentOutcome {
            tenant_id,
            project_id,
            request_id,
        } => OperatorAgent::outcome(&mut client, &tenant_id, &project_id, &request_id).await,
        AccessCommand::AgentAuthorizePreview { input } => {
            OperatorAgent::authorize_preview(&mut client, &agent_authorization_issue_plan(&input)?)
                .await
        }
        AccessCommand::AgentAuthorizeApply {
            input,
            request_id,
            expected_state,
            expected_plan,
        } => {
            OperatorAgent::authorize_apply(
                &mut client,
                &agent_authorization_issue_plan(&input)?,
                &request_id,
                &expected_state,
                &expected_plan,
            )
            .await
        }
        AccessCommand::AgentAuthorizeOutcome {
            tenant_id,
            project_id,
            request_id,
        } => {
            OperatorAgent::authorize_outcome(&mut client, &tenant_id, &project_id, &request_id)
                .await
        }
        AccessCommand::AgentRenewPreview { input } => {
            OperatorAgent::renew_preview(&mut client, &agent_renew_plan(&input)?).await
        }
        AccessCommand::AgentRenewApply {
            input,
            request_id,
            expected_state,
            expected_plan,
        } => {
            OperatorAgent::renew_apply(
                &mut client,
                &agent_renew_plan(&input)?,
                &request_id,
                &expected_state,
                &expected_plan,
            )
            .await
        }
        AccessCommand::AgentRenewOutcome {
            tenant_id,
            project_id,
            request_id,
        } => OperatorAgent::renew_outcome(&mut client, &tenant_id, &project_id, &request_id).await,

        AccessCommand::Inspect {
            tenant_id,
            project_id,
            actor_id,
            client_id,
        } => {
            OperatorAccess::inspect(&mut client, &tenant_id, &project_id, &actor_id, &client_id)
                .await
        }
        AccessCommand::Preview { input } => {
            OperatorAccess::preview(&mut client, &plan(&input)?).await
        }
        AccessCommand::Apply {
            input,
            request_id,
            expected_state,
            expected_plan,
        } => {
            OperatorAccess::apply(
                &mut client,
                &plan(&input)?,
                &request_id,
                &expected_state,
                &expected_plan,
            )
            .await
        }
        AccessCommand::Outcome {
            tenant_id,
            project_id,
            request_id,
        } => OperatorAccess::outcome(&mut client, &tenant_id, &project_id, &request_id).await,
        AccessCommand::RecoveryInspect {
            tenant_id,
            project_id,
        } => OperatorRecovery::inspect(&mut client, &tenant_id, &project_id).await,
        AccessCommand::HistoryPreview {
            tenant_id,
            project_id,
        } => OperatorHistory::preview(&mut client, &tenant_id, &project_id).await,
        AccessCommand::HistoryApply {
            tenant_id,
            project_id,
            request_id,
            expected_state,
            expected_plan,
        } => {
            OperatorHistory::apply(
                &mut client,
                &tenant_id,
                &project_id,
                &request_id,
                &expected_state,
                &expected_plan,
            )
            .await
        }
        AccessCommand::HistoryOutcome {
            tenant_id,
            project_id,
            request_id,
        } => OperatorHistory::outcome(&mut client, &tenant_id, &project_id, &request_id).await,
        AccessCommand::QuarantinePreview {
            tenant_id,
            project_id,
            claim_disposition,
        } => {
            OperatorQuarantine::preview(&mut client, &tenant_id, &project_id, &claim_disposition)
                .await
        }
        AccessCommand::QuarantineApply {
            tenant_id,
            project_id,
            request_id,
            expected_state,
            expected_plan,
            claim_disposition,
        } => {
            OperatorQuarantine::apply(
                &mut client,
                &tenant_id,
                &project_id,
                &request_id,
                &expected_state,
                &expected_plan,
                &claim_disposition,
            )
            .await
        }
        AccessCommand::QuarantineOutcome {
            tenant_id,
            project_id,
            request_id,
        } => OperatorQuarantine::outcome(&mut client, &tenant_id, &project_id, &request_id).await,

        AccessCommand::ExecutionAttributionPreview { input } => {
            OperatorExecutionAttribution::preview(&mut client, &attribution_plan(&input)?).await
        }
        AccessCommand::ExecutionAttributionApply {
            input,
            request_id,
            expected_state,
            expected_plan,
        } => {
            OperatorExecutionAttribution::apply(
                &mut client,
                &attribution_plan(&input)?,
                &request_id,
                &expected_state,
                &expected_plan,
            )
            .await
        }
        AccessCommand::ExecutionAttributionOutcome {
            tenant_id,
            project_id,
            request_id,
        } => {
            OperatorExecutionAttribution::outcome(&mut client, &tenant_id, &project_id, &request_id)
                .await
        }
        AccessCommand::BackupCreate {
            tenant_id,
            project_id,
        } => OperatorBackup::backup(&mut client, &tenant_id, &project_id).await,
        AccessCommand::BackupInspect {
            tenant_id,
            project_id,
            backup_id,
        } => OperatorBackup::inspect(&mut client, &tenant_id, &project_id, &backup_id).await,
        AccessCommand::BackupRestorePreview {
            tenant_id,
            project_id,
            backup_id,
        } => {
            OperatorBackup::restore_preview(&mut client, &tenant_id, &project_id, &backup_id).await
        }
        AccessCommand::BackupRestoreApply {
            tenant_id,
            project_id,
            backup_id,
            request_id,
            expected_state,
            expected_plan,
        } => {
            OperatorBackup::restore_apply(
                &mut client,
                &tenant_id,
                &project_id,
                &backup_id,
                &request_id,
                &expected_state,
                &expected_plan,
            )
            .await
        }
        AccessCommand::BackupRestoreOutcome {
            tenant_id,
            project_id,
            request_id,
        } => {
            OperatorBackup::restore_outcome(&mut client, &tenant_id, &project_id, &request_id).await
        }
        AccessCommand::BackupRebuildPreview {
            tenant_id,
            project_id,
            backup_id,
        } => {
            OperatorBackup::rebuild_preview(&mut client, &tenant_id, &project_id, &backup_id).await
        }
        AccessCommand::BackupRebuildApply {
            tenant_id,
            project_id,
            backup_id,
            request_id,
            expected_state,
            expected_plan,
        } => {
            OperatorBackup::rebuild_apply(
                &mut client,
                &tenant_id,
                &project_id,
                &backup_id,
                &request_id,
                &expected_state,
                &expected_plan,
            )
            .await
        }
        AccessCommand::BackupRebuildOutcome {
            tenant_id,
            project_id,
            request_id,
        } => {
            OperatorBackup::rebuild_outcome(&mut client, &tenant_id, &project_id, &request_id).await
        }
        AccessCommand::AuthorizationInspect {
            tenant_id,
            project_id,
            authorization_id,
        } => {
            let auth = awr_team_pg::AuthorizationStore::new(
                std::env::var("AWR_TEAM_DATABASE_URL").map_err(|_| {
                    (
                        "Unavailable",
                        "AWR_TEAM_DATABASE_URL is required for operator access",
                    )
                })?,
            )
            .get(&tenant_id, &project_id, &authorization_id)
            .await
            .map_err(pg_error)?;
            Ok(json!({"authorization": auth}))
        }
        AccessCommand::AuthorizationList {
            tenant_id,
            project_id,
            responsible_person_id,
            subject_id,
            active_only,
        } => {
            let person = match responsible_person_id {
                Some(id) => Some(
                    awr_core::PersonId::new(id)
                        .map_err(|_| ("InvalidInput", "invalid responsible_person_id"))?,
                ),
                None => None,
            };
            let list = awr_team_pg::AuthorizationStore::new(
                std::env::var("AWR_TEAM_DATABASE_URL").map_err(|_| {
                    (
                        "Unavailable",
                        "AWR_TEAM_DATABASE_URL is required for operator access",
                    )
                })?,
            )
            .list(
                &tenant_id,
                &project_id,
                person.as_ref(),
                subject_id.as_deref(),
                active_only,
            )
            .await
            .map_err(pg_error)?;
            Ok(json!({"authorizations": list}))
        }
        AccessCommand::AuthorizationRevoke {
            tenant_id,
            project_id,
            request_key,
            authorization_id,
            revoked_by,
            reason,
        } => {
            let revoked_by = awr_core::PersonId::new(revoked_by)
                .map_err(|_| ("InvalidInput", "invalid revoked_by"))?;
            let now = awr_core::now_millis().map_err(|_| ("Unavailable", "clock unavailable"))?;
            let req = awr_core::RevokeAuthorizationRequest {
                request_key,
                authorization_id,
                revoked_by,
                revoked_at_ms: now,
                reason,
            };
            let (auth, receipt) = awr_team_pg::AuthorizationStore::new(
                std::env::var("AWR_TEAM_DATABASE_URL").map_err(|_| {
                    (
                        "Unavailable",
                        "AWR_TEAM_DATABASE_URL is required for operator access",
                    )
                })?,
            )
            .revoke(&tenant_id, &project_id, &req)
            .await
            .map_err(pg_error)?;
            Ok(json!({"authorization": auth, "receipt": receipt}))
        }
        AccessCommand::ClaimExplain { input } => {
            let file = std::fs::File::open(&input)
                .map_err(|_| ("InvalidInput", "cannot open claim explain input"))?;
            let mut bytes = Vec::new();
            file.take(65537)
                .read_to_end(&mut bytes)
                .map_err(|_| ("InvalidInput", "cannot read claim explain input"))?;
            if bytes.len() > 65536 {
                return Err(("InvalidInput", "claim explain input exceeds 64 KiB"));
            }
            let doc: ClaimExplainDocument = serde_json::from_slice(&bytes)
                .map_err(|_| ("InvalidInput", "invalid claim explain JSON"))?;
            let explanation =
                match awr_core::explain_claim_eligibility(&awr_core::ClaimEvaluationInput {
                    project_id: &doc.project_id,
                    work_item_id: &doc.work_item_id,
                    task_workstream_id: doc.task_workstream_id.as_deref(),
                    candidate_person: &doc.candidate_person,
                    authorization: doc.authorization.as_ref(),
                    now_ms: doc.now_ms,
                    is_project_member: doc.is_project_member,
                    membership_version: doc.membership_version,
                    assignment_policy: &doc.assignment_policy,
                    assignment_policy_allows: doc.assignment_policy_allows,
                    required_resources: &doc.required_resources,
                    available_resource_ids: &doc.available_resource_ids,
                    required_host_capabilities: &doc.required_host_capabilities,
                    verified_host_capabilities: &doc.verified_host_capabilities,
                    host_id: &doc.host_id,
                    dependencies_satisfied: doc.dependencies_satisfied,
                    task: &doc.task,
                    requested_executor: &doc.requested_executor,
                }) {
                    Ok(value) => value,
                    Err(_) => {
                        return Err(("InvalidInput", "claim explain input is not admissible"));
                    }
                };
            match serde_json::to_value(explanation) {
                Ok(value) => Ok(value),
                Err(_) => return Err(("Unavailable", "cannot encode claim explanation")),
            }
        }
        AccessCommand::HandoffInspect {
            tenant_id,
            project_id,
            handoff_id,
            now_ms,
        } => {
            let store = awr_team_pg::HandoffStore::new(
                std::env::var("AWR_TEAM_DATABASE_URL").map_err(|_| {
                    (
                        "Unavailable",
                        "AWR_TEAM_DATABASE_URL is required for operator access",
                    )
                })?,
            );
            let handoff = store
                .get(&tenant_id, &project_id, &handoff_id)
                .await
                .map_err(pg_error)?;
            let duty = match &handoff {
                Some(h) => Some(
                    h.duty_at(if now_ms == 0 { h.updated_at_ms } else { now_ms })
                        .map_err(|e| pg_error(awr_team_pg::PgError::Protocol(e.to_string())))?,
                ),
                None => None,
            };
            Ok(json!({"handoff": handoff, "duty": duty, "timeout_does_not_stop_execution": true}))
        }
        AccessCommand::HandoffListOpen {
            tenant_id,
            project_id,
            work_id,
        } => {
            // Owner connection: list open handoff ids for a work item.
            let rows = client
                .query(
                    "SELECT id, status, kind, version FROM awr_team.team_handoffs
                     WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                       AND status IN ('proposed','inspected')
                     ORDER BY id",
                    &[&tenant_id, &project_id, &work_id],
                )
                .await
                .map_err(|e| pg_error(awr_team_pg::PgError::Db(e)))?;
            let items: Vec<_> = rows
                .iter()
                .map(|r| {
                    json!({
                        "handoff_id": r.get::<_, String>(0),
                        "status": r.get::<_, String>(1),
                        "kind": r.get::<_, String>(2),
                        "version": r.get::<_, i64>(3).to_string(),
                    })
                })
                .collect();
            Ok(json!({"work_id": work_id, "open_handoffs": items}))
        }
    }
    .map_err(pg_error)
}

#[cfg(test)]
mod agent_cli_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn owner_agent_commands_require_reviewed_input_and_apply_digests() {
        for op in [
            "agent-preview",
            "agent-inspect",
            "agent-authorize-preview",
            "agent-renew-preview",
        ] {
            assert!(
                crate::Args::try_parse_from(["awr-server", "access", op, "--input", "plan.json"])
                    .is_ok()
            );
            assert!(crate::Args::try_parse_from(["awr-server", "access", op]).is_err());
        }
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-apply",
                "--input",
                "plan.json"
            ])
            .is_err()
        );
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-apply",
                "--input",
                "plan.json",
                "--request-id",
                "issue",
                "--expected-state",
                "state",
                "--expected-plan",
                "plan"
            ])
            .is_ok()
        );
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-authorize-apply",
                "--input",
                "authorize.json"
            ])
            .is_err()
        );
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-authorize-apply",
                "--input",
                "authorize.json",
                "--request-id",
                "authorize",
                "--expected-state",
                "state",
                "--expected-plan",
                "plan"
            ])
            .is_ok()
        );
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-authorize-outcome",
                "--tenant-id",
                "tenant",
                "--project-id",
                "project",
                "--request-id",
                "authorize"
            ])
            .is_ok()
        );
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-renew-apply",
                "--input",
                "renew.json"
            ])
            .is_err()
        );
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-renew-apply",
                "--input",
                "renew.json",
                "--request-id",
                "renew",
                "--expected-state",
                "state",
                "--expected-plan",
                "plan"
            ])
            .is_ok()
        );
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-renew-outcome",
                "--tenant-id",
                "tenant",
                "--project-id",
                "project",
                "--request-id",
                "renew"
            ])
            .is_ok()
        );
        assert!(
            crate::Args::try_parse_from([
                "awr-server",
                "access",
                "agent-outcome",
                "--tenant-id",
                "tenant",
                "--project-id",
                "project",
                "--request-id",
                "issue"
            ])
            .is_ok()
        );
    }

    #[test]
    fn agent_plan_loader_bounds_input_and_does_not_echo_it() {
        let path = std::env::temp_dir().join(format!(
            "awr-agent-plan-{}-{}.json",
            std::process::id(),
            awr_core::now_millis().unwrap()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&vec![b'x'; 65537]).unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert_eq!(
            agent_plan(&path).unwrap_err(),
            ("InvalidInput", "Agent plan exceeds 64 KiB")
        );
        std::fs::remove_file(path).unwrap();
    }
}
