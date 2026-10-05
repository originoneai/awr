//! Access provisioning: schema-owner CLI bootstrap plus project-admin MCP (TMCP-012).
//! Owner paths never reach client HTTP/MCP. Admin MCP uses the app role and
//! `access.manage_project`. No raw bearer enters a plan, audit event, receipt,
//! ordinary MCP message or database row — only `secret_hash`.
use crate::pool::PgPool;
use crate::workstream_auth::{authenticate, authorize_domain_action};
use crate::{PgError, PgResult};
use awr_core::{Id, WorkstreamAction, WorkstreamCatalog};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio_postgres::{Client, Transaction};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessActor {
    pub id: String,
    pub kind: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessGrant {
    pub workstream_id: Id,
    pub authority_version: String,
    pub read: bool,
    pub write: bool,
    pub manage: bool,
    pub attest_execution: bool,
    pub reconcile_execution: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessCredential {
    pub id: String,
    pub secret_hash: String,
    pub expires_at_unix_ms: Option<i64>,
}

/// Exact replacement of one actor/client's project grants. Other clients' grants
/// are retained. Membership is actor/project-wide; credentials are tenant-wide.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPlan {
    pub protocol_version: u32,
    pub tenant_id: String,
    pub project_id: String,
    pub actor: AccessActor,
    pub client_id: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub business_roles: Option<BTreeSet<awr_team::BusinessRole>>,
    pub grants: Vec<AccessGrant>,
    pub credential: Option<AccessCredential>,
    pub revoke_credentials: Vec<String>,
    #[serde(default)]
    pub independent_review: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub agent_review: bool,
}

fn invalid() -> PgError {
    PgError::Protocol("invalid operator access plan".into())
}
fn identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn credential_id(s: &str) -> bool {
    identity(s)
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn version(s: &str) -> PgResult<i64> {
    if s.starts_with('0') || !s.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    s.parse::<i64>().ok().filter(|n| *n > 0).ok_or_else(invalid)
}
fn hash(v: &Value) -> PgResult<String> {
    awr_team::request_hash(v).map_err(|_| invalid())
}
impl AccessPlan {
    fn validate(&self) -> PgResult<()> {
        if let Some(roles) = &self.business_roles {
            awr_team::validate_business_roles(roles).map_err(|_| invalid())?;
        }
        if self.protocol_version != 1
            || ![
                &self.tenant_id,
                &self.project_id,
                &self.actor.id,
                &self.client_id,
            ]
            .iter()
            .all(|s| identity(s))
            || !matches!(self.actor.kind.as_str(), "human" | "agent" | "system")
            || (self.agent_review && self.actor.kind != "agent")
            || self.actor.display_name.trim().is_empty()
            || self.actor.display_name.len() > 512
            || self.actor.display_name.chars().any(char::is_control)
            || !matches!(
                self.role.as_str(),
                "admin"
                    | "worker"
                    | "reviewer"
                    | "reader"
                    | "project_admin"
                    | "developer"
                    | "maintainer"
            )
            || self.grants.len() > 256
            || self.revoke_credentials.len() > 256
            || serde_json::to_vec(self).map_err(|_| invalid())?.len() > 65536
        {
            return Err(invalid());
        }
        let mut seen = BTreeSet::new();
        for g in &self.grants {
            version(&g.authority_version)?;
            if !seen.insert(g.workstream_id)
                || !g.read
                || (g.write && self.role == "reader")
                || (g.manage && !matches!(self.role.as_str(), "admin" | "project_admin"))
                || (g.attest_execution && (!g.write || self.actor.kind != "system"))
                || (g.reconcile_execution && (!g.write || !g.manage || self.actor.kind == "agent"))
            {
                return Err(invalid());
            }
        }
        let mut seen = BTreeSet::new();
        if self
            .revoke_credentials
            .iter()
            .any(|id| !credential_id(id) || !seen.insert(id))
        {
            return Err(invalid());
        }
        if let Some(c) = &self.credential {
            if !credential_id(&c.id)
                || !c.secret_hash.strip_prefix("sha256:").is_some_and(hex)
                || c.expires_at_unix_ms
                    .is_some_and(|t| !(1..=253402300799999).contains(&t))
                || self.revoke_credentials.contains(&c.id)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

pub struct OperatorAccess;
impl OperatorAccess {
    pub async fn inspect(
        client: &mut Client,
        tenant: &str,
        project: &str,
        actor: &str,
        caller: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, actor, caller].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        require_owner_project(&tx, tenant, project, false).await?;
        let state = snapshot(&tx, tenant, project, actor, caller).await?;
        let result = json!({"state_digest":hash(&state)?,"state":state});
        tx.commit().await?;
        Ok(result)
    }

    pub async fn preview(client: &mut Client, plan: &AccessPlan) -> PgResult<Value> {
        plan.validate()?;
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        require_owner_project(&tx, &plan.tenant_id, &plan.project_id, false).await?;
        let state = snapshot(
            &tx,
            &plan.tenant_id,
            &plan.project_id,
            &plan.actor.id,
            &plan.client_id,
        )
        .await?;
        validate_current(&tx, plan, &state).await?;
        let plan_digest = hash(&serde_json::to_value(plan).map_err(|_| invalid())?)?;
        let result = json!({"applied":false,"state_digest":hash(&state)?,"plan_digest":plan_digest,"current":state,
            "desired":public_plan(plan),"grant_semantics":"replace_selected_actor_client_project_grants",
            "membership_scope":"all_clients_of_actor_in_project","credential_revocation_scope":"all_projects_in_tenant_using_this_credential"});
        tx.commit().await?;
        Ok(result)
    }

    pub async fn outcome(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, request].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        require_owner_project(&tx, tenant, project, false).await?;
        let row = tx.query_opt("SELECT result_json FROM awr_team.access_changes WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&tenant,&project,&request]).await?;
        let result = match row {
            Some(r) => json!({"outcome":"committed","receipt":r.get::<_,Value>(0)}),
            None => json!({"outcome":"unknown"}),
        };
        tx.commit().await?;
        Ok(result)
    }

    pub async fn apply(
        client: &mut Client,
        plan: &AccessPlan,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
    ) -> PgResult<Value> {
        plan.validate()?;
        if !identity(request) || !hex(expected_state) || !hex(expected_plan) {
            return Err(invalid());
        }
        let plan_digest = hash(&serde_json::to_value(plan).map_err(|_| invalid())?)?;
        let intent_hash = hash(
            &json!({"protocol":"awr-operator-access-v1","plan":plan,"expected_state":expected_state,"expected_plan":expected_plan}),
        )?;
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, &plan.tenant_id, &plan.project_id, true).await?;
        if let Some(r) = tx.query_opt("SELECT request_hash,result_json FROM awr_team.access_changes WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&plan.tenant_id,&plan.project_id,&request]).await? {
            if r.get::<_,String>(0) != intent_hash { return Err(PgError::IdempotencyConflict); }
            let receipt: Value = r.get(1);
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":receipt}));
        }
        if plan_digest != expected_plan {
            return Err(PgError::PreconditionsChanged);
        }
        // One actor's credentials and membership can be shared by projects.
        // Serialize its operator changes before acquiring any shared row locks.
        tx.query_opt(
            "SELECT id FROM awr_team.actors WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
            &[&plan.tenant_id, &plan.actor.id],
        )
        .await?;
        let before = snapshot(
            &tx,
            &plan.tenant_id,
            &plan.project_id,
            &plan.actor.id,
            &plan.client_id,
        )
        .await?;
        if hash(&before)? != expected_state {
            return Err(PgError::PreconditionsChanged);
        }
        validate_current(&tx, plan, &before).await?;
        apply_policy(&tx, plan).await?;
        let after = snapshot(
            &tx,
            &plan.tenant_id,
            &plan.project_id,
            &plan.actor.id,
            &plan.client_id,
        )
        .await?;
        let revision: i64 = tx.query_opt("UPDATE awr_team.projects SET project_revision=project_revision+1 WHERE tenant_id=$1 AND id=$2
            AND project_revision<9223372036854775807 RETURNING project_revision", &[&plan.tenant_id,&plan.project_id]).await?
            .ok_or(PgError::PreconditionsChanged)?.get(0);
        let receipt = json!({"protocol":"awr-operator-access-v1","request_id":request,"request_hash":intent_hash,
            "operator_role":operator,"tenant_id":plan.tenant_id,"project_id":plan.project_id,
            "actor_id":plan.actor.id,"client_id":plan.client_id,"before_digest":expected_state,"after_digest":hash(&after)?,
            "plan_digest":plan_digest,"project_revision":revision.to_string(),"desired":public_plan(plan),
            "previous_policy":policy(&before),"current_policy":policy(&after),
            "state_basis":"at_commit","execution_authorized":false});
        tx.execute("INSERT INTO awr_team.access_changes(tenant_id,project_id,request_id,request_hash,operator_role,result_json)
            VALUES($1,$2,$3,$4,$5,$6)",&[&plan.tenant_id,&plan.project_id,&request,&intent_hash,&operator,&receipt]).await?;
        // No grant contents, credential hashes or raw tokens in client-visible events.
        let event = json!({"operator_role":operator,"actor_id":plan.actor.id,"client_id":plan.client_id,"request_id":request});
        tx.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
            VALUES($1,$2,$3,$4,0,'access.changed',$5,$6)",&[&plan.tenant_id,&plan.project_id,&crate::tx::new_id(),&revision,&plan.actor.id,&event]).await?;
        // TMCP-040: owner access apply also binds ops audit in-TX.
        {
            let summary = serde_json::json!({
                "plan_digest": plan_digest,
                "actor_id": plan.actor.id,
                "client_id": plan.client_id,
                "operator_role": operator,
            });
            let digest = crate::ops_audit::digest_of(&summary);
            let audit = crate::ops_audit::OpsAuditWrite {
                category: crate::ops_audit::OpsCategory::Access,
                action: "access.manage_project".into(),
                result: "committed",
                person_id: None,
                actor_id: plan.actor.id.clone(),
                client_id: plan.client_id.clone(),
                target_kind: "access_plan".into(),
                target_id: Some(plan.actor.id.clone()),
                work_id: None,
                change_id: None,
                request_id: Some(request.to_string()),
                membership_version: None,
                authority_version: None,
                policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
                source_version: None,
                digest: Some(digest),
                summary,
            };
            crate::ops_audit::record_in_tx(&tx, &plan.tenant_id, &plan.project_id, &audit).await?;
        }
        tx.commit().await?;
        Ok(json!({"replayed":false,"receipt":receipt}))
    }
}

pub(crate) async fn require_owner_project(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    write: bool,
) -> PgResult<String> {
    let role = tx
        .query_one(
            "SELECT current_user::text,pg_has_role(current_user,n.nspowner,'USAGE')
        FROM pg_namespace n WHERE n.nspname='awr_team'",
            &[],
        )
        .await?;
    if !role.get::<_, bool>(1) {
        return Err(PgError::Forbidden);
    }
    crate::tx::bind_workstream_scope(tx, tenant, project).await?;
    let mode = tx.query_opt("SELECT enabled FROM awr_team.workstream_modes WHERE tenant_id=$1 AND project_id=$2 FOR SHARE", &[&tenant,&project]).await?
        .ok_or(PgError::Forbidden)?;
    if !mode.get::<_, bool>(0) {
        return Err(PgError::Unsupported(
            "operator access requires enabled workstreams".into(),
        ));
    }
    let sql = if write {
        "SELECT id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2 FOR UPDATE"
    } else {
        "SELECT id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2 FOR SHARE"
    };
    tx.query_opt(sql, &[&tenant, &project])
        .await?
        .ok_or(PgError::Forbidden)?;
    Ok(role.get(0))
}

pub(crate) async fn snapshot(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    actor: &str,
    caller: &str,
) -> PgResult<Value> {
    let p=tx.query_one("SELECT active_snapshot_id,coordinator_epoch FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",&[&tenant,&project]).await?;
    let catalog=tx.query_one("SELECT catalog_json FROM awr_team.workstream_catalogs WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
        &[&tenant,&project,&p.get::<_,Option<String>>(0)]).await?.get::<_,Value>(0);
    let tenant_status: String = tx
        .query_one(
            "SELECT status FROM awr_team.tenants WHERE id=$1 FOR SHARE",
            &[&tenant],
        )
        .await?
        .get(0);
    let a=tx.query_opt("SELECT kind,display_name,status FROM awr_team.actors WHERE tenant_id=$1 AND id=$2 FOR SHARE",&[&tenant,&actor]).await?
        .map(|r|json!({"kind":r.get::<_,String>(0),"display_name":r.get::<_,String>(1),"status":r.get::<_,String>(2)}));
    let member=tx.query_opt("SELECT role,membership_version,independent_review,agent_review,business_roles FROM awr_team.project_memberships WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 FOR SHARE",
        &[&tenant,&project,&actor]).await?.map(|r| {
            let mut membership = json!({"role":r.get::<_,String>(0),"version":r.get::<_,i64>(1).to_string(),"independent_review":r.get::<_,bool>(2),"agent_review":r.get::<_,bool>(3)});
            if let Some(roles) = r.get::<_,Option<Value>>(4) {
                membership["business_roles"] = roles;
            }
            membership
        });
    let grants=tx.query("SELECT workstream_id,authority_version,can_read,can_write,can_manage,can_attest_execution,can_reconcile_execution,active,grant_version
        FROM awr_team.workstream_grants WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 ORDER BY workstream_id FOR SHARE",
        &[&tenant,&project,&actor,&caller]).await?.iter().map(|r|json!({"workstream_id":r.get::<_,String>(0),"authority_version":r.get::<_,i64>(1).to_string(),
        "read":r.get::<_,bool>(2),"write":r.get::<_,bool>(3),"manage":r.get::<_,bool>(4),"attest_execution":r.get::<_,bool>(5),"reconcile_execution":r.get::<_,bool>(6),
        "active":r.get::<_,bool>(7),"version":r.get::<_,i64>(8).to_string()})).collect::<Vec<_>>();
    let credentials=tx.query("SELECT id,(extract(epoch FROM expires_at)*1000)::bigint,(extract(epoch FROM revoked_at)*1000)::bigint,project_id
        FROM awr_team.credentials WHERE tenant_id=$1 AND actor_id=$2 AND client_id=$3 AND (project_id IS NULL OR project_id=$4) ORDER BY id FOR SHARE",&[&tenant,&actor,&caller,&project]).await?.iter()
        .map(|r|json!({"id":r.get::<_,String>(0),"expires_at_unix_ms":r.get::<_,Option<i64>>(1),"revoked_at_unix_ms":r.get::<_,Option<i64>>(2),"project_scoped":r.get::<_,Option<String>>(3).is_some()})).collect::<Vec<_>>();
    Ok(
        json!({"tenant_id":tenant,"project_id":project,"actor_id":actor,"client_id":caller,"tenant_status":tenant_status,
        "source_snapshot_id":p.get::<_,Option<String>>(0),"coordinator_epoch":p.get::<_,String>(1),"catalog":catalog,
        "actor":a,"membership":member,"grants":grants,"credentials":credentials}),
    )
}

async fn validate_current(tx: &Transaction<'_>, plan: &AccessPlan, state: &Value) -> PgResult<()> {
    if state["tenant_status"] != "active" {
        return Err(PgError::Forbidden);
    }
    let actor = &state["actor"];
    if !actor.is_null()
        && (actor["kind"] != plan.actor.kind
            || actor["display_name"] != plan.actor.display_name
            || actor["status"] != "active")
    {
        return Err(PgError::PreconditionsChanged);
    }
    let catalog: WorkstreamCatalog =
        serde_json::from_value(state["catalog"].clone()).map_err(|_| PgError::SourceDivergence)?;
    if catalog.project_id != plan.project_id {
        return Err(PgError::SourceDivergence);
    }
    for g in &plan.grants {
        if catalog.get(g.workstream_id)?.authority_version != version(&g.authority_version)? as u64
        {
            return Err(PgError::PreconditionsChanged);
        }
    }
    if let Some(c) = &plan.credential {
        let prior=tx.query_opt("SELECT actor_id,client_id,secret_hash,(extract(epoch FROM expires_at)*1000)::bigint,revoked_at IS NOT NULL
            FROM awr_team.credentials WHERE tenant_id=$1 AND id=$2 FOR SHARE",&[&plan.tenant_id,&c.id]).await?;
        if let Some(r) = prior {
            if r.get::<_, String>(0) != plan.actor.id
                || r.get::<_, String>(1) != plan.client_id
                || r.get::<_, String>(2) != c.secret_hash
                || r.get::<_, Option<i64>>(3) != c.expires_at_unix_ms
                || r.get::<_, bool>(4)
            {
                return Err(PgError::PreconditionsChanged);
            }
        }
        if let Some(expiry) = c.expires_at_unix_ms {
            let valid: bool = tx
                .query_one(
                    "SELECT to_timestamp($1::bigint::double precision/1000)>clock_timestamp()",
                    &[&expiry],
                )
                .await?
                .get(0);
            if !valid {
                return Err(PgError::PreconditionsChanged);
            }
        }
    }
    for id in &plan.revoke_credentials {
        if !state["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == *id)
        {
            return Err(PgError::Forbidden);
        }
    }
    Ok(())
}

fn public_plan(plan: &AccessPlan) -> Value {
    let mut v = serde_json::to_value(plan).expect("serializable access plan");
    if let Some(c) = v.get_mut("credential").and_then(Value::as_object_mut) {
        c.remove("secret_hash");
    }
    v
}

fn policy(state: &Value) -> Value {
    json!({"actor":state["actor"],"membership":state["membership"],
        "grants":state["grants"],"credentials":state["credentials"]})
}

async fn apply_policy(tx: &Transaction<'_>, p: &AccessPlan) -> PgResult<()> {
    tx.execute("INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES($1,$2,$3,$4,'active') ON CONFLICT DO NOTHING",
        &[&p.tenant_id,&p.actor.id,&p.actor.kind,&p.actor.display_name]).await?;
    let roles = p.business_roles.as_ref().map(|roles| json!(roles));
    tx.execute("INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role,independent_review,agent_review,business_roles) VALUES($1,$2,$3,$4,$5,$6,$7)
        ON CONFLICT(tenant_id,project_id,actor_id) DO UPDATE SET
            role=EXCLUDED.role,
            independent_review=EXCLUDED.independent_review,
            agent_review=EXCLUDED.agent_review,
            business_roles=COALESCE(EXCLUDED.business_roles,awr_team.project_memberships.business_roles),
            membership_version=awr_team.project_memberships.membership_version+1
        WHERE awr_team.project_memberships.role IS DISTINCT FROM EXCLUDED.role
           OR awr_team.project_memberships.independent_review IS DISTINCT FROM EXCLUDED.independent_review
           OR awr_team.project_memberships.agent_review IS DISTINCT FROM EXCLUDED.agent_review
           OR awr_team.project_memberships.business_roles IS DISTINCT FROM COALESCE(EXCLUDED.business_roles,awr_team.project_memberships.business_roles)",
        &[&p.tenant_id,&p.project_id,&p.actor.id,&p.role,&p.independent_review,&p.agent_review,&roles]).await?;
    let ids = p
        .grants
        .iter()
        .map(|g| g.workstream_id.to_string())
        .collect::<Vec<_>>();
    tx.execute("UPDATE awr_team.workstream_grants SET active=false,grant_version=grant_version+1
        WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND active AND NOT(workstream_id=ANY($5))",
        &[&p.tenant_id,&p.project_id,&p.actor.id,&p.client_id,&ids]).await?;
    for g in &p.grants {
        tx.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,
            can_read,can_write,can_manage,can_attest_execution,can_reconcile_execution) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            ON CONFLICT(tenant_id,project_id,actor_id,client_id,workstream_id) DO UPDATE SET authority_version=EXCLUDED.authority_version,
            can_read=EXCLUDED.can_read,can_write=EXCLUDED.can_write,can_manage=EXCLUDED.can_manage,can_attest_execution=EXCLUDED.can_attest_execution,
            can_reconcile_execution=EXCLUDED.can_reconcile_execution,active=true,grant_version=awr_team.workstream_grants.grant_version+1
            WHERE (awr_team.workstream_grants.authority_version,awr_team.workstream_grants.can_read,awr_team.workstream_grants.can_write,
                awr_team.workstream_grants.can_manage,awr_team.workstream_grants.can_attest_execution,awr_team.workstream_grants.can_reconcile_execution,awr_team.workstream_grants.active)
            IS DISTINCT FROM (EXCLUDED.authority_version,EXCLUDED.can_read,EXCLUDED.can_write,EXCLUDED.can_manage,EXCLUDED.can_attest_execution,EXCLUDED.can_reconcile_execution,true)",
            &[&p.tenant_id,&p.project_id,&p.actor.id,&p.client_id,&g.workstream_id.to_string(),&version(&g.authority_version)?,
              &g.read,&g.write,&g.manage,&g.attest_execution,&g.reconcile_execution]).await?;
    }
    if let Some(c) = &p.credential {
        tx.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash,expires_at)
            VALUES($1,$2,$3,$4,$5,to_timestamp($6::bigint::double precision/1000)) ON CONFLICT DO NOTHING",
            &[&p.tenant_id,&c.id,&p.actor.id,&p.client_id,&c.secret_hash,&c.expires_at_unix_ms]).await?;
    }
    for id in &p.revoke_credentials {
        tx.execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE tenant_id=$1 AND id=$2 AND actor_id=$3 AND client_id=$4 AND revoked_at IS NULL",
            &[&p.tenant_id,id,&p.actor.id,&p.client_id]).await?;
    }
    // Recheck concurrent absent-row insertions; never accept someone else's actor
    // identity or a credential collision through ON CONFLICT DO NOTHING.
    let after = snapshot(tx, &p.tenant_id, &p.project_id, &p.actor.id, &p.client_id).await?;
    validate_current(tx, p, &after).await?;
    Ok(())
}

/// Project-bounded member/role/credential plan for authorized project admins (TMCP-012).
/// Tenant/project are bound from the authenticated MCP/HTTP project, never from the body.
/// Raw secrets are refused; only `secret_hash` may be registered.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAccessPlan {
    pub protocol_version: u32,
    pub subject: AccessActor,
    pub subject_client_id: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub business_roles: Option<BTreeSet<awr_team::BusinessRole>>,
    pub grants: Vec<AccessGrant>,
    pub credential: Option<AccessCredential>,
    /// Explicit review.decide grant (TMCP-031). Never implied by role template.
    #[serde(default)]
    pub independent_review: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub agent_review: bool,
    #[serde(default)]
    pub remove_membership: bool,
    /// Tenant-wide credential revoke is owner-only. Project admins must clear
    /// project grants instead; non-empty values are rejected with Forbidden.
    #[serde(default)]
    pub revoke_tenant_credentials: Vec<String>,
    /// Restrict a newly registered credential to this project. Raw values remain
    /// in the caller's one-time delivery channel, never in this plan or receipt.
    #[serde(default, skip_serializing_if = "is_false")]
    pub credential_project_scoped: bool,
    /// Revoke only credentials explicitly scoped to this actor/client/project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoke_project_credentials: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl AdminAccessPlan {
    fn validate_admin_bounds(&self) -> PgResult<()> {
        if let Some(roles) = &self.business_roles {
            awr_team::validate_business_roles(roles).map_err(|_| invalid())?;
        }
        if self.protocol_version != 1
            || !identity(&self.subject.id)
            || !identity(&self.subject_client_id)
            || !matches!(self.subject.kind.as_str(), "human" | "agent" | "system")
            || (self.agent_review && self.subject.kind != "agent")
            || self.subject.display_name.trim().is_empty()
            || self.subject.display_name.len() > 512
            || self.subject.display_name.chars().any(char::is_control)
            || !matches!(
                self.role.as_str(),
                "admin"
                    | "worker"
                    | "reviewer"
                    | "reader"
                    | "project_admin"
                    | "developer"
                    | "maintainer"
            )
            || self.grants.len() > 256
            || self.revoke_tenant_credentials.len() > 256
            || self.revoke_project_credentials.len() > 256
            || self
                .revoke_project_credentials
                .iter()
                .any(|id| !credential_id(id))
            || self.credential_project_scoped && self.credential.is_none()
            || self
                .credential
                .as_ref()
                .is_some_and(|c| self.revoke_project_credentials.contains(&c.id))
            || serde_json::to_vec(self).map_err(|_| invalid())?.len() > 65536
        {
            return Err(invalid());
        }
        if !self.revoke_tenant_credentials.is_empty() {
            // Acceptance: tenant-level credential revoke requires matching
            // (owner/cross-project) permission; otherwise use project grant revoke.
            return Err(PgError::Forbidden);
        }
        if self.remove_membership && !self.grants.is_empty() {
            return Err(invalid());
        }
        if self.independent_review {
            let Some(template) = crate::workstream_auth::map_membership_role(&self.role) else {
                return Err(invalid());
            };
            if !awr_team::independent_review_eligible(template) {
                return Err(PgError::Forbidden);
            }
        }
        let mut seen = BTreeSet::new();
        for g in &self.grants {
            version(&g.authority_version)?;
            // Project admins cannot grant special executor/operator authorities.
            if !seen.insert(g.workstream_id)
                || !g.read
                || g.attest_execution
                || g.reconcile_execution
                || (g.write && matches!(self.role.as_str(), "reader"))
                || (g.manage && !matches!(self.role.as_str(), "admin" | "project_admin"))
            {
                return Err(invalid());
            }
        }
        if let Some(c) = &self.credential {
            if !credential_id(&c.id)
                || !c.secret_hash.strip_prefix("sha256:").is_some_and(hex)
                || c.expires_at_unix_ms
                    .is_some_and(|t| !(1..=253402300799999).contains(&t))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn as_owner_plan(&self, tenant: &str, project: &str) -> AccessPlan {
        AccessPlan {
            protocol_version: 1,
            tenant_id: tenant.into(),
            project_id: project.into(),
            actor: self.subject.clone(),
            client_id: self.subject_client_id.clone(),
            role: self.role.clone(),
            business_roles: self.business_roles.clone(),
            grants: self.grants.clone(),
            credential: self.credential.clone(),
            revoke_credentials: vec![],
            independent_review: self.independent_review,
            agent_review: self.agent_review,
        }
    }
}

fn public_admin_plan(plan: &AdminAccessPlan) -> Value {
    let mut v = serde_json::to_value(plan).expect("serializable admin access plan");
    if let Some(c) = v.get_mut("credential").and_then(Value::as_object_mut) {
        c.remove("secret_hash");
    }
    v
}

fn is_admin_role(role: &str) -> bool {
    matches!(role, "admin" | "project_admin")
}

/// Authenticated client grant ceiling for project-admin access changes (TMCP-012).
/// Membership `access.manage_project` alone is insufficient: every inspect/preview/
/// apply/outcome must also be covered by the caller's explicit workstream manage
/// grants, and requested grant bits must not exceed the caller's own bits.
fn caller_grant<'a>(
    auth: &'a crate::workstream_auth::ReaderAuthority,
    stream: Id,
) -> Option<&'a awr_core::WorkstreamGrant> {
    auth.access
        .grants
        .iter()
        .find(|grant| grant.workstream_id == stream)
}

fn require_project_manage_grant(auth: &crate::workstream_auth::ReaderAuthority) -> PgResult<()> {
    if !auth.access.grants.iter().any(|grant| grant.manage) {
        return Err(PgError::Forbidden);
    }
    Ok(())
}

fn require_manage_on_streams(
    auth: &crate::workstream_auth::ReaderAuthority,
    streams: &[Id],
) -> PgResult<()> {
    if streams.is_empty() {
        // No concrete streams in the delta still requires explicit manage somewhere.
        return require_project_manage_grant(auth);
    }
    for stream in streams {
        let caller = caller_grant(auth, *stream).ok_or(PgError::Forbidden)?;
        if !caller.manage {
            return Err(PgError::Forbidden);
        }
        auth.access
            .authorize(&auth.catalog, *stream, WorkstreamAction::Manage)
            .map_err(|_| PgError::Forbidden)?;
    }
    Ok(())
}

/// Authorize the full access delta: every stream being removed or replaced, plus
/// every desired grant bit. `apply` replaces the selected client's entire project
/// grant set, so empty `grants` / removals must not slip past a one-stream manage.
fn enforce_client_grant_ceiling(
    auth: &crate::workstream_auth::ReaderAuthority,
    plan: &AdminAccessPlan,
    current_streams: &[Id],
) -> PgResult<()> {
    for desired in &plan.grants {
        let caller = caller_grant(auth, desired.workstream_id).ok_or(PgError::Forbidden)?;
        if !caller.manage {
            return Err(PgError::Forbidden);
        }
        // Ceiling: cannot bootstrap bits beyond the authenticated client grant.
        if (desired.read && !caller.read)
            || (desired.write && !caller.write)
            || (desired.manage && !caller.manage)
        {
            return Err(PgError::Forbidden);
        }
    }
    let mut affected: BTreeSet<Id> = current_streams.iter().copied().collect();
    for desired in &plan.grants {
        affected.insert(desired.workstream_id);
    }
    let affected: Vec<Id> = affected.into_iter().collect();
    require_manage_on_streams(auth, &affected)
}

async fn actor_active_grant_streams(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    actor: &str,
) -> PgResult<Vec<Id>> {
    let rows = tx
        .query(
            "SELECT DISTINCT workstream_id FROM awr_team.workstream_grants
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND active
             ORDER BY workstream_id",
            &[&tenant, &project, &actor],
        )
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let raw: String = row.get(0);
        out.push(raw.parse::<Id>().map_err(|_| PgError::Forbidden)?);
    }
    Ok(out)
}

fn enforce_inspect_grant_ceiling(
    auth: &crate::workstream_auth::ReaderAuthority,
    subject_streams: &[Id],
) -> PgResult<()> {
    if subject_streams.is_empty() {
        return require_project_manage_grant(auth);
    }
    for stream in subject_streams {
        let caller = caller_grant(auth, *stream).ok_or(PgError::Forbidden)?;
        if !caller.manage {
            return Err(PgError::Forbidden);
        }
        auth.access
            .authorize(&auth.catalog, *stream, WorkstreamAction::Manage)
            .map_err(|_| PgError::Forbidden)?;
    }
    Ok(())
}

/// App-role store for project-admin MCP preview/apply/outcome (not schema-owner).
mod members;

pub struct ProjectAccessStore {
    pool: Arc<PgPool>,
}

impl ProjectAccessStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: Arc::new(PgPool::new(url)),
        }
    }
    pub fn from_pool(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }
    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: Arc::new(PgPool::from_config(config)),
        }
    }

    pub async fn inspect(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        subject_actor: &str,
        subject_client: &str,
    ) -> PgResult<Value> {
        if ![subject_actor, subject_client].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate(&tx, tenant, project, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::AccessManageProject, None, None)?;
        let state = snapshot(&tx, tenant, project, subject_actor, subject_client).await?;
        enforce_inspect_grant_ceiling(&auth, &active_grant_streams(&state))?;
        let impact = impact_report(&tx, tenant, project, subject_actor, subject_client).await?;
        let result = json!({
            "state_digest": hash(&state)?,
            "state": redacted_state(&state),
            "impact": impact,
            "credential_delivery": "protected_install_or_claim_channel_only",
            "raw_secrets_in_response": false
        });
        tx.commit().await?;
        Ok(result)
    }

    pub async fn preview(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        plan: &AdminAccessPlan,
    ) -> PgResult<Value> {
        plan.validate_admin_bounds()?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate(&tx, tenant, project, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::AccessManageProject, None, None)?;
        refuse_self_special_elevation(&auth, plan)?;
        let owner = plan.as_owner_plan(tenant, project);
        let state = snapshot(
            &tx,
            tenant,
            project,
            &plan.subject.id,
            &plan.subject_client_id,
        )
        .await?;
        let current_streams = if plan.remove_membership
            || state["membership"]["role"] != plan.role
            || state["membership"]["independent_review"] != plan.independent_review
            || state["membership"]["agent_review"] != plan.agent_review
            || business_roles_changed(plan, &state)
        {
            actor_active_grant_streams(&tx, tenant, project, &plan.subject.id).await?
        } else {
            active_grant_streams(&state)
        };
        enforce_client_grant_ceiling(&auth, plan, &current_streams)?;
        validate_current(&tx, &owner, &state).await?;
        members::validate_credentials(&tx, tenant, project, plan, &state).await?;
        ensure_last_admin_safe(&tx, tenant, project, plan, &state).await?;
        let impact = impact_report(
            &tx,
            tenant,
            project,
            &plan.subject.id,
            &plan.subject_client_id,
        )
        .await?;
        let plan_digest = hash(&serde_json::to_value(plan).map_err(|_| invalid())?)?;
        let result = json!({
            "applied": false,
            "state_digest": hash(&state)?,
            "plan_digest": plan_digest,
            "current": redacted_state(&state),
            "desired": public_admin_plan(plan),
            "impact": impact,
            "grant_semantics": "replace_selected_actor_client_project_grants",
            "membership_scope": "all_clients_of_actor_in_project",
            "credential_revocation_scope": "explicit_project_credentials_only",
            "project_revoke_preserves_other_projects": true,
            "permission_ceiling": "project_admin_template_without_special_authorities",
            "raw_secrets_in_response": false
        });
        tx.commit().await?;
        Ok(result)
    }

    pub async fn outcome(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: &str,
    ) -> PgResult<Value> {
        if !identity(request) {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate(&tx, tenant, project, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::AccessManageProject, None, None)?;
        // Outcome/replay inspection still requires an explicit manage grant ceiling.
        require_project_manage_grant(&auth)?;
        let row = tx
            .query_opt(
                "SELECT result_json FROM awr_team.project_access_changes
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant, &project, &request],
            )
            .await?;
        let result = match row {
            Some(r) => {
                json!({"outcome":"committed","receipt": redact_receipt(r.get::<_, Value>(0))})
            }
            None => json!({"outcome":"unknown"}),
        };
        tx.commit().await?;
        Ok(result)
    }

    pub async fn apply(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        plan: &AdminAccessPlan,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
    ) -> PgResult<Value> {
        plan.validate_admin_bounds()?;
        if !identity(request) || !hex(expected_state) || !hex(expected_plan) {
            return Err(invalid());
        }
        let plan_digest = hash(&serde_json::to_value(plan).map_err(|_| invalid())?)?;
        let intent_hash = hash(&json!({
            "protocol":"awr-project-admin-access-v1",
            "plan":plan,
            "expected_state":expected_state,
            "expected_plan":expected_plan
        }))?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = authenticate(&tx, tenant, project, bearer).await?;
        if let Err(err) =
            authorize_domain_action(&auth, awr_team::Action::AccessManageProject, None, None)
        {
            tx.rollback().await?;
            let _ = crate::ops_audit::record_deny(
                self.pool.as_ref(),
                tenant,
                project,
                &crate::ops_audit::OpsDenyWrite {
                    category: crate::ops_audit::OpsCategory::Access,
                    action: "access.manage_project".into(),
                    actor_id: Some(auth.actor_id.clone()),
                    client_id: Some(auth.client_id.clone()),
                    person_id: None,
                    target_kind: Some("member".into()),
                    target_id: Some(plan.subject.id.clone()),
                    request_id: Some(request.to_string()),
                    reason_code: "permission_denied".into(),
                },
            )
            .await;
            return Err(err);
        }
        refuse_self_special_elevation(&auth, plan)?;
        if let Some(r) = tx
            .query_opt(
                "SELECT request_hash,result_json FROM awr_team.project_access_changes
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant, &project, &request],
            )
            .await?
        {
            if r.get::<_, String>(0) != intent_hash {
                return Err(PgError::IdempotencyConflict);
            }
            let receipt = redact_receipt(r.get(1));
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":receipt,"raw_secrets_in_response":false}));
        }
        // Serialize subject actor changes (membership shared across clients).
        tx.query_opt(
            "SELECT id FROM awr_team.actors WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
            &[&tenant, &plan.subject.id],
        )
        .await?;
        let before = snapshot(
            &tx,
            tenant,
            project,
            &plan.subject.id,
            &plan.subject_client_id,
        )
        .await?;
        let current_streams = if plan.remove_membership
            || before["membership"]["role"] != plan.role
            || before["membership"]["independent_review"] != plan.independent_review
            || before["membership"]["agent_review"] != plan.agent_review
            || business_roles_changed(plan, &before)
        {
            actor_active_grant_streams(&tx, tenant, project, &plan.subject.id).await?
        } else {
            active_grant_streams(&before)
        };
        // Full-delta ceiling must run before digest gates so mismatched digests
        // cannot mask an unauthorized wipe/replace of unmanaged streams.
        enforce_client_grant_ceiling(&auth, plan, &current_streams)?;
        if plan_digest != expected_plan {
            return Err(PgError::PreconditionsChanged);
        }
        if hash(&before)? != expected_state {
            return Err(PgError::PreconditionsChanged);
        }
        let owner = plan.as_owner_plan(tenant, project);
        validate_current(&tx, &owner, &before).await?;
        members::validate_credentials(&tx, tenant, project, plan, &before).await?;
        ensure_last_admin_safe(&tx, tenant, project, plan, &before).await?;
        if plan.remove_membership {
            apply_remove_membership(&tx, tenant, project, plan).await?;
        } else {
            apply_policy(&tx, &owner).await?;
            members::apply_credentials(&tx, tenant, project, plan).await?;
        }
        let after = snapshot(
            &tx,
            tenant,
            project,
            &plan.subject.id,
            &plan.subject_client_id,
        )
        .await?;
        if !plan.remove_membership {
            validate_current(&tx, &owner, &after).await?;
        }
        let revision: i64 = tx
            .query_opt(
                "UPDATE awr_team.projects SET project_revision=project_revision+1
                 WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807
                 RETURNING project_revision",
                &[&tenant, &project],
            )
            .await?
            .ok_or(PgError::PreconditionsChanged)?
            .get(0);
        let impact = impact_report(
            &tx,
            tenant,
            project,
            &plan.subject.id,
            &plan.subject_client_id,
        )
        .await?;
        let receipt = json!({
            "protocol":"awr-project-admin-access-v1",
            "request_id":request,
            "request_hash":intent_hash,
            "admin_actor_id":auth.actor_id,
            "admin_client_id":auth.client_id,
            "tenant_id":tenant,
            "project_id":project,
            "subject_actor_id":plan.subject.id,
            "subject_client_id":plan.subject_client_id,
            "before_digest":expected_state,
            "after_digest":hash(&after)?,
            "plan_digest":plan_digest,
            "project_revision":revision.to_string(),
            "desired":public_admin_plan(plan),
            "previous_policy":policy(&before),
            "current_policy":policy(&after),
            "impact":impact,
            "state_basis":"at_commit",
            "execution_authorized":false,
            "credential_delivery":"protected_install_or_claim_channel_only",
            "raw_secrets_in_response":false
        });
        tx.execute(
            "INSERT INTO awr_team.project_access_changes(
                tenant_id,project_id,request_id,request_hash,admin_actor_id,admin_client_id,result_json)
             VALUES($1,$2,$3,$4,$5,$6,$7)",
            &[
                &tenant,
                &project,
                &request,
                &intent_hash,
                &auth.actor_id,
                &auth.client_id,
                &receipt,
            ],
        )
        .await?;
        let event = json!({
            "admin_actor_id":auth.actor_id,
            "admin_client_id":auth.client_id,
            "subject_actor_id":plan.subject.id,
            "subject_client_id":plan.subject_client_id,
            "request_id":request,
            "remove_membership":plan.remove_membership
        });
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
             VALUES($1,$2,$3,$4,0,'access.changed',$5,$6)",
            &[
                &tenant,
                &project,
                &crate::tx::new_id(),
                &revision,
                &auth.actor_id,
                &event,
            ],
        )
        .await?;
        // TMCP-040: bind ops audit in the same TX as access receipt/event.
        {
            let summary = serde_json::json!({
                "plan_digest": plan_digest,
                "remove_membership": plan.remove_membership,
                "subject_actor_id": plan.subject.id,
                "subject_client_id": plan.subject_client_id,
                "role": plan.role,
                "issued_credential_id": plan.credential.as_ref().map(|c| &c.id),
                "revoked_project_credentials": plan.revoke_project_credentials,
            });
            let digest = crate::ops_audit::digest_of(&summary);
            let mut audit = crate::ops_audit::write_from_auth(
                &auth,
                crate::ops_audit::OpsCategory::Access,
                "access.manage_project",
                "access_plan",
            );
            audit.request_id = Some(request.to_string());
            audit.target_id = Some(plan.subject.id.clone());
            audit.digest = Some(digest);
            audit.summary = summary;
            crate::ops_audit::record_in_tx(&tx, tenant, project, &audit).await?;
        }
        // Concurrent membership revoke of the caller must not commit. Intentional
        // self-demotion / last-admin handoff is allowed when another admin remains.
        let self_demotion = plan.subject.id == auth.actor_id
            && (plan.remove_membership
                || !policy_can_manage(&plan.role, effective_roles(plan, &before)?.as_ref()));
        if !self_demotion {
            let live = authenticate(&tx, tenant, project, bearer).await?;
            authorize_domain_action(&live, awr_team::Action::AccessManageProject, None, None)?;
            enforce_client_grant_ceiling(&live, plan, &current_streams)?;
        }
        tx.commit().await?;
        Ok(json!({"replayed":false,"receipt":receipt,"raw_secrets_in_response":false}))
    }
}

fn active_grant_streams(state: &Value) -> Vec<Id> {
    state
        .get("grants")
        .and_then(|g| g.as_array())
        .into_iter()
        .flatten()
        .filter(|g| g.get("active").and_then(|a| a.as_bool()).unwrap_or(true))
        .filter_map(|g| {
            g.get("workstream_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Id>().ok())
        })
        .collect()
}

fn redacted_state(state: &Value) -> Value {
    // Credentials already omit secret_hash; keep metadata only.
    state.clone()
}

fn redact_receipt(mut receipt: Value) -> Value {
    if let Some(obj) = receipt.as_object_mut() {
        if let Some(desired) = obj.get_mut("desired").and_then(|v| v.as_object_mut()) {
            if let Some(c) = desired
                .get_mut("credential")
                .and_then(|v| v.as_object_mut())
            {
                c.remove("secret_hash");
            }
        }
    }
    receipt
}

fn refuse_self_special_elevation(
    auth: &crate::workstream_auth::ReaderAuthority,
    plan: &AdminAccessPlan,
) -> PgResult<()> {
    // Body cannot forge caller identity; still refuse plans that try to attach
    // special authorities (already validated) or escalate beyond templates.
    if plan
        .grants
        .iter()
        .any(|g| g.attest_execution || g.reconcile_execution)
    {
        return Err(PgError::Forbidden);
    }
    // Non-admins never reach here (authorize_domain_action). An admin demoting
    // themselves is allowed only when another admin remains (checked separately).
    let _ = auth;
    Ok(())
}

async fn ensure_last_admin_safe(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    plan: &AdminAccessPlan,
    state: &Value,
) -> PgResult<()> {
    let current_role = state
        .get("membership")
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str());
    let old_roles = crate::workstream_auth::decode_business_roles(
        state["membership"].get("business_roles").cloned(),
    )?;
    let currently_admin =
        current_role.is_some_and(|role| policy_can_manage(role, old_roles.as_ref()));
    let will_be_admin = !plan.remove_membership
        && policy_can_manage(&plan.role, effective_roles(plan, state)?.as_ref());
    if currently_admin && !will_be_admin {
        if !other_live_manager_exists(tx, tenant, project, &plan.subject.id, &state["catalog"])
            .await?
        {
            return Err(PgError::Forbidden);
        }
    }
    Ok(())
}

fn business_roles_changed(plan: &AdminAccessPlan, state: &Value) -> bool {
    plan.business_roles
        .as_ref()
        .is_some_and(|roles| state["membership"]["business_roles"] != json!(roles))
}

fn effective_roles(
    plan: &AdminAccessPlan,
    state: &Value,
) -> PgResult<Option<BTreeSet<awr_team::BusinessRole>>> {
    if let Some(roles) = &plan.business_roles {
        return Ok(Some(roles.clone()));
    }
    crate::workstream_auth::decode_business_roles(
        state["membership"].get("business_roles").cloned(),
    )
}

fn policy_can_manage(role: &str, roles: Option<&BTreeSet<awr_team::BusinessRole>>) -> bool {
    is_admin_role(role)
        && crate::workstream_auth::role_ceiling_allows(roles, awr_team::Action::AccessManageProject)
}

/// A label-only, inactive or client-unscoped administrator cannot take over.
async fn other_live_manager_exists(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    excluded: &str,
    catalog: &Value,
) -> PgResult<bool> {
    let catalog: WorkstreamCatalog =
        serde_json::from_value(catalog.clone()).map_err(|_| PgError::SourceDivergence)?;
    let rows = tx.query("SELECT m.role,m.business_roles,g.workstream_id,g.authority_version,g.can_read,g.can_write
        FROM awr_team.project_memberships m
        JOIN awr_team.actors a ON a.tenant_id=m.tenant_id AND a.id=m.actor_id
        JOIN awr_team.workstream_grants g ON g.tenant_id=m.tenant_id AND g.project_id=m.project_id AND g.actor_id=m.actor_id
        WHERE m.tenant_id=$1 AND m.project_id=$2 AND m.actor_id<>$3 AND m.role IN ('admin','project_admin')
          AND a.status='active' AND a.kind IN ('human','system') AND g.active AND g.can_manage
          AND EXISTS (SELECT 1 FROM awr_team.credentials c WHERE c.tenant_id=m.tenant_id AND c.actor_id=m.actor_id
            AND c.client_id=g.client_id AND (c.project_id IS NULL OR c.project_id=m.project_id)
            AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>clock_timestamp()))",
        &[&tenant,&project,&excluded]).await?;
    for row in rows {
        let roles = crate::workstream_auth::decode_business_roles(row.get(1))?;
        if !policy_can_manage(&row.get::<_, String>(0), roles.as_ref()) {
            continue;
        }
        let access = awr_core::WorkstreamAccess {
            project_id: project.into(),
            subject: "verified-management-handoff".into(),
            grants: vec![awr_core::WorkstreamGrant {
                workstream_id: row
                    .get::<_, String>(2)
                    .parse()
                    .map_err(|_| PgError::Forbidden)?,
                authority_version: row
                    .get::<_, i64>(3)
                    .try_into()
                    .map_err(|_| PgError::Forbidden)?,
                read: row.get(4),
                write: row.get(5),
                manage: true,
            }],
        };
        if access
            .authorize(
                &catalog,
                access.grants[0].workstream_id,
                WorkstreamAction::Manage,
            )
            .is_ok()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn impact_report(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    actor: &str,
    client: &str,
) -> PgResult<Value> {
    let other_clients: Vec<Value> = tx
        .query(
            "SELECT DISTINCT client_id FROM awr_team.workstream_grants
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id<>$4
             ORDER BY client_id",
            &[&tenant, &project, &actor, &client],
        )
        .await?
        .iter()
        .map(|r| Value::String(r.get(0)))
        .collect();
    // Project RLS intentionally hides other projects; preserve-other-projects is a
    // write-scope guarantee (UPDATE/DELETE only touch the bound project), not a cross-project read.
    let same_credential_clients: Vec<Value> = tx
        .query(
            "SELECT id, client_id,
                    (extract(epoch FROM revoked_at)*1000)::bigint AS revoked_at_unix_ms
             FROM awr_team.credentials
             WHERE tenant_id=$1 AND actor_id=$2 AND (project_id IS NULL OR project_id=$3)
             ORDER BY id",
            &[&tenant, &actor, &project],
        )
        .await?
        .iter()
        .map(|r| {
            json!({
                "credential_id": r.get::<_, String>(0),
                "client_id": r.get::<_, String>(1),
                "revoked_at_unix_ms": r.get::<_, Option<i64>>(2)
            })
        })
        .collect();
    Ok(json!({
        "other_clients_sharing_membership": other_clients,
        "membership_change_affects_all_clients_of_actor": true,
        "project_grant_revoke_preserves_other_projects": true,
        "other_projects_not_readable_under_project_rls": true,
        "credentials_of_subject_actor": same_credential_clients,
        "tenant_credential_revoke": "requires_owner_operator_access_not_project_admin_mcp"
    }))
}

async fn apply_remove_membership(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    plan: &AdminAccessPlan,
) -> PgResult<()> {
    // Remove this project's grants for every client of the actor, then membership.
    // Grant FK references membership; other projects and tenant credentials are untouched.
    // Prior grant/membership state remains in the access receipt previous_policy.
    tx.execute(
        "DELETE FROM awr_team.workstream_grants
         WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3",
        &[&tenant, &project, &plan.subject.id],
    )
    .await?;
    tx.execute(
        "DELETE FROM awr_team.project_memberships
         WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3",
        &[&tenant, &project, &plan.subject.id],
    )
    .await?;
    Ok(())
}
