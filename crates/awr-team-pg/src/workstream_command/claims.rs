//! Coordination ownership under the authenticated command transaction. A claim
//! is not dependency admission, a resource lease, or permission to run effects.
use super::*;
use tokio_postgres::Row;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Acquire {
    session_id: String,
    expected_session_version: String,
    expected_work_version: String,
    ttl_seconds: i32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Renew {
    session_id: String,
    expected_session_version: String,
    claim_id: String,
    expected_fence: String,
    expected_lease_version: String,
    ttl_seconds: i32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Release {
    session_id: String,
    expected_session_version: String,
    claim_id: String,
    expected_fence: String,
    expected_lease_version: String,
}
pub(super) enum Action {
    Acquire(Acquire),
    Renew(Renew),
    Release(Release),
}
impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        let result = match op {
            "claim.acquire" => {
                let a: Acquire = serde_json::from_value(args).map_err(|_| invalid())?;
                version(&a.expected_work_version)?;
                ttl(a.ttl_seconds)?;
                Self::Acquire(a)
            }
            "claim.renew" => {
                let a: Renew = serde_json::from_value(args).map_err(|_| invalid())?;
                claim_identity(&a.claim_id, &a.expected_fence, &a.expected_lease_version)?;
                ttl(a.ttl_seconds)?;
                Self::Renew(a)
            }
            "claim.release" => {
                let a: Release = serde_json::from_value(args).map_err(|_| invalid())?;
                claim_identity(&a.claim_id, &a.expected_fence, &a.expected_lease_version)?;
                Self::Release(a)
            }
            _ => return Err(invalid()),
        };
        let (session, expected) = result.session();
        if !identity(session) || version(expected)? == 0 {
            return Err(invalid());
        }
        Ok(result)
    }
    fn session(&self) -> (&str, &str) {
        match self {
            Self::Acquire(a) => (&a.session_id, &a.expected_session_version),
            Self::Renew(a) => (&a.session_id, &a.expected_session_version),
            Self::Release(a) => (&a.session_id, &a.expected_session_version),
        }
    }
    pub(super) fn requires_active_stream(&self) -> bool {
        !matches!(self, Self::Release(_))
    }
}
fn ttl(seconds: i32) -> PgResult<()> {
    if !(1..=3600).contains(&seconds) {
        return Err(invalid());
    }
    Ok(())
}
fn claim_identity(id: &str, fence: &str, lease: &str) -> PgResult<()> {
    if !identity(id) || version(fence)? == 0 || version(lease)? == 0 {
        return Err(invalid());
    }
    Ok(())
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    action: Action,
) -> PgResult<Applied> {
    let (session_id, expected_session_version) = action.session();
    session(
        tx,
        tenant,
        project,
        auth,
        command,
        session_id,
        expected_session_version,
        ownership,
    )
    .await?;
    if action.requires_active_stream() {
        let enabled: bool = tx.query_one("SELECT c.definition_state='enabled' AND s.status='active'
            FROM awr_team.work_contracts c JOIN awr_team.work_scopes s
              ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.scope_id
            WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main' AND c.work_id=$4",
            &[&tenant,&project,&auth.snapshot,&command.work_id]).await?.get(0);
        if !enabled {
            return Err(PgError::PreconditionsChanged);
        }
    }
    let mut applied = match action {
        Action::Acquire(a) => acquire(tx, tenant, project, auth, command, ownership, a).await,
        Action::Renew(a) => {
            let r = owned(
                tx,
                tenant,
                project,
                auth,
                command,
                ownership,
                &a.session_id,
                &a.claim_id,
                &a.expected_fence,
                &a.expected_lease_version,
            )
            .await?;
            if !r.get::<_, bool>(10) {
                return Err(PgError::LeaseExpired);
            }
            // Check the clock again in the update; waiting or evaluating guards
            // must never resurrect a lease that expired after the first read.
            let updated = tx.query_opt("UPDATE awr_team.claims
                SET lease_version=lease_version+1,expires_at=clock_timestamp()+make_interval(secs=>$4)
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                  AND state='active' AND expires_at>clock_timestamp()
                RETURNING lease_version,expires_at::text",
                &[&tenant,&project,&a.claim_id,&f64::from(a.ttl_seconds)]).await?.ok_or(PgError::LeaseExpired)?;
            Ok(Applied {
                data: json!({"claim_id":a.claim_id,"session_id":a.session_id,
                "fence":r.get::<_,i64>(3).to_string(),"lease_version":updated.get::<_,i64>(0).to_string(),
                "expires_at":updated.get::<_,String>(1),"state":"active"}),
                preceding_events: vec![],
            })
        }
        Action::Release(a) => {
            let r = owned(
                tx,
                tenant,
                project,
                auth,
                command,
                ownership,
                &a.session_id,
                &a.claim_id,
                &a.expected_fence,
                &a.expected_lease_version,
            )
            .await?;
            require_resolved_effects(tx, tenant, project, &command.work_id).await?;
            // Expiry alone never resolves an execution or an unknown resource.
            // Once effects are resolved, the original owner may close even an
            // elapsed lease without first extending its authority.
            tx.execute(
                "UPDATE awr_team.claims SET state='released',lease_version=lease_version+1
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &a.claim_id],
            )
            .await?;
            let updated = tx.query_opt("UPDATE awr_team.work_runtime
                SET work_version=work_version+1,state=CASE WHEN state='claimed' THEN 'unclaimed' ELSE state END
                WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3
                  AND work_version>=0 AND work_version<9223372036854775807 RETURNING work_version",
                &[&tenant,&project,&command.work_id]).await?.ok_or(PgError::PreconditionsChanged)?;
            Ok(Applied {
                data: json!({"claim_id":a.claim_id,"session_id":a.session_id,
                "fence":r.get::<_,i64>(3).to_string(),"lease_version":(r.get::<_,i64>(4)+1).to_string(),
                "expires_at":r.get::<_,String>(5),"state":"released","work_version":updated.get::<_,i64>(0).to_string(),
                "resource_release_performed":false}),
                preceding_events: vec![],
            })
        }
    }?;
    if matches!(command.op.as_str(), "claim.acquire" | "claim.renew") {
        applied.data["lease_guidance"] = lease_guidance(
            applied.data["claim_id"].as_str().unwrap(),
            applied.data["fence"].as_str().unwrap(),
            applied.data["lease_version"].as_str().unwrap(),
            applied.data["expires_at"].as_str().unwrap(),
        );
    }
    Ok(applied)
}

pub(super) fn lease_guidance(claim: &str, fence: &str, lease: &str, expires_at: &str) -> Value {
    json!({
        "condition": "While the matching owned claim remains live.",
        "basis": {"claim_id":claim,"fence":fence,"lease_version":lease,"expires_at":expires_at},
        "next_action": "Renew with claim.renew before expires_at if work continues; use execution.report when stopped. After expiry, inspect the run and obtain a fresh claim before new admission.",
        "recheck": "Before admission/report; after renewal, disconnection or ownership/contract change."
    })
}

pub(super) async fn require_resolved_effects(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<()> {
    let unresolved:bool=tx.query_one("SELECT
        EXISTS(SELECT 1 FROM awr_team.work_runtime WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND recovery_blocked) OR
        EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
            AND state NOT IN ('succeeded','failed','cancelled')) OR
        EXISTS(SELECT 1 FROM awr_team.resource_reservations WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown')",
        &[&tenant,&project,&work]).await?.get(0);
    if unresolved {
        return Err(PgError::RecoveryBlocked);
    }
    Ok(())
}

async fn acquire(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    a: Acquire,
) -> PgResult<Applied> {
    super::task_intake::require_admissible(tx, tenant, project, auth, command).await?;
    super::task_intake::require_executor(tx, tenant, project, auth, command).await?;
    let waiting: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.wait_items
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='open')",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    if waiting {
        return Err(PgError::WaitOpen);
    }
    let runtime=tx.query_opt("SELECT work_version,last_fence,state,selected_completion_id
        FROM awr_team.work_runtime WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3 FOR UPDATE",
        &[&tenant,&project,&command.work_id]).await?;
    let (work_version, fence) = runtime
        .as_ref()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, i64>(1)))
        .unwrap_or((0, 0));
    if work_version != version(&a.expected_work_version)?
        || work_version == i64::MAX
        || fence == i64::MAX
        || fence < 0
        || runtime.as_ref().is_some_and(|r| {
            matches!(
                r.get::<_, String>(2).as_str(),
                "completed" | "cancelled" | "archived"
            ) || r.get::<_, Option<String>>(3).is_some()
        })
    {
        return Err(PgError::PreconditionsChanged);
    }
    let active=tx.query_opt("SELECT id,workstream_id,ownership_version,coordinator_epoch,expires_at>clock_timestamp(),fence
        FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3 AND state='active' FOR UPDATE",
        &[&tenant,&project,&command.work_id]).await?;
    let mut events = vec![];
    if let Some(r) = active {
        // Reassignment and restore require an explicit recovery/migration path;
        // an elapsed old row must not be attributed to the new workstream.
        if r.get::<_, Option<String>>(1) != Some(command.workstream_id.to_string())
            || r.get::<_, Option<i64>>(2) != Some(ownership)
            || r.get::<_, Option<String>>(3) != Some(auth.epoch.clone())
            || r.get::<_, i64>(5) != fence
        {
            return Err(PgError::RecoveryBlocked);
        }
        if r.get::<_, bool>(4) {
            return Err(PgError::ClaimHeld);
        }
        let id: String = r.get(0);
        tx.execute("UPDATE awr_team.claims SET state='expired' WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant,&project,&id]).await?;
        events.push((
            "claim.expired",
            json!({"claim_id":id,"fence":fence.to_string()}),
        ));
    }
    tx.execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
        VALUES($1,$2,'main',$3,'claimed',$4,$5) ON CONFLICT(tenant_id,project_id,scope_id,work_id)
        DO UPDATE SET state='claimed',work_version=$4,last_fence=$5",
        &[&tenant,&project,&command.work_id,&(work_version+1),&(fence+1)]).await?;
    let id = crate::tx::new_id();
    let r=tx.query_one("INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,
        fence,lease_version,expires_at,state,workstream_id,ownership_version,coordinator_epoch)
        VALUES($1,$2,$3,'main',$4,$5,$6,$7,1,clock_timestamp()+make_interval(secs=>$8),'active',$9,$10,$11)
        RETURNING expires_at::text",
        &[&tenant,&project,&id,&command.work_id,&a.session_id,&auth.actor_id,&(fence+1),&f64::from(a.ttl_seconds),
          &command.workstream_id.to_string(),&ownership,&auth.epoch]).await?;
    super::task_intake::bind_claim(tx, tenant, project, auth, command, &id).await?;
    Ok(Applied {
        data: json!({"claim_id":id,"session_id":a.session_id,"fence":(fence+1).to_string(),
        "lease_version":"1","expires_at":r.get::<_,String>(0),"state":"active","work_version":(work_version+1).to_string()}),
        preceding_events: events,
    })
}

async fn load(tx: &Transaction<'_>, tenant: &str, project: &str, id: &str) -> PgResult<Row> {
    tx.query_opt("SELECT c.work_id,c.session_id,c.actor_id,c.fence,c.lease_version,c.expires_at::text,c.state,
        c.workstream_id,c.ownership_version,c.coordinator_epoch,c.expires_at>clock_timestamp(),
        s.actor_id,s.client_id,s.state,s.work_id,s.workstream_id,s.ownership_version,w.last_fence
        FROM awr_team.claims c JOIN awr_team.sessions s
          ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.session_id AND s.scope_id=c.scope_id
        LEFT JOIN awr_team.work_runtime w
          ON w.tenant_id=c.tenant_id AND w.project_id=c.project_id AND w.scope_id=c.scope_id AND w.work_id=c.work_id
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.id=$3 AND c.scope_id='main'",
        &[&tenant,&project,&id]).await?.ok_or(PgError::Forbidden)
}
fn require_binding(r: &Row, work: &str, stream: &str, ownership: i64) -> PgResult<()> {
    if r.get::<_, String>(0) != work
        || r.get::<_, Option<String>>(7).as_deref() != Some(stream)
        || r.get::<_, Option<i64>>(8) != Some(ownership)
        || r.get::<_, String>(14) != work
        || r.get::<_, Option<String>>(15).as_deref() != Some(stream)
        || r.get::<_, Option<i64>>(16) != Some(ownership)
        || r.get::<_, String>(2) != r.get::<_, String>(11)
    {
        return Err(PgError::Forbidden);
    }
    Ok(())
}
async fn owned(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    session: &str,
    id: &str,
    expected_fence: &str,
    expected_lease: &str,
) -> PgResult<Row> {
    let r = load(tx, tenant, project, id).await?;
    require_binding(
        &r,
        &command.work_id,
        &command.workstream_id.to_string(),
        ownership,
    )?;
    if r.get::<_, String>(1) != session
        || r.get::<_, String>(2) != auth.actor_id
        || r.get::<_, String>(12) != auth.client_id
    {
        return Err(PgError::Forbidden);
    }
    if r.get::<_, Option<String>>(9).as_deref() != Some(&auth.epoch) {
        return Err(PgError::EpochChanged);
    }
    let fence = r.get::<_, i64>(3);
    if fence != version(expected_fence)? || r.get::<_, Option<i64>>(17) != Some(fence) {
        return Err(PgError::StaleFence);
    }
    if r.get::<_, i64>(4) != version(expected_lease)? || r.get::<_, i64>(4) == i64::MAX {
        return Err(PgError::PreconditionsChanged);
    }
    if r.get::<_, String>(6) != "active" {
        return Err(PgError::LeaseExpired);
    }
    super::task_intake::require_executor(tx, tenant, project, auth, command).await?;
    Ok(r)
}

pub(super) async fn require_live(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    session: &str,
    claim: &str,
    fence: &str,
    lease: &str,
) -> PgResult<i64> {
    let r = owned(
        tx, tenant, project, auth, command, ownership, session, claim, fence, lease,
    )
    .await?;
    if !r.get::<_, bool>(10) {
        return Err(PgError::LeaseExpired);
    }
    Ok(r.get(3))
}

pub(super) struct TerminalLeaseObservation {
    pub live: bool,
    pub expires_at: String,
    pub observed_at: String,
}

/// Only the opted-in terminal workspace report may relax elapsed lease time.
/// An inactive/replaced claim, ended session or changed fence still fails.
pub(super) async fn current_terminal_lease(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    session: &str,
    claim: &str,
    fence: &str,
    lease: &str,
) -> PgResult<TerminalLeaseObservation> {
    let r = owned(
        tx, tenant, project, auth, command, ownership, session, claim, fence, lease,
    )
    .await?;
    if r.get::<_, String>(13) != "active" {
        return Err(PgError::PreconditionsChanged);
    }
    let observed = tx
        .query_one(
            "WITH observation AS MATERIALIZED (SELECT clock_timestamp() AS at)
         SELECT c.expires_at>o.at,c.expires_at::text,o.at::text
         FROM awr_team.claims c CROSS JOIN observation o
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.id=$3",
            &[&tenant, &project, &claim],
        )
        .await?;
    Ok(TerminalLeaseObservation {
        live: observed.get(0),
        expires_at: observed.get(1),
        observed_at: observed.get(2),
    })
}

pub(crate) async fn inspect(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
    ownership: i64,
    id: &str,
    session: Option<&str>,
) -> PgResult<Value> {
    let r = load(tx, tenant, project, id).await?;
    require_binding(&r, work, stream, ownership)?;
    if session.is_some_and(|s| r.get::<_, String>(1) != s) {
        return Err(PgError::Forbidden);
    }
    let epoch_matches = r.get::<_, Option<String>>(9).as_deref() == Some(&auth.epoch);
    let current_fence = r.get::<_, Option<i64>>(17) == Some(r.get::<_, i64>(3));
    let active = r.get::<_, String>(6) == "active" && r.get::<_, String>(13) == "active";
    // Inspection never grants start permission. Only the original successful
    // execution.start response may set execution_authorized=true (one-time).
    let eligibility = crate::delegation_auth::execution_side_effect_permitted(auth)
        && active
        && r.get::<_, bool>(10)
        && epoch_matches
        && current_fence;
    Ok(json!({
        "claim_id": id,
        "session_id": r.get::<_, String>(1),
        "fence": r.get::<_, i64>(3).to_string(),
        "lease_version": r.get::<_, i64>(4).to_string(),
        "expires_at": r.get::<_, String>(5),
        "state": r.get::<_, String>(6),
        "owned_by_client": r.get::<_, String>(2) == auth.actor_id
            && r.get::<_, String>(12) == auth.client_id,
        "epoch_matches_current": epoch_matches,
        "current_fence": current_fence,
        "lease_live": active && r.get::<_, bool>(10) && epoch_matches && current_fence,
        "execution_authorized": false,
        "execution_eligibility_advisory": eligibility,
    }))
}
