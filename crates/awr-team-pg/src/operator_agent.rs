//! Owner-only initial Agent delegation. Access identity provisioning is separate.
//! All writes and receipts share one transaction; no historical actor is rewritten.
use crate::operator_access::{require_owner_project, snapshot};
use crate::{PgError, PgResult};
use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, WorkstreamCatalog,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::{Client, Transaction};

const PROTOCOL: &str = "awr-operator-agent-v1";
const RENEW_PROTOCOL: &str = "awr-operator-agent-renew-v1";
const AUTHORIZE_PROTOCOL: &str = "awr-operator-agent-authorize-v1";
/// Initial plans must be reviewed/applied within 24 hours of their issue time.
const MAX_ISSUE_AGE_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProvisionPlan {
    pub protocol_version: u32,
    pub tenant_id: String,
    pub project_id: String,
    pub authorization: AgentAuthorization,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRenewPlan {
    pub protocol_version: u32,
    pub tenant_id: String,
    pub project_id: String,
    pub previous_authorization_id: String,
    pub authorization: AgentAuthorization,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAuthorizationIssuePlan {
    pub protocol_version: u32,
    pub tenant_id: String,
    pub project_id: String,
    pub authorization: AgentAuthorization,
}

fn invalid() -> PgError {
    PgError::Protocol("invalid initial Agent provisioning plan".into())
}
fn identity(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn hash(v: &Value) -> PgResult<String> {
    awr_team::request_hash(v).map_err(|_| invalid())
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn issue_time_valid(created: i64, now: i64) -> bool {
    created <= now && now.saturating_sub(created) <= MAX_ISSUE_AGE_MS
}
async fn clock(tx: &Transaction<'_>) -> PgResult<i64> {
    Ok(tx
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0))
}

impl AgentProvisionPlan {
    fn validate(&self) -> PgResult<()> {
        let a = &self.authorization;
        if self.protocol_version != 1
            || !identity(&self.tenant_id)
            || !identity(&self.project_id)
            || a.subject_kind != ExecutionSubjectKind::Agent
            || a.parent_authorization_id.is_some()
            || a.maintainer_person_id.is_some()
            || a.session_id.is_some()
            || a.authorizer_person_id != a.responsible_person_id
            || !a.verifiable_capabilities.is_empty()
            || !a.self_reported_skill_hints.is_empty()
            || a.actions.iter().any(|action| {
                !matches!(
                    action,
                    AuthorizedAction::Inspect
                        | AuthorizedAction::ClaimCoordination
                        | AuthorizedAction::StartWork
                        | AuthorizedAction::Review
                )
            })
            || a.scope.project_id() != self.project_id
            || !matches!(
                a.scope,
                AuthorizationScope::Workstream { .. } | AuthorizationScope::Task { .. }
            )
            || serde_json::to_vec(self).map_err(|_| invalid())?.len() > 65536
        {
            return Err(invalid());
        }
        awr_core::validate_issue(&IssueAuthorizationRequest {
            request_key: a.id.clone(),
            authorization: a.clone(),
        })
        .map_err(|_| invalid())
    }
}

impl AgentRenewPlan {
    fn provision_plan(&self) -> AgentProvisionPlan {
        AgentProvisionPlan {
            protocol_version: self.protocol_version,
            tenant_id: self.tenant_id.clone(),
            project_id: self.project_id.clone(),
            authorization: self.authorization.clone(),
        }
    }

    fn validate(&self) -> PgResult<()> {
        self.provision_plan().validate()?;
        if !identity(&self.previous_authorization_id)
            || self.previous_authorization_id == self.authorization.id
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl AgentAuthorizationIssuePlan {
    fn provision_plan(&self) -> AgentProvisionPlan {
        AgentProvisionPlan {
            protocol_version: self.protocol_version,
            tenant_id: self.tenant_id.clone(),
            project_id: self.project_id.clone(),
            authorization: self.authorization.clone(),
        }
    }

    fn validate(&self) -> PgResult<()> {
        self.provision_plan().validate()?;
        if self.authorization.expires_at_ms.is_none() {
            return Err(invalid());
        }
        Ok(())
    }
}

pub struct OperatorAgent;
impl OperatorAgent {
    /// Inspect the current binding against the retained initial plan. A receipt
    /// or matching configuration does not prove native credential possession.
    pub async fn inspect(client: &mut Client, plan: &AgentProvisionPlan) -> PgResult<Value> {
        plan.validate()?;
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        require_owner_project(&tx, &plan.tenant_id, &plan.project_id, false).await?;
        let state = state(&tx, plan).await?;
        let reason = configuration_reason(plan, &state, clock(&tx).await?);
        let result = json!({"protocol":PROTOCOL,"state_digest":hash(&state)?,"state":state,
            "configuration_matches_plan":reason.is_none(),"mismatch_reason":reason,
            "authority_basis":"initial_plan_configuration_only; native requests evaluate live permissions","execution_authorized":false,
            "human_approval":false,"team_independent_acceptance":false});
        tx.commit().await?;
        Ok(result)
    }

    pub async fn preview(client: &mut Client, plan: &AgentProvisionPlan) -> PgResult<Value> {
        plan.validate()?;
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        require_owner_project(&tx, &plan.tenant_id, &plan.project_id, false).await?;
        let state = state(&tx, plan).await?;
        validate_initial(plan, &state, clock(&tx).await?)?;
        let result = json!({"protocol":PROTOCOL,"applied":false,"state_digest":hash(&state)?,
            "plan_digest":hash(&json!(plan))?,"current":state,"desired":plan,
            "person_creation":"selected active human project-member identity; no historical rewrite",
            "maximum_issue_age_ms":MAX_ISSUE_AGE_MS,"execution_authorized":false});
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
        let tx = client.transaction().await?;
        require_owner_project(&tx, tenant, project, false).await?;
        let row=tx.query_opt("SELECT result_json FROM awr_team.access_changes WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",&[&tenant,&project,&request]).await?;
        let result = if let Some(row) = row {
            let receipt: Value = row.get(0);
            if receipt["protocol"] != PROTOCOL {
                return Err(PgError::IdempotencyConflict);
            }
            json!({"outcome":"committed","receipt":receipt,"execution_authorized":false})
        } else {
            json!({"outcome":"unknown","execution_authorized":false})
        };
        tx.commit().await?;
        Ok(result)
    }

    /// Preview an additional finite Agent authorization for a distinct scope.
    /// Existing identity, binding, access, and authorization rows are immutable.
    pub async fn authorize_preview(
        client: &mut Client,
        plan: &AgentAuthorizationIssuePlan,
    ) -> PgResult<Value> {
        plan.validate()?;
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        require_owner_project(&tx, &plan.tenant_id, &plan.project_id, false).await?;
        let state = authorization_state(&tx, &plan.provision_plan()).await?;
        validate_authorization_issue(plan, &state, clock(&tx).await?, false)?;
        let result = json!({
            "protocol": AUTHORIZE_PROTOCOL,
            "applied": false,
            "state_digest": hash(&state)?,
            "plan_digest": hash(&json!(plan))?,
            "current": state,
            "desired": plan,
            "binding_reused": true,
            "historical_authorizations_rewritten": false,
            "maximum_issue_age_ms": MAX_ISSUE_AGE_MS,
            "execution_authorized": false,
        });
        tx.commit().await?;
        Ok(result)
    }

    pub async fn authorize_outcome(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, request].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        require_owner_project(&tx, tenant, project, false).await?;
        let row=tx.query_opt("SELECT result_json FROM awr_team.access_changes WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",&[&tenant,&project,&request]).await?;
        let result = if let Some(row) = row {
            let receipt: Value = row.get(0);
            if receipt["protocol"] != AUTHORIZE_PROTOCOL {
                return Err(PgError::IdempotencyConflict);
            }
            json!({"outcome":"committed","receipt":receipt,"execution_authorized":false})
        } else {
            json!({"outcome":"unknown","execution_authorized":false})
        };
        tx.commit().await?;
        Ok(result)
    }

    pub async fn authorize_apply(
        client: &mut Client,
        plan: &AgentAuthorizationIssuePlan,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
    ) -> PgResult<Value> {
        plan.validate()?;
        if !identity(request) || !digest(expected_state) || !digest(expected_plan) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, &plan.tenant_id, &plan.project_id, true).await?;
        let intent = hash(
            &json!({"protocol":AUTHORIZE_PROTOCOL,"plan":plan,"expected_state":expected_state,"expected_plan":expected_plan}),
        )?;
        if let Some(row)=tx.query_opt("SELECT request_hash,result_json FROM awr_team.access_changes WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",&[&plan.tenant_id,&plan.project_id,&request]).await?{
            let receipt:Value=row.get(1);
            if row.get::<_,String>(0)!=intent || receipt["protocol"]!=AUTHORIZE_PROTOCOL{return Err(PgError::IdempotencyConflict);}
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":receipt,"execution_authorized":false}));
        }
        let a = &plan.authorization;
        let mut actors = vec![a.subject_id.as_str(), a.responsible_person_id.as_str()];
        actors.sort();
        actors.dedup();
        for actor in actors {
            tx.query_opt(
                "SELECT id FROM awr_team.actors WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
                &[&plan.tenant_id, &actor],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        }
        let provision = plan.provision_plan();
        let before = authorization_state(&tx, &provision).await?;
        if hash(&before)? != expected_state || hash(&json!(plan))? != expected_plan {
            return Err(PgError::PreconditionsChanged);
        }
        validate_authorization_issue(plan, &before, clock(&tx).await?, false)?;
        let authorization_request = format!("operator-agent-authorize:{}", hash(&json!(request))?);
        let (_, issued) = crate::agent_authorization::issue_in_tx(
            &tx,
            &plan.tenant_id,
            &plan.project_id,
            &IssueAuthorizationRequest {
                request_key: authorization_request,
                authorization: a.clone(),
            },
        )
        .await?;
        if issued.replayed {
            return Err(PgError::IdempotencyConflict);
        }
        let mut after = authorization_state(&tx, &provision).await?;
        if validate_authorization_issue(plan, &after, clock(&tx).await?, true).is_err() {
            return Err(PgError::PreconditionsChanged);
        }
        let revision:i64=tx.query_opt("UPDATE awr_team.projects SET project_revision=project_revision+1 WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807 RETURNING project_revision",&[&plan.tenant_id,&plan.project_id]).await?.ok_or(PgError::PreconditionsChanged)?.get(0);
        after["project_revision"] = json!(revision.to_string());
        let binding = a.binding_id.as_deref().ok_or_else(invalid)?;
        let receipt = json!({"protocol":AUTHORIZE_PROTOCOL,"request_id":request,"request_hash":intent,"operator_role":operator,
            "tenant_id":plan.tenant_id,"project_id":plan.project_id,"actor_id":a.subject_id,"client_id":a.client_id,
            "authorization_id":a.id,"authorization_request_key":issued.request_key,"binding_id":binding,
            "person_id":a.responsible_person_id,"scope":a.scope,"actions":a.actions,
            "before_digest":expected_state,"after_digest":hash(&after)?,"event_id":issued.event_id,
            "plan_digest":expected_plan,"project_revision":revision.to_string(),"state_basis":"at_commit",
            "execution_authorized":false,"binding_reused":true,"historical_authorizations_rewritten":false,
            "historical_identities_rewritten":false,"human_approval":false,"team_independent_acceptance":false});
        tx.execute("INSERT INTO awr_team.access_changes(tenant_id,project_id,request_id,request_hash,operator_role,result_json) VALUES($1,$2,$3,$4,$5,$6)",&[&plan.tenant_id,&plan.project_id,&request,&intent,&operator,&receipt]).await?;
        let summary = json!({"plan_digest":expected_plan,"request_id":request,"authorization_id":a.id,
            "binding_id":binding,"subject_actor_id":a.subject_id,"subject_client_id":a.client_id,
            "responsible_person_id":a.responsible_person_id,"scope":a.scope,"actions":a.actions,
            "operator_role":operator,"historical_authorizations_rewritten":false});
        tx.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json) VALUES($1,$2,$3,$4,0,'agent.authorization.authorized',$5,$6)",
            &[&plan.tenant_id,&plan.project_id,&issued.event_id,&revision,&operator,&summary]).await?;
        let audit = crate::OpsAuditWrite {
            category: crate::OpsCategory::Access,
            action: "agent.authorization.authorize".into(),
            result: "committed",
            person_id: None,
            actor_id: operator.clone(),
            client_id: "awr-server-owner-cli".into(),
            target_kind: "access_plan".into(),
            target_id: Some(a.id.clone()),
            work_id: None,
            change_id: None,
            request_id: Some(request.into()),
            membership_version: None,
            authority_version: None,
            policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
            source_version: None,
            digest: Some(crate::digest_of(&summary)),
            summary,
        };
        crate::record_in_tx(&tx, &plan.tenant_id, &plan.project_id, &audit).await?;
        tx.commit().await?;
        Ok(json!({"replayed":false,"receipt":receipt,"execution_authorized":false}))
    }

    /// Preview a finite-lived successor for an expired initial authorization.
    /// The existing person↔Agent binding and predecessor row remain immutable.
    pub async fn renew_preview(client: &mut Client, plan: &AgentRenewPlan) -> PgResult<Value> {
        plan.validate()?;
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        require_owner_project(&tx, &plan.tenant_id, &plan.project_id, false).await?;
        let state = authorization_state(&tx, &plan.provision_plan()).await?;
        let now = clock(&tx).await?;
        validate_renewal(plan, &state, now, false)?;
        let previous_lifetime_ms = predecessor_lifetime(plan, &state)?;
        let result = json!({
            "protocol": RENEW_PROTOCOL,
            "applied": false,
            "state_digest": hash(&state)?,
            "plan_digest": hash(&json!(plan))?,
            "current": state,
            "desired": plan,
            "previous_lifetime_ms": previous_lifetime_ms.to_string(),
            "binding_reused": true,
            "predecessor_rewritten": false,
            "maximum_issue_age_ms": MAX_ISSUE_AGE_MS,
            "execution_authorized": false,
        });
        tx.commit().await?;
        Ok(result)
    }

    pub async fn renew_outcome(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, request].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        require_owner_project(&tx, tenant, project, false).await?;
        let row=tx.query_opt("SELECT result_json FROM awr_team.access_changes WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",&[&tenant,&project,&request]).await?;
        let result = if let Some(row) = row {
            let receipt: Value = row.get(0);
            if receipt["protocol"] != RENEW_PROTOCOL {
                return Err(PgError::IdempotencyConflict);
            }
            json!({"outcome":"committed","receipt":receipt,"execution_authorized":false})
        } else {
            json!({"outcome":"unknown","execution_authorized":false})
        };
        tx.commit().await?;
        Ok(result)
    }

    pub async fn renew_apply(
        client: &mut Client,
        plan: &AgentRenewPlan,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
    ) -> PgResult<Value> {
        plan.validate()?;
        if !identity(request) || !digest(expected_state) || !digest(expected_plan) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, &plan.tenant_id, &plan.project_id, true).await?;
        let intent = hash(
            &json!({"protocol":RENEW_PROTOCOL,"plan":plan,"expected_state":expected_state,"expected_plan":expected_plan}),
        )?;
        if let Some(row)=tx.query_opt("SELECT request_hash,result_json FROM awr_team.access_changes WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",&[&plan.tenant_id,&plan.project_id,&request]).await?{
            let receipt:Value=row.get(1);
            if row.get::<_,String>(0)!=intent || receipt["protocol"]!=RENEW_PROTOCOL{return Err(PgError::IdempotencyConflict);}
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":receipt,"execution_authorized":false}));
        }
        let a = &plan.authorization;
        let mut actors = vec![a.subject_id.as_str(), a.responsible_person_id.as_str()];
        actors.sort();
        actors.dedup();
        for actor in actors {
            tx.query_opt(
                "SELECT id FROM awr_team.actors WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
                &[&plan.tenant_id, &actor],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        }
        let provision = plan.provision_plan();
        let before = authorization_state(&tx, &provision).await?;
        if hash(&before)? != expected_state || hash(&json!(plan))? != expected_plan {
            return Err(PgError::PreconditionsChanged);
        }
        let now = clock(&tx).await?;
        validate_renewal(plan, &before, now, false)?;
        let previous_lifetime_ms = predecessor_lifetime(plan, &before)?;
        let authorization_request = format!("operator-agent-renew:{}", hash(&json!(request))?);
        let (_, issued) = crate::agent_authorization::issue_in_tx(
            &tx,
            &plan.tenant_id,
            &plan.project_id,
            &IssueAuthorizationRequest {
                request_key: authorization_request,
                authorization: a.clone(),
            },
        )
        .await?;
        if issued.replayed {
            return Err(PgError::IdempotencyConflict);
        }
        let mut after = authorization_state(&tx, &provision).await?;
        if validate_renewal(plan, &after, clock(&tx).await?, true).is_err() {
            return Err(PgError::PreconditionsChanged);
        }
        let revision:i64=tx.query_opt("UPDATE awr_team.projects SET project_revision=project_revision+1 WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807 RETURNING project_revision",&[&plan.tenant_id,&plan.project_id]).await?.ok_or(PgError::PreconditionsChanged)?.get(0);
        after["project_revision"] = json!(revision.to_string());
        let binding = a.binding_id.as_deref().ok_or_else(invalid)?;
        let receipt = json!({"protocol":RENEW_PROTOCOL,"request_id":request,"request_hash":intent,"operator_role":operator,
            "tenant_id":plan.tenant_id,"project_id":plan.project_id,"actor_id":a.subject_id,"client_id":a.client_id,
            "previous_authorization_id":plan.previous_authorization_id,"authorization_id":a.id,
            "authorization_request_key":issued.request_key,"binding_id":binding,"person_id":a.responsible_person_id,
            "previous_lifetime_ms":previous_lifetime_ms.to_string(),"before_digest":expected_state,
            "after_digest":hash(&after)?,"event_id":issued.event_id,"plan_digest":expected_plan,
            "project_revision":revision.to_string(),"state_basis":"at_commit","execution_authorized":false,
            "binding_reused":true,"predecessor_rewritten":false,"historical_identities_rewritten":false,
            "human_approval":false,"team_independent_acceptance":false});
        tx.execute("INSERT INTO awr_team.access_changes(tenant_id,project_id,request_id,request_hash,operator_role,result_json) VALUES($1,$2,$3,$4,$5,$6)",&[&plan.tenant_id,&plan.project_id,&request,&intent,&operator,&receipt]).await?;
        let summary = json!({"plan_digest":expected_plan,"request_id":request,
            "previous_authorization_id":plan.previous_authorization_id,"authorization_id":a.id,
            "binding_id":binding,"subject_actor_id":a.subject_id,"subject_client_id":a.client_id,
            "responsible_person_id":a.responsible_person_id,"previous_lifetime_ms":previous_lifetime_ms.to_string(),
            "operator_role":operator,"predecessor_rewritten":false});
        tx.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json) VALUES($1,$2,$3,$4,0,'agent.authorization.renewed',$5,$6)",
            &[&plan.tenant_id,&plan.project_id,&issued.event_id,&revision,&operator,&summary]).await?;
        let audit = crate::OpsAuditWrite {
            category: crate::OpsCategory::Access,
            action: "agent.authorization.renew".into(),
            result: "committed",
            person_id: None,
            actor_id: operator.clone(),
            client_id: "awr-server-owner-cli".into(),
            target_kind: "access_plan".into(),
            target_id: Some(a.id.clone()),
            work_id: None,
            change_id: None,
            request_id: Some(request.into()),
            membership_version: None,
            authority_version: None,
            policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
            source_version: None,
            digest: Some(crate::digest_of(&summary)),
            summary,
        };
        crate::record_in_tx(&tx, &plan.tenant_id, &plan.project_id, &audit).await?;
        tx.commit().await?;
        Ok(json!({"replayed":false,"receipt":receipt,"execution_authorized":false}))
    }

    pub async fn apply(
        client: &mut Client,
        plan: &AgentProvisionPlan,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
    ) -> PgResult<Value> {
        plan.validate()?;
        if !identity(request) || !digest(expected_state) || !digest(expected_plan) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, &plan.tenant_id, &plan.project_id, true).await?;
        let intent = hash(
            &json!({"protocol":PROTOCOL,"plan":plan,"expected_state":expected_state,"expected_plan":expected_plan}),
        )?;
        if let Some(row)=tx.query_opt("SELECT request_hash,result_json FROM awr_team.access_changes WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",&[&plan.tenant_id,&plan.project_id,&request]).await?{
            let receipt:Value=row.get(1);
            if row.get::<_,String>(0)!=intent || receipt["protocol"]!=PROTOCOL{return Err(PgError::IdempotencyConflict);}
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":receipt,"execution_authorized":false}));
        }
        // The project lock also serializes the internal WS-015/016 stores.
        // Actor locks cover tenant-wide access edits across different projects.
        let a = &plan.authorization;
        let mut actors = vec![a.subject_id.as_str(), a.responsible_person_id.as_str()];
        actors.sort();
        actors.dedup();
        for actor in actors {
            tx.query_opt(
                "SELECT id FROM awr_team.actors WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
                &[&plan.tenant_id, &actor],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        }
        let before = state(&tx, plan).await?;
        if hash(&before)? != expected_state || hash(&json!(plan))? != expected_plan {
            return Err(PgError::PreconditionsChanged);
        }
        validate_initial(plan, &before, clock(&tx).await?)?;
        let name = before["human_actor"]["display_name"]
            .as_str()
            .ok_or_else(invalid)?;
        tx.execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES($1,$2,$3,$4,'active') ON CONFLICT DO NOTHING",&[&plan.tenant_id,&plan.project_id,&a.responsible_person_id.as_str(),&name]).await?;
        let binding = a.binding_id.as_deref().ok_or_else(invalid)?;
        tx.execute("INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES($1,$2,$3,$4,$5,'active')",
            &[&plan.tenant_id,&plan.project_id,&binding,&a.responsible_person_id.as_str(),&a.subject_id]).await?;
        let authorization_request = format!("operator-agent:{}", hash(&json!(request))?);
        let (_, issued) = crate::agent_authorization::issue_in_tx(
            &tx,
            &plan.tenant_id,
            &plan.project_id,
            &IssueAuthorizationRequest {
                request_key: authorization_request,
                authorization: a.clone(),
            },
        )
        .await?;
        if issued.replayed {
            return Err(PgError::IdempotencyConflict);
        }
        let mut after = state(&tx, plan).await?;
        if configuration_reason(plan, &after, clock(&tx).await?).is_some() {
            return Err(PgError::PreconditionsChanged);
        }
        let revision:i64=tx.query_opt("UPDATE awr_team.projects SET project_revision=project_revision+1 WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807 RETURNING project_revision",&[&plan.tenant_id,&plan.project_id]).await?.ok_or(PgError::PreconditionsChanged)?.get(0);
        after["project_revision"] = json!(revision.to_string());
        let receipt = json!({"protocol":PROTOCOL,"request_id":request,"request_hash":intent,"operator_role":operator,
            "tenant_id":plan.tenant_id,"project_id":plan.project_id,"actor_id":a.subject_id,"client_id":a.client_id,
            "authorization_id":a.id,"authorization_request_key":issued.request_key,"binding_id":binding,"person_id":a.responsible_person_id,
            "before_digest":expected_state,"after_digest":hash(&after)?,"event_id":issued.event_id,"plan_digest":expected_plan,
            "project_revision":revision.to_string(),"state_basis":"at_commit","execution_authorized":false,
            "historical_identities_rewritten":false,"human_approval":false,"team_independent_acceptance":false});
        tx.execute("INSERT INTO awr_team.access_changes(tenant_id,project_id,request_id,request_hash,operator_role,result_json) VALUES($1,$2,$3,$4,$5,$6)",&[&plan.tenant_id,&plan.project_id,&request,&intent,&operator,&receipt]).await?;
        let summary = json!({"plan_digest":expected_plan,"request_id":request,"authorization_id":a.id,"binding_id":binding,
            "subject_actor_id":a.subject_id,"subject_client_id":a.client_id,"responsible_person_id":a.responsible_person_id,"operator_role":operator});
        tx.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json) VALUES($1,$2,$3,$4,0,'agent.authorization.issued',$5,$6)",
            &[&plan.tenant_id,&plan.project_id,&issued.event_id,&revision,&operator,&summary]).await?;
        let audit = crate::OpsAuditWrite {
            category: crate::OpsCategory::Access,
            action: "agent.authorization.issue".into(),
            result: "committed",
            person_id: None,
            actor_id: operator.clone(),
            client_id: "awr-server-owner-cli".into(),
            target_kind: "access_plan".into(),
            target_id: Some(a.id.clone()),
            work_id: None,
            change_id: None,
            request_id: Some(request.into()),
            membership_version: None,
            authority_version: None,
            policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
            source_version: None,
            digest: Some(crate::digest_of(&summary)),
            summary,
        };
        crate::record_in_tx(&tx, &plan.tenant_id, &plan.project_id, &audit).await?;
        tx.commit().await?;
        Ok(json!({"replayed":false,"receipt":receipt,"execution_authorized":false}))
    }
}

fn current_task_stream<'a>(state: &'a Value, work_item_id: &str) -> Option<&'a str> {
    let item = state["authorization_task_ownership"]
        .as_array()?
        .iter()
        .find(|item| item["work_item_id"] == work_item_id)?;
    let ownership = &item["ownership"];
    if ownership["definition_state"] != "enabled"
        || ownership["workstream_id"] != ownership["snapshot_workstream_id"]
        || ownership["ownership_version"] != ownership["snapshot_ownership_version"]
    {
        return None;
    }
    ownership["workstream_id"].as_str()
}

fn authorization_scopes_overlap(
    existing: &AuthorizationScope,
    proposed: &AuthorizationScope,
    state: &Value,
) -> bool {
    match (existing, proposed) {
        (AuthorizationScope::Project { .. }, _) | (_, AuthorizationScope::Project { .. }) => true,
        (
            AuthorizationScope::Workstream {
                workstream_id: existing,
                ..
            },
            AuthorizationScope::Workstream {
                workstream_id: proposed,
                ..
            },
        ) => existing == proposed,
        (
            AuthorizationScope::Task {
                work_item_id: existing,
                ..
            },
            AuthorizationScope::Task {
                work_item_id: proposed,
                ..
            },
        ) => existing == proposed,
        (
            AuthorizationScope::Workstream { workstream_id, .. },
            AuthorizationScope::Task { work_item_id, .. },
        ) => current_task_stream(state, work_item_id) == Some(workstream_id),
        (
            AuthorizationScope::Task { work_item_id, .. },
            AuthorizationScope::Workstream { workstream_id, .. },
        ) => {
            current_task_stream(state, work_item_id).is_none_or(|current| current == workstream_id)
        }
        (AuthorizationScope::TaskPool { .. }, _) | (_, AuthorizationScope::TaskPool { .. }) => true,
    }
}

fn validate_authorization_issue(
    p: &AgentAuthorizationIssuePlan,
    state: &Value,
    now: i64,
    authorization_present: bool,
) -> PgResult<()> {
    let provision = p.provision_plan();
    if state["access"]["membership"].is_null()
        || access_reason(&provision, state, now).is_some()
        || state["person"].is_null()
    {
        return Err(PgError::Forbidden);
    }
    let a = &p.authorization;
    let bindings = state["bindings"].as_array().ok_or(PgError::Forbidden)?;
    let active_bindings = bindings
        .iter()
        .filter(|binding| binding["status"] == "active")
        .collect::<Vec<_>>();
    if active_bindings.len() != 1
        || active_bindings[0]["id"].as_str() != a.binding_id.as_deref()
        || active_bindings[0]["person_id"] != a.responsible_person_id.as_str()
        || active_bindings[0]["agent_id"] != a.subject_id
        || !issue_time_valid(a.created_at_ms, now)
        || !a.is_effective_at(now)
    {
        return Err(PgError::Forbidden);
    }
    let authorizations = state["authorizations"]
        .as_array()
        .ok_or(PgError::Forbidden)?;
    let stored_authorization = authorizations.iter().find(|item| item["id"] == a.id);
    if authorization_present {
        let stored: AgentAuthorization = serde_json::from_value(
            stored_authorization
                .cloned()
                .ok_or(PgError::PreconditionsChanged)?,
        )
        .map_err(|_| invalid())?;
        if &stored != a || !stored.is_effective_at(now) {
            return Err(PgError::PreconditionsChanged);
        }
    } else if stored_authorization.is_some() {
        return Err(PgError::PreconditionsChanged);
    }
    for stored in authorizations {
        let stored: AgentAuthorization =
            serde_json::from_value(stored.clone()).map_err(|_| invalid())?;
        if stored.id != a.id
            && stored.subject_id == a.subject_id
            && stored.client_id == a.client_id
            && stored.is_effective_at(now)
            && authorization_scopes_overlap(&stored.scope, &a.scope, state)
        {
            return Err(PgError::Forbidden);
        }
    }
    Ok(())
}

fn predecessor<'a>(p: &AgentRenewPlan, s: &'a Value) -> PgResult<AgentAuthorization> {
    let stored = s["authorizations"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["id"] == p.previous_authorization_id)
        })
        .ok_or(PgError::Forbidden)?;
    serde_json::from_value(stored.clone()).map_err(|_| invalid())
}

fn predecessor_lifetime(p: &AgentRenewPlan, s: &Value) -> PgResult<i64> {
    let previous = predecessor(p, s)?;
    previous
        .expires_at_ms
        .and_then(|expires| expires.checked_sub(previous.created_at_ms))
        .filter(|duration| *duration > 0)
        .ok_or_else(invalid)
}

fn same_renewed_authority(previous: &AgentAuthorization, next: &AgentAuthorization) -> bool {
    previous.authorizer_person_id == next.authorizer_person_id
        && previous.responsible_person_id == next.responsible_person_id
        && previous.subject_kind == next.subject_kind
        && previous.subject_id == next.subject_id
        && previous.client_id == next.client_id
        && previous.session_id == next.session_id
        && previous.model_id == next.model_id
        && previous.scope == next.scope
        && previous.actions == next.actions
        && previous.verifiable_capabilities == next.verifiable_capabilities
        && previous.self_reported_skill_hints == next.self_reported_skill_hints
        && previous.parent_authorization_id == next.parent_authorization_id
        && previous.maintainer_person_id == next.maintainer_person_id
        && previous.binding_id == next.binding_id
}

fn validate_renewal(
    p: &AgentRenewPlan,
    s: &Value,
    now: i64,
    successor_present: bool,
) -> PgResult<()> {
    let provision = p.provision_plan();
    if s["access"]["membership"].is_null() || access_reason(&provision, s, now).is_some() {
        return Err(PgError::Forbidden);
    }
    if s["person"].is_null() {
        return Err(PgError::Forbidden);
    }
    let a = &p.authorization;
    let bindings = s["bindings"].as_array().ok_or(PgError::Forbidden)?;
    if bindings.len() != 1
        || bindings[0]["id"].as_str() != a.binding_id.as_deref()
        || bindings[0]["person_id"] != a.responsible_person_id.as_str()
        || bindings[0]["agent_id"] != a.subject_id
        || bindings[0]["status"] != "active"
    {
        return Err(PgError::Forbidden);
    }
    let previous = predecessor(p, s)?;
    previous.validate().map_err(|_| invalid())?;
    if previous.id == a.id
        || !matches!(previous.status, AuthorizationStatus::Active)
        || previous.revoked_at_ms.is_some()
        || previous.revoked_by.is_some()
        || !same_renewed_authority(&previous, a)
    {
        return Err(PgError::Forbidden);
    }
    let previous_expiry = previous.expires_at_ms.ok_or(PgError::Forbidden)?;
    let successor_expiry = a.expires_at_ms.ok_or(PgError::Forbidden)?;
    let previous_lifetime = previous_expiry
        .checked_sub(previous.created_at_ms)
        .filter(|duration| *duration > 0)
        .ok_or(PgError::Forbidden)?;
    let successor_lifetime = successor_expiry
        .checked_sub(a.created_at_ms)
        .filter(|duration| *duration > 0)
        .ok_or(PgError::Forbidden)?;
    if now < previous_expiry
        || previous.is_effective_at(now)
        || a.created_at_ms < previous_expiry
        || !issue_time_valid(a.created_at_ms, now)
        || !a.is_effective_at(now)
        || successor_lifetime != previous_lifetime
    {
        return Err(PgError::Forbidden);
    }
    let auths = s["authorizations"].as_array().ok_or(PgError::Forbidden)?;
    let stored_successor = auths.iter().find(|item| item["id"] == a.id);
    if successor_present {
        let stored: AgentAuthorization = serde_json::from_value(
            stored_successor
                .cloned()
                .ok_or(PgError::PreconditionsChanged)?,
        )
        .map_err(|_| invalid())?;
        if &stored != a || !stored.is_effective_at(now) {
            return Err(PgError::PreconditionsChanged);
        }
    } else if stored_successor.is_some() {
        return Err(PgError::PreconditionsChanged);
    }
    for stored in auths {
        let stored: AgentAuthorization =
            serde_json::from_value(stored.clone()).map_err(|_| invalid())?;
        if !authorization_scopes_overlap(&stored.scope, &a.scope, s) {
            continue;
        }
        if stored.id != a.id
            && stored.id != previous.id
            && stored.created_at_ms >= previous.created_at_ms
        {
            return Err(PgError::Forbidden);
        }
        if stored.id != p.previous_authorization_id
            && stored.id != a.id
            && stored.is_effective_at(now)
        {
            return Err(PgError::Forbidden);
        }
    }
    Ok(())
}

async fn task_ownership_state(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: Option<&str>,
    work_item_id: &str,
) -> PgResult<Option<Value>> {
    Ok(tx.query_opt("SELECT o.workstream_id,o.ownership_version,s.workstream_id,s.ownership_version,c.contract_hash,c.definition_state FROM awr_team.workstream_ownership o JOIN awr_team.workstream_snapshot_ownership s ON s.tenant_id=o.tenant_id AND s.project_id=o.project_id AND s.work_id=o.work_id AND s.scope_id='main' JOIN awr_team.work_contracts c ON c.tenant_id=s.tenant_id AND c.project_id=s.project_id AND c.snapshot_id=s.snapshot_id AND c.scope_id=s.scope_id AND c.work_id=s.work_id WHERE o.tenant_id=$1 AND o.project_id=$2 AND o.work_id=$3 AND s.snapshot_id=$4 FOR SHARE OF o,s,c",&[&tenant,&project,&work_item_id,&snapshot]).await?
        .map(|r|json!({"workstream_id":r.get::<_,String>(0),"ownership_version":r.get::<_,i64>(1),"snapshot_workstream_id":r.get::<_,String>(2),"snapshot_ownership_version":r.get::<_,i64>(3),"contract_hash":r.get::<_,String>(4),"definition_state":r.get::<_,String>(5)})))
}

async fn state(tx: &Transaction<'_>, p: &AgentProvisionPlan) -> PgResult<Value> {
    let a = &p.authorization;
    let access = snapshot(tx, &p.tenant_id, &p.project_id, &a.subject_id, &a.client_id).await?;
    let project = tx
        .query_one(
            "SELECT status,project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&p.tenant_id, &p.project_id],
        )
        .await?;
    let human=tx.query_opt("SELECT a.kind,a.display_name,a.status,m.role,m.membership_version FROM awr_team.actors a JOIN awr_team.project_memberships m ON m.tenant_id=a.tenant_id AND m.actor_id=a.id WHERE a.tenant_id=$1 AND m.project_id=$2 AND a.id=$3 FOR SHARE OF a,m",&[&p.tenant_id,&p.project_id,&a.responsible_person_id.as_str()]).await?
        .map(|r|json!({"kind":r.get::<_,String>(0),"display_name":r.get::<_,String>(1),"status":r.get::<_,String>(2),"role":r.get::<_,String>(3),"membership_version":r.get::<_,i64>(4).to_string()}));
    let person=tx.query_opt("SELECT display_name,status FROM awr_team.persons WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR SHARE",&[&p.tenant_id,&p.project_id,&a.responsible_person_id.as_str()]).await?
        .map(|r|json!({"display_name":r.get::<_,String>(0),"status":r.get::<_,String>(1)}));
    let bindings=tx.query("SELECT id,person_id,agent_id,status FROM awr_team.person_agent_bindings WHERE tenant_id=$1 AND project_id=$2 AND (agent_id=$3 OR id=$4) ORDER BY id FOR SHARE",&[&p.tenant_id,&p.project_id,&a.subject_id,&a.binding_id]).await?.into_iter()
        .map(|r|json!({"id":r.get::<_,String>(0),"person_id":r.get::<_,String>(1),"agent_id":r.get::<_,String>(2),"status":r.get::<_,String>(3)})).collect::<Vec<_>>();
    let auths=tx.query("SELECT body_json FROM awr_team.agent_authorizations WHERE tenant_id=$1 AND project_id=$2 AND (id=$3 OR (subject_id=$4 AND client_id=$5)) ORDER BY id FOR SHARE",&[&p.tenant_id,&p.project_id,&a.id,&a.subject_id,&a.client_id]).await?.into_iter().map(|r|r.get::<_,Value>(0)).collect::<Vec<_>>();
    let source_snapshot = access["source_snapshot_id"].as_str();
    let ownership = match &a.scope {
        AuthorizationScope::Task { work_item_id, .. } => {
            task_ownership_state(
                tx,
                &p.tenant_id,
                &p.project_id,
                source_snapshot,
                work_item_id,
            )
            .await?
        }
        _ => None,
    };
    Ok(
        json!({"project_status":project.get::<_,String>(0),"project_revision":project.get::<_,i64>(1).to_string(),"access":access,"human_actor":human,"person":person,"bindings":bindings,"authorizations":auths,"task_ownership":ownership}),
    )
}

async fn authorization_state(tx: &Transaction<'_>, plan: &AgentProvisionPlan) -> PgResult<Value> {
    let mut state = state(tx, plan).await?;
    let source_snapshot = state["access"]["source_snapshot_id"].as_str();
    let mut authorization_tasks = state["authorizations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| serde_json::from_value::<AgentAuthorization>(value.clone()).ok())
        .filter_map(|authorization| match authorization.scope {
            AuthorizationScope::Task { work_item_id, .. } => Some(work_item_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let AuthorizationScope::Task { work_item_id, .. } = &plan.authorization.scope {
        authorization_tasks.push(work_item_id.clone());
    }
    authorization_tasks.sort();
    authorization_tasks.dedup();
    let mut ownership = Vec::with_capacity(authorization_tasks.len());
    for work_item_id in authorization_tasks {
        let task_ownership = task_ownership_state(
            tx,
            &plan.tenant_id,
            &plan.project_id,
            source_snapshot,
            &work_item_id,
        )
        .await?;
        ownership.push(json!({
            "work_item_id": work_item_id,
            "ownership": task_ownership,
        }));
    }
    state["authorization_task_ownership"] = json!(ownership);
    Ok(state)
}

fn access_reason(p: &AgentProvisionPlan, s: &Value, now: i64) -> Option<&'static str> {
    let a = &p.authorization;
    let access = &s["access"];
    if s["project_status"] != "active" || access["tenant_status"] != "active" {
        return Some("project_inactive");
    }
    if access["actor"]["kind"] != "agent" || access["actor"]["status"] != "active" {
        return Some("actor_not_active_agent");
    }
    let Some(role) = access["membership"]["role"]
        .as_str()
        .and_then(crate::workstream_auth::map_membership_role)
    else {
        return Some("membership_missing");
    };
    if access["membership"]["independent_review"] == true {
        return Some("agent_cannot_be_human_reviewer");
    }
    let human = &s["human_actor"];
    if human["kind"] != "human"
        || human["status"] != "active"
        || human["role"]
            .as_str()
            .and_then(crate::workstream_auth::map_membership_role)
            .is_none()
    {
        return Some("person_not_active_human_member");
    }
    if !s["person"].is_null()
        && (s["person"]["status"] != "active"
            || s["person"]["display_name"] != human["display_name"])
    {
        return Some("person_mismatch");
    }
    if !access["credentials"].as_array().is_some_and(|cs| {
        cs.iter().any(|c| {
            c["revoked_at_unix_ms"].is_null()
                && c["expires_at_unix_ms"].as_i64().is_none_or(|e| e > now)
        })
    }) {
        return Some("credential_unusable");
    }
    let stream = match &a.scope {
        AuthorizationScope::Workstream { workstream_id, .. } => workstream_id.as_str(),
        AuthorizationScope::Task { .. } => {
            let o = &s["task_ownership"];
            if o["definition_state"] != "enabled"
                || o["workstream_id"] != o["snapshot_workstream_id"]
                || o["ownership_version"] != o["snapshot_ownership_version"]
            {
                return Some("scope_stale");
            }
            match o["workstream_id"].as_str() {
                Some(v) => v,
                None => return Some("scope_stale"),
            }
        }
        _ => return Some("scope_unsupported"),
    };
    let Ok(catalog) = serde_json::from_value::<WorkstreamCatalog>(access["catalog"].clone()) else {
        return Some("scope_stale");
    };
    let Some(definition) = catalog
        .workstreams
        .iter()
        .find(|w| w.id.to_string() == stream)
    else {
        return Some("scope_stale");
    };
    if catalog.project_id != p.project_id || definition.state != awr_core::WorkstreamState::Active {
        return Some("workstream_inactive");
    }
    let Some(grant) = access["grants"].as_array().and_then(|gs| {
        gs.iter().find(|g| {
            g["workstream_id"] == stream
                && g["active"] == true
                && g["read"] == true
                && g["authority_version"] == definition.authority_version.to_string()
        })
    }) else {
        return Some("grant_stale");
    };
    if ["manage", "attest_execution", "reconcile_execution"]
        .iter()
        .any(|k| grant[k] == true)
    {
        return Some("grant_elevated");
    }
    let mapped = crate::tmcp_actions_for_authorized_set(&a.actions);
    let mut allowed = crate::intersect_delegation_with_template(role, &mapped);
    if access["membership"]["agent_review"] == true
        && mapped.contains(&awr_team::Action::ReviewDecide)
    {
        allowed.insert(awr_team::Action::ReviewDecide);
    }
    if !mapped.is_subset(&allowed)
        || (mapped.iter().any(|x| *x != awr_team::Action::WorkRead) && grant["write"] != true)
    {
        return Some("action_not_effective");
    }
    None
}

fn validate_initial(p: &AgentProvisionPlan, s: &Value, now: i64) -> PgResult<()> {
    if access_reason(p, s, now).is_some() || s["access"]["membership"].is_null() {
        return Err(PgError::Forbidden);
    }
    if !s["bindings"].as_array().is_some_and(Vec::is_empty)
        || s["authorizations"].as_array().is_none_or(|xs| {
            xs.iter()
                .any(|x| x["id"] == p.authorization.id || x["status"] == "active")
        })
        || !issue_time_valid(p.authorization.created_at_ms, now)
        || !p.authorization.is_effective_at(now)
    {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}

fn configuration_reason(p: &AgentProvisionPlan, s: &Value, now: i64) -> Option<&'static str> {
    if s["access"]["membership"].is_null() {
        return Some("membership_missing");
    }
    if let Some(reason) = access_reason(p, s, now) {
        return Some(reason);
    }
    if s["person"].is_null() {
        return Some("person_missing");
    }
    let Some(bs) = s["bindings"].as_array() else {
        return Some("binding_missing");
    };
    if bs.len() != 1 {
        return Some(if bs.is_empty() {
            "binding_missing"
        } else {
            "binding_ambiguous"
        });
    }
    let b = &bs[0];
    let a = &p.authorization;
    if b["id"].as_str() != a.binding_id.as_deref()
        || b["person_id"] != a.responsible_person_id.as_str()
        || b["agent_id"] != a.subject_id
        || b["status"] != "active"
    {
        return Some("binding_mismatch");
    }
    let Some(auths) = s["authorizations"].as_array() else {
        return Some("authorization_missing");
    };
    let Some(stored) = auths.iter().find(|x| x["id"] == a.id) else {
        return Some("authorization_missing");
    };
    let Ok(auth) = serde_json::from_value::<AgentAuthorization>(stored.clone()) else {
        return Some("authorization_invalid");
    };
    if !auth.is_effective_at(now) || auth.created_at_ms > now {
        return Some("authorization_inactive");
    }
    for other in auths.iter().filter(|x| x["id"] != a.id) {
        let Ok(other) = serde_json::from_value::<AgentAuthorization>(other.clone()) else {
            return Some("authorization_invalid");
        };
        if other.is_effective_at(now) {
            return Some("authorization_ambiguous");
        }
    }
    if &auth != a {
        return Some("authorization_identity_mismatch");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn issue_time_has_exact_bounds() {
        assert!(issue_time_valid(100, 100));
        assert!(!issue_time_valid(101, 100));
        assert!(issue_time_valid(100, 100 + MAX_ISSUE_AGE_MS));
        assert!(!issue_time_valid(100, 101 + MAX_ISSUE_AGE_MS));
    }
}
