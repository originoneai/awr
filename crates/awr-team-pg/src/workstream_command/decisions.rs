//! Version-bound ordinary questions. An answer records a decision and grants no execution rights.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    session_id: String,
    expected_session_version: String,
    claim_id: String,
    expected_fence: String,
    expected_lease_version: String,
    request_key: String,
    question: String,
    impact: String,
    allowed_responder_person_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resolve {
    session_id: String,
    expected_session_version: String,
    decision_id: String,
    expected_decision_version: String,
    reply: String,
}

pub(super) enum Action {
    Request(Request),
    Respond(Resolve),
    Cancel(Resolve),
}

fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.contains('\0')
}

impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        if op == "decision.request" {
            let a: Request = serde_json::from_value(args).map_err(|_| invalid())?;
            if !identity(&a.session_id)
                || version(&a.expected_session_version)? == 0
                || !identity(&a.claim_id)
                || version(&a.expected_fence)? == 0
                || version(&a.expected_lease_version)? == 0
                || !identity(&a.request_key)
                || !text(&a.question, 2048)
                || !text(&a.impact, 2048)
                || a.allowed_responder_person_ids.is_empty()
                || a.allowed_responder_person_ids.len() > 20
                || a.allowed_responder_person_ids
                    .iter()
                    .any(|id| !identity(id))
                || a.allowed_responder_person_ids
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != a.allowed_responder_person_ids.len()
            {
                return Err(invalid());
            }
            return Ok(Self::Request(a));
        }
        let a: Resolve = serde_json::from_value(args).map_err(|_| invalid())?;
        if !identity(&a.session_id)
            || version(&a.expected_session_version)? == 0
            || !identity(&a.decision_id)
            || version(&a.expected_decision_version)? == 0
            || !text(&a.reply, 4096)
        {
            return Err(invalid());
        }
        match op {
            "decision.respond" => Ok(Self::Respond(a)),
            "decision.cancel" => Ok(Self::Cancel(a)),
            _ => Err(invalid()),
        }
    }
}

/// Only selected contracts, responsibility and verified required sources enter the baseline.
/// Unrelated project audit revisions and source snapshots do not invalidate an answer.
pub(crate) async fn basis(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
) -> PgResult<Value> {
    let (binding, ownership) = work_binding(tx, tenant, project, auth, work).await?;
    let row = tx
        .query_one(
            "SELECT contract_hash FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &work],
        )
        .await?;
    let (_, mut sources, reasons) = crate::workstream_read::controlled_prepare_context(
        tx,
        tenant,
        project,
        auth,
        binding.workstream_id,
    )
    .await?;
    sources.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let responsibility = crate::responsibility::current(tx, tenant, project, work).await?;
    Ok(
        json!({"contract_hash":row.get::<_,String>(0),"ownership_version":ownership.to_string(),
        "workstream_id":binding.workstream_id,"responsibility_version":responsibility.version.to_string(),
        "authority_version":auth.catalog.get(binding.workstream_id)?.authority_version.to_string(),
        "coordinator_epoch":auth.epoch,
        "sources":sources,"complete":reasons.is_empty()}),
    )
}

pub(crate) fn basis_digest(basis: &Value) -> PgResult<String> {
    awr_team::request_hash(basis).map_err(|_| PgError::SourceDivergence)
}

async fn active_responder(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    person: &str,
) -> PgResult<()> {
    let active: bool = tx.query_one("SELECT EXISTS(
        SELECT 1 FROM awr_team.actors a JOIN awr_team.project_memberships m
          ON m.tenant_id=a.tenant_id AND m.actor_id=a.id
        LEFT JOIN awr_team.persons p ON p.tenant_id=m.tenant_id AND p.project_id=m.project_id AND p.id=$3
        WHERE a.tenant_id=$1 AND m.project_id=$2 AND a.status='active'
          AND (p.status IS NULL OR p.status='active')
          AND ((a.kind='human' AND a.id=$3) OR EXISTS(
            SELECT 1 FROM awr_team.person_agent_bindings b
            WHERE b.tenant_id=$1 AND b.project_id=$2 AND b.person_id=$3 AND b.agent_id=a.id AND b.status='active')))",
        &[&tenant,&project,&person]).await?.get(0);
    if !active {
        return Err(PgError::Forbidden);
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
    let (session_id, session_version) = match &action {
        Action::Request(a) => (&a.session_id, &a.expected_session_version),
        Action::Respond(a) | Action::Cancel(a) => (&a.session_id, &a.expected_session_version),
    };
    session(
        tx,
        tenant,
        project,
        auth,
        command,
        session_id,
        session_version,
        ownership,
    )
    .await?;
    let actor = task_intake::actor_instance(tx, tenant, project, auth).await?;
    let person = actor.person_id().as_str();
    let current = basis(tx, tenant, project, auth, &command.work_id).await?;
    let current_digest = basis_digest(&current)?;
    let data = match action {
        Action::Request(mut a) => {
            task_intake::require_enabled(tx, tenant, project, auth, &command.work_id).await?;
            if current["complete"] != true {
                return Err(PgError::PreconditionsChanged);
            }
            a.allowed_responder_person_ids.sort();
            let intent = json!({"question":a.question,"impact":a.impact,
                "requester_person_id":person,"allowed_responder_person_ids":a.allowed_responder_person_ids});
            let request_digest = basis_digest(&intent)?;
            let prior = tx.query_opt("SELECT id,decision_json FROM awr_team.wait_items
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND decision_key=$4 AND decision_baseline_digest=$5",
                &[&tenant,&project,&command.work_id,&a.request_key,&current_digest]).await?;
            if let Some(row) = prior {
                if row.get::<_, Value>(1)["request_digest"] != request_digest {
                    return Err(PgError::IdempotencyConflict);
                }
                json!({"decision_id":row.get::<_,String>(0),"deduplicated":true})
            } else {
                claims::require_live(
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
                for responder in &a.allowed_responder_person_ids {
                    active_responder(tx, tenant, project, responder).await?;
                }
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
                let id = crate::tx::new_id();
                let metadata = json!({"basis":current,"request_digest":request_digest,
                    "impact":a.impact,"allowed_responder_person_ids":a.allowed_responder_person_ids,
                    "requester":{"actor_id":auth.actor_id,"client_id":auth.client_id,"person_id":person},
                    "source_snapshot_id":auth.snapshot});
                tx.execute("INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,
                    question,state,decision_key,decision_baseline_digest,decision_json,decision_version)
                    VALUES($1,$2,$3,$4,$5,$6,'open',$7,$8,$9,1)",
                    &[&tenant,&project,&id,&a.session_id,&command.work_id,&a.question,
                      &a.request_key,&current_digest,&metadata]).await?;
                json!({"decision_id":id,"deduplicated":false})
            }
        }
        Action::Respond(a) | Action::Cancel(a) => {
            let cancel = command.op == "decision.cancel";
            let row = tx.query_opt("SELECT state,decision_version,decision_baseline_digest,decision_json
                FROM awr_team.wait_items WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND id=$4
                  AND decision_key IS NOT NULL FOR UPDATE",
                &[&tenant,&project,&command.work_id,&a.decision_id]).await?.ok_or(PgError::Forbidden)?;
            let metadata: Value = row.get(3);
            let listed = metadata["allowed_responder_person_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id == person));
            let requester = metadata["requester"]["person_id"] == person;
            let responsibility =
                crate::responsibility::current(tx, tenant, project, &command.work_id).await?;
            let owner = responsibility
                .owner
                .as_ref()
                .is_some_and(|id| id.as_str() == person);
            let supervisor = crate::workstream_auth::authorize_domain_action(
                auth,
                awr_team::Action::WorkAssign,
                Some(command.workstream_id),
                Some(&command.work_id),
            )
            .is_ok();
            if !listed && !(cancel && (requester || owner || supervisor)) {
                return Err(PgError::Forbidden);
            }
            if row.get::<_, String>(0) != "open" {
                json!({"decision_id":a.decision_id,"applied":false,"already_resolved":true})
            } else {
                let old_version: i64 = row.get(1);
                if old_version != version(&a.expected_decision_version)? || old_version == i64::MAX
                {
                    return Err(PgError::PreconditionsChanged);
                }
                if !cancel
                    && (current["complete"] != true || row.get::<_, String>(2) != current_digest)
                {
                    return Err(PgError::PreconditionsChanged);
                }
                let at: i64 = tx
                    .query_one(
                        "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
                        &[],
                    )
                    .await?
                    .get(0);
                let resolution = json!({"actor_id":auth.actor_id,"client_id":auth.client_id,
                    "person_id":person,"session_id":a.session_id,"at_unix_ms":at,
                    "request_id":command.request_id,"kind":if cancel {"cancelled"} else {"replied"}});
                let next = old_version + 1;
                tx.execute(
                    "UPDATE awr_team.wait_items SET state=$5,reply=$6,decision_version=$7,
                    decision_json=jsonb_set(decision_json,'{resolution}',$8)
                    WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND id=$4",
                    &[
                        &tenant,
                        &project,
                        &command.work_id,
                        &a.decision_id,
                        &if cancel { "cancelled" } else { "replied" },
                        &a.reply,
                        &next,
                        &resolution,
                    ],
                )
                .await?;
                json!({"decision_id":a.decision_id,"applied":true,"already_resolved":false})
            }
        }
    };
    let id = data["decision_id"]
        .as_str()
        .ok_or(PgError::SourceDivergence)?
        .to_owned();
    let mut data = data;
    data["decision"] = record(tx, tenant, project, &command.work_id, &id, &current_digest).await?;
    data["execution_authorized"] = json!(false);
    data["approval_granted"] = json!(false);
    Ok(Applied {
        data,
        preceding_events: vec![],
    })
}

pub(crate) async fn record(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    id: &str,
    current_digest: &str,
) -> PgResult<Value> {
    let row = tx.query_one("SELECT id,session_id,question,reply,state,decision_key,
        decision_version,decision_json,decision_baseline_digest,
        (extract(epoch FROM created_at)*1000)::bigint AS created_at
        FROM awr_team.wait_items WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND id=$4 AND decision_key IS NOT NULL",
        &[&tenant,&project,&work,&id]).await?;
    let metadata: Value = row.get("decision_json");
    let allowed = metadata["allowed_responder_person_ids"]
        .as_array()
        .ok_or(PgError::SourceDivergence)?;
    let mut person_ids = allowed
        .iter()
        .map(|person| {
            person
                .as_str()
                .map(str::to_owned)
                .ok_or(PgError::SourceDivergence)
        })
        .collect::<PgResult<BTreeSet<_>>>()?;
    let mut requester = metadata["requester"].clone();
    let mut resolution = metadata["resolution"].clone();
    for person in [&requester["person_id"], &resolution["person_id"]]
        .into_iter()
        .filter_map(Value::as_str)
    {
        person_ids.insert(person.to_owned());
    }
    let people = tx.query("SELECT chosen.id,COALESCE(p.display_name,a.display_name) FROM unnest($3::text[]) chosen(id)
        LEFT JOIN awr_team.persons p ON p.tenant_id=$1 AND p.project_id=$2 AND p.id=chosen.id
        LEFT JOIN awr_team.actors a ON a.tenant_id=$1 AND a.id=chosen.id AND EXISTS(
            SELECT 1 FROM awr_team.project_memberships m WHERE m.tenant_id=$1 AND m.project_id=$2 AND m.actor_id=a.id)",
        &[&tenant,&project,&person_ids.into_iter().collect::<Vec<_>>()]).await?;
    let names = people
        .iter()
        .map(|person| {
            (
                person.get::<_, String>(0),
                person
                    .get::<_, Option<String>>(1)
                    .map(|name| name.chars().take(128).collect::<String>()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for actor in [&mut requester, &mut resolution] {
        if let Some(id) = actor["person_id"].as_str() {
            actor["display_name"] = json!(names.get(id).cloned().flatten());
        }
    }
    let responders = allowed
        .iter()
        .map(|person| {
            json!({"person_id":person,
        "display_name":person.as_str().and_then(|id|names.get(id)).cloned().flatten()})
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"decision_id":row.get::<_,String>("id"),"work_id":work,
        "session_id":row.get::<_,String>("session_id"),"request_key":row.get::<_,String>("decision_key"),
        "question":row.get::<_,String>("question"),"impact":metadata["impact"],
        "reply":row.get::<_,Option<String>>("reply"),"state":row.get::<_,String>("state"),
        "decision_version":row.get::<_,i64>("decision_version").to_string(),
        "contract_hash":metadata["basis"]["contract_hash"],
        "baseline_matches_current":row.get::<_,String>("decision_baseline_digest")==current_digest,
        "requester":requester,"allowed_responder_person_ids":metadata["allowed_responder_person_ids"],
        "allowed_responders":responders,
        "resolution":resolution,"created_at_unix_ms":row.get::<_,i64>("created_at"),
        "source_snapshot_id":metadata["source_snapshot_id"],"approval_granted":false}),
    )
}
