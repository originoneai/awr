//! Organization grouping stays project-qualified and separate from access policy.
use crate::workstream_auth::{ReaderAuthority, authenticate, authenticate_writer};
use crate::{PgError, PgPool, PgResult};
use awr_core::WorkstreamAction;
use awr_team::{Action, ProjectDirectoryPlan};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_postgres::Transaction;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryApplyRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub expected_version: String,
    pub expected_plan_digest: String,
    pub plan: ProjectDirectoryPlan,
}

pub struct DirectoryStore {
    pool: Arc<PgPool>,
}

fn invalid() -> PgError {
    PgError::Protocol("invalid project directory request".into())
}

fn version(value: &str) -> PgResult<i64> {
    let parsed = value.parse::<i64>().map_err(|_| invalid())?;
    if parsed < 0 || parsed.to_string() != value {
        return Err(invalid());
    }
    Ok(parsed)
}

async fn authorize(
    tx: &Transaction<'_>,
    auth: &mut ReaderAuthority,
    project: &str,
    manage: bool,
) -> PgResult<()> {
    let action = if manage {
        Action::DirectoryManageProject
    } else {
        Action::WorkRead
    };
    crate::delegation_auth::authorize_project_action(tx, auth, project, action).await?;
    if auth.catalog.workstreams.is_empty() {
        return Err(PgError::Forbidden);
    }
    // Directory management has its own business action. The shared access.manage
    // projection intentionally narrows generic Manage to member administrators.
    let management_grants = if manage {
        tx.query(
            "SELECT workstream_id,authority_version FROM awr_team.workstream_grants
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4
               AND active AND can_read AND can_manage ORDER BY workstream_id FOR SHARE",
            &[&auth.tenant_id, &project, &auth.actor_id, &auth.client_id],
        )
        .await?
        .iter()
        .map(|row| (row.get::<_, String>(0), row.get::<_, i64>(1)))
        .collect()
    } else {
        std::collections::BTreeMap::new()
    };
    for stream in &auth.catalog.workstreams {
        auth.access
            .authorize(&auth.catalog, stream.id, WorkstreamAction::Read)
            .map_err(|_| PgError::Forbidden)?;
        let expected = i64::try_from(stream.authority_version).map_err(|_| PgError::Forbidden)?;
        if manage && management_grants.get(&stream.id.to_string()) != Some(&expected) {
            return Err(PgError::Forbidden);
        }
    }
    Ok(())
}

async fn snapshot(tx: &Transaction<'_>, tenant: &str, project: &str) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT version,plan_json,provenance,(extract(epoch FROM updated_at)*1000)::bigint
         FROM awr_team.project_directories WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?;
    let Some(row) = row else {
        return Ok(
            json!({"project_id":project,"version":"0","plan":ProjectDirectoryPlan::default(),
            "provenance":null,"updated_at_ms":null,"configured":false}),
        );
    };
    Ok(
        json!({"project_id":project,"version":row.get::<_,i64>(0).to_string(),
        "plan":row.get::<_,Value>(1),"provenance":row.get::<_,Value>(2),
        "updated_at_ms":row.get::<_,i64>(3),"configured":true}),
    )
}

async fn validate_people(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    plan: &ProjectDirectoryPlan,
) -> PgResult<()> {
    let people: Vec<_> = plan
        .members
        .iter()
        .map(|member| member.person_id.as_str())
        .collect();
    let rows = tx
        .query(
            "SELECT id FROM awr_team.persons WHERE tenant_id=$1 AND project_id=$2
         AND id=ANY($3) AND status='active' ORDER BY id FOR SHARE",
            &[&tenant, &project, &people],
        )
        .await?;
    if rows.len() != people.len() {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}

impl DirectoryStore {
    pub(crate) fn from_pool(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    pub async fn read(&self, tenant: &str, project: &str, bearer: &str) -> PgResult<Value> {
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = authenticate(&tx, tenant, project, bearer).await?;
        authorize(&tx, &mut auth, project, false).await?;
        let mut result = snapshot(&tx, tenant, project).await?;
        let plan: ProjectDirectoryPlan =
            serde_json::from_value(result["plan"].clone()).map_err(|_| invalid())?;
        let people: Vec<_> = plan
            .members
            .iter()
            .map(|member| member.person_id.as_str())
            .collect();
        let rows = tx.query(
            "SELECT id,display_name,status FROM awr_team.persons WHERE tenant_id=$1 AND project_id=$2 AND id=ANY($3)",
            &[&tenant,&project,&people],
        ).await?;
        let names: std::collections::BTreeMap<_, _> = rows
            .iter()
            .map(|row| {
                (
                    row.get::<_, String>(0),
                    (row.get::<_, String>(1), row.get::<_, String>(2)),
                )
            })
            .collect();
        result["members"] = json!(
            plan.members
                .iter()
                .map(|member| {
                    let current = names
                        .get(&member.person_id)
                        .filter(|(_, status)| status == "active");
                    json!({"member_ref":{"project_id":project,"person_id":member.person_id},
                "display_name":current.map(|(name,_)| name),"available":current.is_some(),
                "department_ids":member.department_ids})
                })
                .collect::<Vec<_>>()
        );
        result["as_of_project_revision"] = json!(auth.revision.to_string());
        result["cross_project_person_merging"] = json!(false);
        result["access_grants_changed"] = json!(false);
        tx.commit().await?;
        Ok(result)
    }

    pub async fn preview(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        plan: &ProjectDirectoryPlan,
    ) -> PgResult<Value> {
        let plan = plan.normalized().map_err(|_| invalid())?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = authenticate(&tx, tenant, project, bearer).await?;
        authorize(&tx, &mut auth, project, true).await?;
        validate_people(&tx, tenant, project, &plan).await?;
        let before = snapshot(&tx, tenant, project).await?;
        let digest = plan.digest().map_err(|_| invalid())?;
        tx.commit().await?;
        Ok(
            json!({"project_id":project,"expected_version":before["version"],
            "plan_digest":digest,"before":before["plan"],"after":plan,
            "access_grants_changed":false,"task_responsibility_changed":false}),
        )
    }

    pub async fn apply(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: &DirectoryApplyRequest,
    ) -> PgResult<Value> {
        crate::retry_rolled_back(|| self.apply_once(tenant, project, bearer, request)).await
    }

    async fn apply_once(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: &DirectoryApplyRequest,
    ) -> PgResult<Value> {
        if request.protocol_version != 1 || !awr_team::directory_id(&request.request_id) {
            return Err(invalid());
        }
        let expected = version(&request.expected_version)?;
        let plan = request.plan.normalized().map_err(|_| invalid())?;
        let digest = plan.digest().map_err(|_| invalid())?;
        if digest != request.expected_plan_digest {
            return Err(PgError::PreconditionsChanged);
        }
        let intent = awr_team::request_hash(&json!({"expected_version":request.expected_version,
            "plan_digest":digest,"plan":plan}))
        .map_err(|_| invalid())?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate_writer(&tx, tenant, project, bearer).await?;
        authorize(&tx, &mut auth, project, true).await?;
        if auth.project_status != "active" {
            return Err(PgError::PreconditionsChanged);
        }
        let prior = tx.query_opt(
            "SELECT request_hash,result_json FROM awr_team.project_directory_changes
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND request_id=$5",
            &[&tenant,&project,&auth.actor_id,&auth.client_id,&request.request_id],
        ).await?;
        if let Some(row) = prior {
            if row.get::<_, String>(0) != intent {
                return Err(PgError::IdempotencyConflict);
            }
            let receipt: Value = row.get(1);
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":receipt}));
        }
        let before = snapshot(&tx, tenant, project).await?;
        if before["version"] != request.expected_version {
            return Err(PgError::PreconditionsChanged);
        }
        let next = expected
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        validate_people(&tx, tenant, project, &plan).await?;
        let provenance = json!({"actor_id":auth.actor_id,"client_id":auth.client_id,
            "delegation_id":auth.delegation_id,"membership_version":auth.membership_version.to_string(),
            "authorization_binding":auth.binding,"source_snapshot_id":auth.snapshot,
            "grant_versions":auth.grant_versions});
        let value = serde_json::to_value(&plan).map_err(|_| invalid())?;
        tx.execute(
            "INSERT INTO awr_team.project_directories(tenant_id,project_id,version,plan_json,provenance)
             VALUES($1,$2,$3,$4,$5) ON CONFLICT(tenant_id,project_id) DO UPDATE
             SET version=EXCLUDED.version,plan_json=EXCLUDED.plan_json,provenance=EXCLUDED.provenance,updated_at=clock_timestamp()",
            &[&tenant,&project,&next,&value,&provenance],
        ).await?;
        let row = tx.query_opt(
            "UPDATE awr_team.projects SET project_revision=project_revision+1
             WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807 RETURNING project_revision",
            &[&tenant,&project],
        ).await?.ok_or(PgError::PreconditionsChanged)?;
        let revision: i64 = row.get(0);
        let receipt = json!({"protocol":"awr-project-directory-v1","receipt_id":crate::tx::new_id(),
            "request_id":request.request_id,"project_id":project,"version":next.to_string(),
            "previous_version":request.expected_version,"plan_digest":digest,"plan":plan,
            "provenance":provenance,"project_revision":revision.to_string(),"state_basis":"at_commit",
            "access_grants_changed":false,"task_responsibility_changed":false});
        tx.execute(
            "INSERT INTO awr_team.project_directory_changes(tenant_id,project_id,actor_id,client_id,request_id,request_hash,result_json)
             VALUES($1,$2,$3,$4,$5,$6,$7)",
            &[&tenant,&project,&auth.actor_id,&auth.client_id,&request.request_id,&intent,&receipt],
        ).await?;
        let mut audit = crate::ops_audit::write_from_auth(
            &auth,
            crate::OpsCategory::Access,
            "directory.manage_project",
            "directory",
        );
        audit.target_id = Some(project.into());
        audit.request_id = Some(request.request_id.clone());
        audit.digest = Some(digest.clone());
        audit.summary = json!({"plan_digest":digest,"directory_version":next.to_string(),
            "department_count":plan.departments.len(),"member_count":plan.members.len(),
            "department_depth":plan.department_depth().map_err(|_| invalid())?,
            "leaf_department_count":plan.leaf_department_count()});
        crate::ops_audit::record_in_tx(&tx, tenant, project, &audit).await?;
        tx.commit().await?;
        Ok(json!({"replayed":false,"receipt":receipt}))
    }

    pub async fn outcome(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request_id: &str,
    ) -> PgResult<Value> {
        if !awr_team::directory_id(request_id) {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant, project, bearer).await?;
        authorize(&tx, &mut auth, project, true).await?;
        let row = tx.query_opt(
            "SELECT result_json FROM awr_team.project_directory_changes
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND request_id=$5",
            &[&tenant,&project,&auth.actor_id,&auth.client_id,&request_id],
        ).await?;
        let receipt = row.map(|row| row.get::<_, Value>(0));
        tx.commit().await?;
        Ok(json!({"outcome":if receipt.is_some() {"recorded"} else {"absent"},"receipt":receipt}))
    }
}
