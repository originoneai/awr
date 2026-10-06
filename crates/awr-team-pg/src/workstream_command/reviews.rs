//! Mainline evidence / review / rework / complete commands (WS-018).
mod agent_completion;
mod artifact_input;

use super::*;
use crate::review::{
    evidence_digest, legacy_pr_snapshot, required_dependencies_covered, resolve_person_id,
    self_review_permitted,
};
use awr_team::{CompletionView, EvidenceBundle, EvidenceGrade, ReviewPolicy, current_completion};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubmitEvidence {
    session_id: String,
    expected_session_version: String,
    claimed_trust: Option<String>,
    payload: Value,
    artifact_hex: Option<String>,
    artifact_text: Option<String>,
    input_digest: Option<String>,
    dirty_tree: bool,
    execution_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenReview {
    session_id: String,
    expected_session_version: String,
    evidence_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Decide {
    session_id: String,
    expected_session_version: String,
    round_id: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rework {
    session_id: String,
    expected_session_version: String,
    round_id: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Complete {
    session_id: String,
    expected_session_version: String,
    evidence_id: String,
    requested_policy: Option<String>,
    context_complete: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DecideUnified {
    session_id: String,
    expected_session_version: String,
    round_id: String,
    decision: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegisterPr {
    session_id: String,
    expected_session_version: String,
    repository: String,
    pr_number: i32,
    pr_url: String,
    head_sha: String,
    merge_sha: Option<String>,
    test_evidence_id: Option<String>,
    fact_source: String,
    observed_at: String,
    author_actor_id: Option<String>,
    owner_person_id: Option<String>,
    executor_actor_id: Option<String>,
    gh_submitted: Option<bool>,
    gh_approved: Option<bool>,
    gh_merged: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ObservePr {
    session_id: String,
    expected_session_version: String,
    delivery_id: String,
    expected_head_sha: String,
    fact_source: String,
    observed_at: String,
    gh_approved: Option<bool>,
    gh_merged: Option<bool>,
    merge_sha: Option<String>,
}

pub(super) enum Action {
    Submit(SubmitEvidence),
    Open(OpenReview),
    Accept(Decide),
    Return(Decide),
    Decide(DecideUnified),
    Rework(Rework),
    Complete(Complete),
    RegisterPr(RegisterPr),
    ObservePr(ObservePr),
    SubmitAndRequest(OpenReview),
}

impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        fn parse_args<T: serde::de::DeserializeOwned>(args: Value) -> PgResult<T> {
            if args.is_object() && args.get("expected_session_version").is_none() {
                return Err(PgError::missing_review_session_version());
            }
            serde_json::from_value(args).map_err(|_| invalid())
        }
        let action = match op {
            "evidence.submit" => Self::Submit(parse_args(args)?),
            "review.open" => Self::Open(parse_args(args)?),
            "review.accept" => Self::Accept(parse_args(args)?),
            "review.return" => Self::Return(parse_args(args)?),
            "review.decide" => Self::Decide(parse_args(args)?),
            "work.rework" => Self::Rework(parse_args(args)?),
            "work.complete" | "delivery.finalize" => Self::Complete(parse_args(args)?),
            "delivery.register_pr" => Self::RegisterPr(parse_args(args)?),
            "delivery.observe_pr" => Self::ObservePr(parse_args(args)?),
            "delivery.submit_and_request_review" => Self::SubmitAndRequest(parse_args(args)?),
            _ => return Err(invalid()),
        };
        let (s, v) = action.session();
        if !identity(s) || version(v)? == 0 {
            return Err(invalid());
        }
        Ok(action)
    }
    fn session(&self) -> (&str, &str) {
        match self {
            Self::Submit(a) => (&a.session_id, &a.expected_session_version),
            Self::Open(a) | Self::SubmitAndRequest(a) => {
                (&a.session_id, &a.expected_session_version)
            }
            Self::Accept(a) | Self::Return(a) => (&a.session_id, &a.expected_session_version),
            Self::Decide(a) => (&a.session_id, &a.expected_session_version),
            Self::Rework(a) => (&a.session_id, &a.expected_session_version),
            Self::Complete(a) => (&a.session_id, &a.expected_session_version),
            Self::RegisterPr(a) => (&a.session_id, &a.expected_session_version),
            Self::ObservePr(a) => (&a.session_id, &a.expected_session_version),
        }
    }
    pub(super) fn requires_active_stream(&self) -> bool {
        !matches!(self, Self::Return(_) | Self::Rework(_))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod input_guidance_tests {
    use super::*;

    #[test]
    fn review_actions_require_an_explicit_session_version() {
        let cases = [
            ("evidence.submit", json!({"payload":{},"dirty_tree":false})),
            ("review.open", json!({"evidence_id":"evidence"})),
            (
                "review.accept",
                json!({"round_id":"round","reason":"reviewed"}),
            ),
            (
                "review.return",
                json!({"round_id":"round","reason":"revise"}),
            ),
            (
                "review.decide",
                json!({"round_id":"round","reason":"reviewed","decision":"approve"}),
            ),
            ("work.rework", json!({"round_id":"round","note":"revised"})),
            (
                "work.complete",
                json!({"evidence_id":"evidence","context_complete":true}),
            ),
            (
                "delivery.finalize",
                json!({"evidence_id":"evidence","context_complete":true}),
            ),
            (
                "delivery.submit_and_request_review",
                json!({"evidence_id":"evidence"}),
            ),
            (
                "delivery.register_pr",
                json!({"repository":"example/project","pr_number":1,
                "pr_url":"https://github.com/example/project/pull/1","head_sha":"a".repeat(40),
                "fact_source":"operator_recorded_observation","observed_at":"2026-01-01T00:00:00Z"}),
            ),
            (
                "delivery.observe_pr",
                json!({"delivery_id":"delivery","expected_head_sha":"a".repeat(40),
                "fact_source":"operator_recorded_observation","observed_at":"2026-01-01T00:00:00Z"}),
            ),
        ];
        for (op, mut args) in cases {
            args["session_id"] = json!("private-session-sentinel");
            let error = Action::parse(op, args.clone()).err().unwrap();
            assert!(error.is_missing_review_session_version(), "{op}");
            assert!(!error.to_string().contains("private-session-sentinel"));
            args["expected_session_version"] = json!("2");
            let action = Action::parse(op, args.clone()).ok().unwrap();
            assert_eq!(action.session(), ("private-session-sentinel", "2"));
            for invalid_version in [
                json!(2),
                Value::Null,
                json!("0"),
                json!("02"),
                json!("unknown"),
            ] {
                args["expected_session_version"] = invalid_version;
                let error = Action::parse(op, args.clone()).err().unwrap();
                assert!(error.is_invalid_command_fields(), "{op}");
            }
            args["expected_session_version"] = json!("2");
            args["unexpected"] = json!("private-value-sentinel");
            let error = Action::parse(op, args).err().unwrap();
            assert!(error.is_invalid_command_fields());
            assert!(!error.to_string().contains("private-value-sentinel"));
        }
        assert!(
            Action::parse("unknown", json!({}))
                .err()
                .unwrap()
                .is_invalid_command_fields()
        );
        assert!(
            Action::parse("evidence.submit", Value::Null)
                .err()
                .unwrap()
                .is_invalid_command_fields()
        );
    }
}

fn trust(actor_kind: &str, claimed: Option<&str>) -> String {
    match actor_kind {
        "system" => "trusted_executor".into(),
        "human" => "human_review".into(),
        _ => {
            let _ = claimed;
            "caller_asserted".into()
        }
    }
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    contract: &awr_team::WorkContract,
    action: Action,
) -> PgResult<Applied> {
    let (sid, ever) = action.session();
    session(tx, tenant, project, auth, command, sid, ever, ownership).await?;
    if action.requires_active_stream() {
        let ok: bool = tx
            .query_one(
                "SELECT c.definition_state='enabled' AND s.status='active'
                 FROM awr_team.work_contracts c
                 JOIN awr_team.work_scopes s
                   ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.scope_id
                 WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3
                   AND c.scope_id='main' AND c.work_id=$4",
                &[&tenant, &project, &auth.snapshot, &command.work_id],
            )
            .await?
            .get(0);
        if !ok {
            return Err(PgError::PreconditionsChanged);
        }
    }
    let contract_hash = contract
        .hash()
        .map_err(|e| PgError::Protocol(e.to_string()))?;
    match action {
        Action::Submit(a) => submit(tx, tenant, project, auth, command, &contract_hash, a).await,
        Action::Open(a) | Action::SubmitAndRequest(a) => {
            open(tx, tenant, project, auth, command, a).await
        }
        Action::Accept(a) => {
            decide(
                tx,
                tenant,
                project,
                auth,
                command,
                &contract_hash,
                a,
                "approve",
            )
            .await
        }
        Action::Return(a) => {
            decide(
                tx,
                tenant,
                project,
                auth,
                command,
                &contract_hash,
                a,
                "reject",
            )
            .await
        }
        Action::Decide(a) => {
            if !matches!(a.decision.as_str(), "approve" | "reject") {
                return Err(invalid());
            }
            let decision = a.decision.clone();
            let mapped = Decide {
                session_id: a.session_id,
                expected_session_version: a.expected_session_version,
                round_id: a.round_id,
                reason: a.reason,
            };
            decide(
                tx,
                tenant,
                project,
                auth,
                command,
                &contract_hash,
                mapped,
                &decision,
            )
            .await
        }
        Action::Rework(a) => rework(tx, tenant, project, auth, command, a).await,
        Action::Complete(a) => {
            complete(
                tx,
                tenant,
                project,
                auth,
                command,
                contract,
                &contract_hash,
                a,
            )
            .await
        }
        Action::RegisterPr(a) => {
            register_pr(tx, tenant, project, auth, command, &contract_hash, a).await
        }
        Action::ObservePr(a) => observe_pr(tx, tenant, project, auth, command, a).await,
    }
}

async fn submit(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    contract_hash: &str,
    a: SubmitEvidence,
) -> PgResult<Applied> {
    let kind: String = tx
        .query_opt(
            "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &auth.actor_id],
        )
        .await?
        .map(|r| r.get(0))
        .ok_or(PgError::Forbidden)?;
    let trust_basis = trust(&kind, a.claimed_trust.as_deref());
    let artifact_bytes =
        artifact_input::decode(a.artifact_hex.as_deref(), a.artifact_text.as_deref())?;
    if a.dirty_tree && a.input_digest.is_none() && artifact_bytes.is_none() {
        return Err(PgError::EvidenceInvalid);
    }
    if a.payload.get("passed").and_then(Value::as_bool) == Some(true)
        && artifact_bytes.is_none()
        && a.payload.get("output_digest").is_none()
    {
        return Err(PgError::EvidenceInvalid);
    }
    if let Some(d) = &a.input_digest {
        if !digest(d) {
            return Err(invalid());
        }
    }
    if let Some(id) = &a.execution_id {
        if !identity(id) {
            return Err(invalid());
        }
        let ok = tx
            .query_opt(
                "SELECT 1 FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND work_id=$4",
                &[&tenant, &project, id, &command.work_id],
            )
            .await?
            .is_some();
        if !ok {
            return Err(PgError::EvidenceInvalid);
        }
    }
    let output_digest = artifact_bytes.as_deref().map(sha256_hex);
    let execution_result_digest = a
        .payload
        .get("output_digest")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let digest_v = evidence_digest(
        &command.work_id,
        contract_hash,
        a.input_digest.as_deref(),
        output_digest.as_deref(),
        execution_result_digest.as_deref(),
        &a.payload,
    )?;
    let mut artifact_id = None;
    if let Some(bytes) = artifact_bytes.as_deref() {
        let id = crate::tx::new_id();
        tx.execute(
            "INSERT INTO awr_team.artifacts(
                tenant_id, project_id, id, object_key, sha256, byte_length,
                media_type, state, created_by, content)
             VALUES ($1,$2,$3,$4,$5,$6,'application/octet-stream','finalized',$7,$8)",
            &[
                &tenant,
                &project,
                &id,
                &format!("evidence/{id}"),
                &sha256_hex(bytes),
                &(bytes.len() as i64),
                &auth.actor_id,
                &bytes,
            ],
        )
        .await?;
        artifact_id = Some(id);
    }
    let id = crate::tx::new_id();
    tx.execute(
        "INSERT INTO awr_team.evidence(
            tenant_id, project_id, id, work_id, execution_id, artifact_id,
            contract_hash, input_digest, output_digest, execution_result_digest,
            evidence_kind, trust_basis, digest, payload_json, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'report',$11,$12,$13,$14)",
        &[
            &tenant,
            &project,
            &id,
            &command.work_id,
            &a.execution_id,
            &artifact_id,
            &contract_hash,
            &a.input_digest,
            &output_digest,
            &execution_result_digest,
            &trust_basis,
            &digest_v,
            &a.payload,
            &auth.actor_id,
        ],
    )
    .await?;
    Ok(Applied {
        data: json!({
            "evidence_id": id,
            "digest": digest_v,
            "artifact_id": artifact_id,
            "artifact_digest": output_digest,
            "trust_basis": trust_basis,
            "contract_hash": contract_hash,
            "execution_success": a.payload.get("passed").and_then(Value::as_bool),
            "author_self_report": trust_basis == "caller_asserted",
            "human_approval": false,
            "task_complete": false,
        }),
        preceding_events: vec![(
            "evidence.recorded",
            json!({
                "evidence_id": id,
                "digest": digest_v,
                "trust_basis": trust_basis,
                "execution_id": a.execution_id,
            }),
        )],
    })
}

async fn open(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    a: OpenReview,
) -> PgResult<Applied> {
    if !identity(&a.evidence_id) {
        return Err(invalid());
    }
    let row = tx
        .query_opt(
            "SELECT work_id, contract_hash, digest, execution_id, output_digest, execution_result_digest
             FROM awr_team.evidence WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &a.evidence_id],
        )
        .await?
        .ok_or(PgError::EvidenceInvalid)?;
    let work_id: String = row.get(0);
    let contract_hash: String = row.get(1);
    let digest_v: String = row.get(2);
    let execution_id: Option<String> = row.get(3);
    let artifact_digest: Option<String> = row.get(4);
    let execution_result_digest: Option<String> = row.get(5);
    if work_id != command.work_id {
        return Err(PgError::EvidenceInvalid);
    }
    // Resolve to a person for independence checks. Unbound agents get a
    // person row keyed by actor id so the FK holds; that person is not a
    // substitute for a real human owner when judging team independence.
    let author_person = match resolve_person_id(tx, tenant, project, &auth.actor_id).await {
        Ok(p) => p,
        Err(_) => {
            tx.execute(
                "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
                 VALUES ($1,$2,$3,$3,'active') ON CONFLICT DO NOTHING",
                &[&tenant, &project, &auth.actor_id],
            )
            .await?;
            tx.execute(
                "INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
                 VALUES ($1,$2,$3,$4,$4,'active') ON CONFLICT DO NOTHING",
                &[&tenant, &project, &crate::tx::new_id(), &auth.actor_id],
            )
            .await?;
            auth.actor_id.clone()
        }
    };
    tx.execute(
        "UPDATE awr_team.review_rounds SET state='invalidated'
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
           AND state IN ('open','approved') AND bundle_hash <> $4",
        &[&tenant, &project, &command.work_id, &digest_v],
    )
    .await?;
    let round_index: i32 = tx
        .query_one(
            "SELECT COALESCE(max(round_index),0)+1 FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    let id = crate::tx::new_id();
    tx.execute(
        "INSERT INTO awr_team.review_rounds(
            tenant_id, project_id, id, work_id, round_index, bundle_hash,
            contract_hash, author_actor_id, state, author_person_id, evidence_id,
            execution_id, artifact_digest, execution_result_digest, author_client_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'open',$9,$10,$11,$12,$13,$14)",
        &[
            &tenant,
            &project,
            &id,
            &command.work_id,
            &round_index,
            &digest_v,
            &contract_hash,
            &auth.actor_id,
            &author_person,
            &a.evidence_id,
            &execution_id,
            &artifact_digest,
            &execution_result_digest,
            &auth.client_id,
        ],
    )
    .await?;
    Ok(Applied {
        data: json!({
            "round_id": id,
            "round_index": round_index,
            "bundle_hash": digest_v,
            "contract_hash": contract_hash,
            "evidence_id": a.evidence_id,
            "execution_id": execution_id,
            "author_person_id": author_person,
            "state": "open",
            "binds_exact_contract_artifact_execution_round": true,
        }),
        preceding_events: vec![(
            "review.opened",
            json!({
                "round_id": id,
                "round_index": round_index,
                "bundle_hash": digest_v,
                "evidence_id": a.evidence_id,
                "author_person_id": author_person,
            }),
        )],
    })
}

async fn decide(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    contract_hash: &str,
    a: Decide,
    decision: &str,
) -> PgResult<Applied> {
    if !identity(&a.round_id) || a.reason.trim().is_empty() || a.reason.len() > 4096 {
        return Err(invalid());
    }
    let row = tx
        .query_opt(
            "SELECT work_id, author_actor_id, bundle_hash, state, round_index, contract_hash,
                    author_person_id, evidence_id, author_client_id
             FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
            &[&tenant, &project, &a.round_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("review round not found".into()))?;
    let work_id: String = row.get(0);
    let author: String = row.get(1);
    let bundle_hash: String = row.get(2);
    let state: String = row.get(3);
    let round_index: i32 = row.get(4);
    let round_contract: String = row.get(5);
    let author_person: Option<String> = row.get(6);
    let evidence_id: Option<String> = row.get(7);
    let author_client: Option<String> = row.get(8);
    if work_id != command.work_id {
        return Err(PgError::Forbidden);
    }
    if state != "open" {
        return Err(PgError::ReviewRequired);
    }
    if round_contract != contract_hash {
        return Err(PgError::ReviewRequired);
    }
    let reviewer_kind: String = tx
        .query_opt(
            "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &auth.actor_id],
        )
        .await?
        .map(|r| r.get(0))
        .ok_or(PgError::Forbidden)?;
    let author_person = match author_person {
        Some(p) => p,
        None => resolve_person_id(tx, tenant, project, &author)
            .await
            .unwrap_or(author.clone()),
    };
    let reviewer_person = resolve_person_id(tx, tenant, project, &auth.actor_id).await?;
    let same_person = author_person == reviewer_person || auth.actor_id == author;
    let live_policy: String = tx
        .query_opt(
            "SELECT contract_json->>'completion_policy'
             FROM awr_team.work_contracts
             WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3
               AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &command.work_id],
        )
        .await?
        .and_then(|r| r.get::<_, Option<String>>(0))
        .unwrap_or_else(|| "trusted_execution_and_review".into());
    let agent_policy = live_policy == crate::review::AGENT_REVIEW_POLICY;
    let independence_kind = if agent_policy {
        if reviewer_kind != "agent" || command.op != "review.decide" {
            return Err(PgError::Forbidden);
        }
        crate::tx::require_agent_review_grant(tx, tenant, project, &auth.actor_id).await?;
        // Person equality is allowed only because this is explicitly Agent
        // review. Both authenticated execution identities must still differ.
        if auth.actor_id == author || author_client.as_deref().is_none_or(|c| c == auth.client_id) {
            return Err(PgError::AuthorCannotReview);
        }
        // Opening somebody else's bundle cannot hide its actual author or
        // executor from the Agent self-review check.
        let self_authored: bool = tx
            .query_opt(
                "SELECT e.created_by=$4 OR COALESCE(x.executor_actor_id=$4, false)
             FROM awr_team.evidence e LEFT JOIN awr_team.executions x
               ON x.tenant_id=e.tenant_id AND x.project_id=e.project_id AND x.id=e.execution_id
             WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.id=$3",
                &[&tenant, &project, &evidence_id, &auth.actor_id],
            )
            .await?
            .ok_or(PgError::EvidenceInvalid)?
            .get(0);
        if self_authored {
            return Err(PgError::AuthorCannotReview);
        }
        "agent_review"
    } else {
        if reviewer_kind == "agent" {
            return Err(PgError::AuthorCannotReview);
        }
        crate::tx::validate_reviewer(tx, tenant, project, &auth.actor_id).await?;
        crate::tx::require_independent_review_grant(tx, tenant, project, &auth.actor_id).await?;
        if same_person {
            if decision == "approve" && !self_review_permitted(&live_policy) {
                return Err(PgError::AuthorCannotReview);
            }
            "personal_self_review"
        } else {
            "team_independent"
        }
    };
    let approval_basis = crate::review::approval_basis(independence_kind);
    let next = if decision == "approve" {
        "approved"
    } else {
        "rejected"
    };
    tx.execute(
        "INSERT INTO awr_team.review_decisions(
            tenant_id, project_id, id, review_round_id, work_id, bundle_hash,
            reviewer_actor_id, decision, reason, reviewer_person_id, independence_kind,
            reviewer_client_id, approval_basis)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        &[
            &tenant,
            &project,
            &crate::tx::new_id(),
            &a.round_id,
            &work_id,
            &bundle_hash,
            &auth.actor_id,
            &decision,
            &a.reason,
            &reviewer_person,
            &independence_kind,
            &auth.client_id,
            &approval_basis,
        ],
    )
    .await?;
    tx.execute(
        "UPDATE awr_team.review_rounds SET state=$4
         WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant, &project, &a.round_id, &next],
    )
    .await?;
    let team_independent_acceptance =
        independence_kind == "team_independent" && decision == "approve";
    Ok(Applied {
        data: json!({
            "round_id": a.round_id,
            "round_index": round_index,
            "state": next,
            "decision": decision,
            "independence_kind": independence_kind,
            "approval_basis": approval_basis,
            "author_actor_id": author,
            "author_client_id": author_client,
            "reviewer_actor_id": auth.actor_id,
            "reviewer_client_id": auth.client_id,
            "team_independent_acceptance": team_independent_acceptance,
            "author_person_id": author_person,
            "reviewer_person_id": reviewer_person,
            "evidence_id": evidence_id,
            "human_approval": !agent_policy && decision == "approve",
            "task_complete": false,
        }),
        preceding_events: vec![(
            "review.decided",
            json!({
                "round_id": a.round_id,
                "decision": decision,
                "independence_kind": independence_kind,
                "approval_basis": approval_basis,
                "author_actor_id": author,
                "author_client_id": author_client,
                "reviewer_actor_id": auth.actor_id,
                "reviewer_client_id": auth.client_id,
                "human_approval": !agent_policy && decision == "approve",
                "team_independent_acceptance": team_independent_acceptance,
            }),
        )],
    })
}

async fn rework(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    a: Rework,
) -> PgResult<Applied> {
    if !identity(&a.round_id) || a.note.trim().is_empty() || a.note.len() > 4096 {
        return Err(invalid());
    }
    let row = tx
        .query_opt(
            "SELECT work_id, state, round_index FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
            &[&tenant, &project, &a.round_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("review round not found".into()))?;
    let work_id: String = row.get(0);
    let state: String = row.get(1);
    let round_index: i32 = row.get(2);
    if work_id != command.work_id {
        return Err(PgError::Forbidden);
    }
    if state != "rejected" {
        return Err(PgError::Protocol(
            "rework requires a returned/rejected review round".into(),
        ));
    }
    let actor_person = resolve_person_id(tx, tenant, project, &auth.actor_id)
        .await
        .unwrap_or_else(|_| auth.actor_id.clone());
    Ok(Applied {
        data: json!({
            "round_id": a.round_id,
            "round_index": round_index,
            "state": "rejected",
            "rework_acknowledged": true,
            "note": a.note,
            "actor_person_id": actor_person,
            "history_retained": true,
            "task_complete": false,
        }),
        preceding_events: vec![(
            "work.rework",
            json!({
                "round_id": a.round_id,
                "note": a.note,
                "history_retained": true,
            }),
        )],
    })
}

async fn complete(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    contract: &awr_team::WorkContract,
    contract_hash: &str,
    a: Complete,
) -> PgResult<Applied> {
    if !identity(&a.evidence_id) {
        return Err(invalid());
    }
    if !a.context_complete {
        return Err(PgError::ContextIncomplete);
    }
    let unknown: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.executions
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown'",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    if unknown > 0 {
        return Err(PgError::RecoveryBlocked);
    }
    let blocked: bool = tx
        .query_opt(
            "SELECT recovery_blocked FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .map(|r| r.get(0))
        .unwrap_or(false);
    if blocked {
        return Err(PgError::RecoveryBlocked);
    }
    crate::workstream_command::executions::require_clear_of_selective_blocks(
        tx,
        tenant,
        project,
        &command.work_id,
    )
    .await?;
    let policy = contract.completion_policy.as_str();
    let agent_policy = policy == crate::review::AGENT_REVIEW_POLICY;
    if let Some(requested) = a.requested_policy.as_deref() {
        if requested != policy {
            return Err(PgError::PolicyDowngrade);
        }
    }
    // Delegate the heavy gates to ReviewStore-equivalent SQL via a nested
    // call pattern: reuse complete by constructing gates inline.
    // For WS-018 mainline, require an approved review round for this evidence
    // unless ordinary_confirm.
    let ev = tx
        .query_opt(
            "SELECT work_id, contract_hash, digest, trust_basis, payload_json, artifact_id,
                    output_digest, execution_result_digest, input_digest, execution_id, created_by
             FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &a.evidence_id],
        )
        .await?
        .ok_or(PgError::EvidenceInvalid)?;
    let ev_work: String = ev.get(0);
    let ev_contract: String = ev.get(1);
    let ev_digest: String = ev.get(2);
    let trust_basis: String = ev.get(3);
    let payload: Value = ev.get(4);
    let artifact_id: Option<String> = ev.get(5);
    let output_digest: Option<String> = ev.get(6);
    let execution_result_digest: Option<String> = ev.get(7);
    let input_digest: Option<String> = ev.get(8);
    let execution_id: Option<String> = ev.get(9);
    let created_by: String = ev.get(10);
    if ev_work != command.work_id || ev_contract != contract_hash {
        return Err(PgError::EvidenceInvalid);
    }
    let recomputed = evidence_digest(
        &command.work_id,
        contract_hash,
        input_digest.as_deref(),
        output_digest.as_deref(),
        execution_result_digest.as_deref(),
        &payload,
    )?;
    if recomputed != ev_digest {
        return Err(PgError::EvidenceInvalid);
    }
    if let Some(aid) = &artifact_id {
        let row = tx
            .query_opt(
                "SELECT sha256, state, content FROM awr_team.artifacts
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, aid],
            )
            .await?
            .ok_or(PgError::EvidenceInvalid)?;
        let sha: String = row.get(0);
        let state: String = row.get(1);
        let content: Option<Vec<u8>> = row.get(2);
        let content = content.ok_or(PgError::EvidenceInvalid)?;
        if state != "finalized"
            || sha256_hex(&content) != sha
            || output_digest.as_deref() != Some(sha.as_str())
        {
            return Err(PgError::EvidenceInvalid);
        }
    }
    let grade = match trust_basis.as_str() {
        "trusted_executor" => EvidenceGrade::TrustedExecutionReceipt,
        "human_review" => EvidenceGrade::AuthorizedReview,
        _ => EvidenceGrade::AgentSelfReport,
    };
    let mut caller_execution_binding = Value::Null;
    let mut execution_success = false;
    let mut verified_executor_actor: Option<String> = None;
    if policy == "ordinary_confirm" {
        let kind: String = tx
            .query_one(
                "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
                &[&tenant, &auth.actor_id],
            )
            .await?
            .get(0);
        if kind != "human" {
            return Err(PgError::Forbidden);
        }
    } else if if agent_policy {
        trust_basis != "caller_asserted" || artifact_id.is_none() || input_digest.is_none()
    } else {
        grade != EvidenceGrade::TrustedExecutionReceipt
    } {
        return Err(PgError::EvidenceInvalid);
    } else {
        if payload.get("passed").and_then(Value::as_bool) == Some(false) {
            return Err(PgError::EvidenceInvalid);
        }
        let execution_id = execution_id.as_deref().ok_or(PgError::EvidenceInvalid)?;
        let exec = tx
            .query_opt(
                "SELECT state, contract_hash, input_digest, executor_actor_id, scope_id, result_digest,
                        id, work_id, executor_client_id, session_id, workstream_id, ownership_version,
                        environment_digest, observed_paths_json, coordinator_epoch, execution_version,
                        claim_id,fence,declared_scope_json,settlement_policy_json,admission_mode,
                        admission_lease_version,terminal_reported,workspace_effects_settled
                 FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &execution_id],
            )
            .await?
            .ok_or(PgError::EvidenceInvalid)?;
        let exec_state: String = exec.get(0);
        let exec_contract: String = exec.get(1);
        let exec_input: Option<String> = exec.get(2);
        let exec_executor: String = exec.get(3);
        let exec_scope: String = exec.get(4);
        let exec_result: Option<String> = exec.get(5);
        if exec_state != "succeeded" || exec_contract != ev_contract {
            return Err(PgError::EvidenceInvalid);
        }
        if exec_executor != created_by || exec_scope != "main" {
            return Err(PgError::EvidenceInvalid);
        }
        if exec_input != input_digest {
            return Err(PgError::EvidenceInvalid);
        }
        let declared = execution_result_digest
            .as_deref()
            .ok_or(PgError::EvidenceInvalid)?;
        let recorded = exec_result.as_deref().ok_or(PgError::EvidenceInvalid)?;
        if declared != recorded {
            return Err(PgError::EvidenceInvalid);
        }
        if agent_policy {
            caller_execution_binding = agent_completion::verify_execution(
                tx,
                tenant,
                project,
                auth,
                command,
                &exec,
                output_digest.as_deref(),
            )
            .await?;
        }
        execution_success = true;
        verified_executor_actor = Some(exec_executor);
    }
    let contract_value =
        serde_json::to_value(contract).map_err(|e| PgError::Protocol(e.to_string()))?;
    let (binding_valid, dependency_links) = required_dependencies_covered(
        tx,
        tenant,
        project,
        &command.work_id,
        "main",
        &contract_value,
    )
    .await?;
    let mut review = ReviewPolicy {
        required: policy != "ordinary_confirm",
        author_may_self_approve: self_review_permitted(policy),
        approved: false,
        reviewer_is_author: false,
    };
    let mut independence_kind = if policy == "ordinary_confirm" {
        "ordinary_confirm".to_string()
    } else {
        "unspecified".to_string()
    };
    let mut approver_actor: Option<String> = None;
    let mut approver_person: Option<String> = None;
    let mut approver_client: Option<String> = None;
    if policy != "ordinary_confirm" {
        // Only a still-valid (non-invalidated) approved round may satisfy
        // completion. review.open invalidates prior approved rounds when a
        // different evidence digest is opened; keep their decisions for
        // history but refuse to complete on them.
        let pinned = tx
            .query_opt(
                "SELECT id, contract_hash, state, evidence_id, execution_id, artifact_digest,
                        execution_result_digest, author_actor_id, author_client_id
                 FROM awr_team.review_rounds
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND bundle_hash=$4
                 ORDER BY round_index DESC LIMIT 1",
                &[&tenant, &project, &command.work_id, &ev_digest],
            )
            .await?
            .ok_or(PgError::ReviewRequired)?;
        let round_id: String = pinned.get(0);
        let round_contract: String = pinned.get(1);
        let round_state: String = pinned.get(2);
        if round_contract != ev_contract || round_state != "approved" {
            return Err(PgError::ReviewRequired);
        }
        let d = tx
            .query_opt(
                "SELECT decision, reviewer_actor_id, reviewer_person_id, independence_kind, reviewer_client_id, approval_basis
                 FROM awr_team.review_decisions
                 WHERE tenant_id=$1 AND project_id=$2 AND review_round_id=$3
                 ORDER BY created_at DESC LIMIT 1",
                &[&tenant, &project, &round_id],
            )
            .await?
            .ok_or(PgError::ReviewRequired)?;
        let dec: String = d.get(0);
        approver_actor = Some(d.get(1));
        approver_person = d.get(2);
        independence_kind = d.get(3);
        approver_client = d.get(4);
        if agent_policy {
            let author: String = pinned.get(7);
            let author_client: Option<String> = pinned.get(8);
            if independence_kind != "agent_review"
                || d.get::<_, String>(5) != "agent_review"
                || pinned.get::<_, Option<String>>(3).as_deref() != Some(a.evidence_id.as_str())
                || pinned.get::<_, Option<String>>(4) != execution_id
                || pinned.get::<_, Option<String>>(5) != output_digest
                || pinned.get::<_, Option<String>>(6) != execution_result_digest
                || approver_actor.as_deref() == Some(created_by.as_str())
                || approver_actor.as_deref() == Some(author.as_str())
                || approver_client.as_deref().is_none_or(|c| {
                    Some(c) == author_client.as_deref()
                        || Some(c) == caller_execution_binding["executor_client_id"].as_str()
                })
                || author_client.is_none()
            {
                return Err(PgError::ReviewRequired);
            }
        }
        review.approved = dec == "approve";
        if let Some(ap) = tx
            .query_opt(
                "SELECT author_person_id FROM awr_team.review_rounds
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &round_id],
            )
            .await?
            .and_then(|r| r.get::<_, Option<String>>(0))
        {
            if let Some(rp) = &approver_person {
                review.reviewer_is_author = &ap == rp;
            }
        }
        if !review.approved {
            return Err(PgError::ReviewRequired);
        }
    }
    let bundle = EvidenceBundle {
        grade,
        contract_hash: ev_contract.clone(),
        artifact_digest: output_digest.clone(),
        accessible: true,
    };
    // This explicit policy accepts a reconciled caller report plus the exact
    // Agent review checked above. Do not relabel its evidence grade to pass
    // the legacy trusted/human completion classifier.
    let view = if agent_policy {
        if binding_valid && review.approved && execution_success {
            CompletionView::CurrentlyVerified
        } else {
            CompletionView::NeedsRevalidation
        }
    } else {
        current_completion(
            false,
            true,
            Some(ev_contract.as_str()),
            &ev_contract,
            binding_valid,
            Some(&bundle),
            &review,
        )
    };
    if view != CompletionView::CurrentlyVerified {
        return Err(PgError::CompletionRejected);
    }
    let team_independent_acceptance = independence_kind == "team_independent";
    let approval_basis = crate::review::approval_basis(&independence_kind);
    let execution_basis = if agent_policy {
        caller_execution_binding["execution_basis"]
            .as_str()
            .ok_or(PgError::EvidenceInvalid)?
            .to_owned()
    } else if execution_success {
        "trusted_execution_receipt".to_owned()
    } else {
        "not_required".to_owned()
    };
    let human_approval = !agent_policy && review.approved;
    let submitter_person = resolve_person_id(tx, tenant, project, &auth.actor_id)
        .await
        .ok();
    // Active PR delivery attribution (TMCP-031); merge never substitutes for acceptance.
    let pr = tx
        .query_opt(
            "SELECT id, contract_hash, author_actor_id, owner_person_id, executor_actor_id, gh_merged
             FROM awr_team.pr_deliveries
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'
             ORDER BY created_at DESC LIMIT 1",
            &[&tenant, &project, &command.work_id],
        )
        .await?;
    let mut pr_delivery_id: Option<String> = None;
    let mut author_actor: Option<String> = None;
    let mut owner_person: Option<String> = None;
    let mut executor_actor: Option<String> = None;
    if let Some(row) = &pr {
        let pr_contract: String = row.get(1);
        if pr_contract != ev_contract {
            return Err(PgError::PreconditionsChanged);
        }
        pr_delivery_id = Some(row.get(0));
        author_actor = row.get(2);
        owner_person = row.get(3);
        executor_actor = row.get(4);
        let _gh_merged: bool = row.get(5);
        let _ = _gh_merged;
    }
    // Prefer PR attribution when present; otherwise carry verified evidence /
    // execution identities. Never substitute the finalizer or an execution object
    // ID for author/executor person-agent namespaces (TMCP-031 CR).
    let author_actor = author_actor.or_else(|| Some(created_by.clone()));
    let executor_actor = executor_actor.or(verified_executor_actor);
    let approved_by = json!({
        "approved_by": approver_actor,
        "approved_by_person_id": approver_person,
        "submitted_by": auth.actor_id,
        "submitted_by_person_id": submitter_person,
        "author_actor_id": author_actor,
        "owner_person_id": owner_person,
        "executor_actor_id": executor_actor,
        "reviewer_actor_id": approver_actor,
        "final_submitter_actor_id": auth.actor_id,
        "pr_delivery_id": pr_delivery_id,
        "github_merged_does_not_complete": true,
        "independence_kind": independence_kind,
        "team_independent_acceptance": team_independent_acceptance,
        "execution_success": execution_success,
        "author_self_report": trust_basis == "caller_asserted",
        "human_approval": human_approval,
        "approval_basis": approval_basis,
        "execution_basis": execution_basis,
        "reviewer_client_id": approver_client,
        "caller_execution_binding": caller_execution_binding,
    });
    let dependency_binding_hash = sha256_hex(json!(&dependency_links).to_string().as_bytes());
    let receipt_id = crate::tx::new_id();
    let independence_for_receipt = if policy == "ordinary_confirm" {
        "ordinary_confirm"
    } else {
        independence_kind.as_str()
    };
    tx.execute(
        "INSERT INTO awr_team.completion_receipts(
            tenant_id, project_id, id, work_id, scope_id, contract_hash,
            result_digest, dependency_binding_hash, evidence_bundle_hash,
            policy, approved_by_json, independence_kind, evidence_id, execution_id,
            approved_by_person_id, submitted_by_person_id,
            author_actor_id, owner_person_id, executor_actor_id,
            final_submitter_actor_id, pr_delivery_id)
         VALUES ($1,$2,$3,$4,'main',$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)",
        &[
            &tenant,
            &project,
            &receipt_id,
            &command.work_id,
            &ev_contract,
            &ev_digest,
            &dependency_binding_hash,
            &ev_digest,
            &policy,
            &approved_by,
            &independence_for_receipt,
            &a.evidence_id,
            &execution_id,
            &approver_person,
            &submitter_person,
            &author_actor,
            &owner_person,
            &executor_actor,
            &auth.actor_id,
            &pr_delivery_id,
        ],
    )
    .await?;
    tx.execute(
        "INSERT INTO awr_team.completion_evidence(
            tenant_id, project_id, completion_id, evidence_id, criterion_id)
         VALUES ($1,$2,$3,$4,'contract')",
        &[&tenant, &project, &receipt_id, &a.evidence_id],
    )
    .await?;
    for (upstream_work, upstream_receipt) in &dependency_links {
        tx.execute(
            "INSERT INTO awr_team.completion_dependencies(
                tenant_id, project_id, completion_id, predecessor_work_id,
                predecessor_completion_id)
             VALUES ($1,$2,$3,$4,$5)",
            &[
                &tenant,
                &project,
                &receipt_id,
                upstream_work,
                upstream_receipt,
            ],
        )
        .await?;
    }
    tx.execute(
        "INSERT INTO awr_team.work_runtime(
            tenant_id, project_id, scope_id, work_id, state, work_version, last_fence,
            selected_completion_id)
         VALUES ($1,$2,'main',$3,'completed',1,0,$4)
         ON CONFLICT (tenant_id, project_id, scope_id, work_id)
         DO UPDATE SET state='completed', selected_completion_id=$4,
             work_version = awr_team.work_runtime.work_version + 1",
        &[&tenant, &project, &command.work_id, &receipt_id],
    )
    .await?;
    let selected: Option<String> = tx
        .query_one(
            "SELECT selected_completion_id FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    if selected.as_deref() != Some(receipt_id.as_str()) {
        return Err(PgError::CompletionRejected);
    }
    Ok(Applied {
        data: json!({
            "receipt_id": receipt_id,
            "selected_completion_id": receipt_id,
            "contract_hash": ev_contract,
            "policy": policy,
            "independence_kind": independence_for_receipt,
            "team_independent_acceptance": team_independent_acceptance,
            "evidence_id": a.evidence_id,
            "execution_id": execution_id,
            "approved_by_person_id": approver_person,
            "author_actor_id": author_actor,
            "executor_actor_id": executor_actor,
            "reviewer_actor_id": approver_actor,
            "final_submitter_actor_id": auth.actor_id,
            "execution_success": execution_success,
            "author_self_report": trust_basis == "caller_asserted",
            "human_approval": human_approval,
        "approval_basis": approval_basis,
        "execution_basis": execution_basis,
        "reviewer_client_id": approver_client,
        "caller_execution_binding": caller_execution_binding,
            "task_complete": true,
            "provider_private_session": Value::Null,
        }),
        preceding_events: vec![(
            "work.completed",
            json!({
                "receipt_id": receipt_id,
                "evidence_id": a.evidence_id,
                "execution_id": execution_id,
                "independence_kind": independence_for_receipt,
                "approval_basis": approval_basis,
                "execution_basis": execution_basis,
                "human_approval": human_approval,
                "team_independent_acceptance": team_independent_acceptance,
                "approved_by_person_id": approver_person,
            }),
        )],
    })
}

async fn register_pr(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    contract_hash: &str,
    a: RegisterPr,
) -> PgResult<Applied> {
    if a.pr_number <= 0
        || a.repository.trim().is_empty()
        || a.repository.len() > 256
        || a.pr_url.trim().is_empty()
        || a.pr_url.len() > 512
        || a.head_sha.len() != 40
        || !a
            .head_sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || a.merge_sha.as_ref().is_some_and(|s| {
            s.len() != 40
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        || !matches!(
            a.fact_source.as_str(),
            "authorized_human_github_verification" | "operator_recorded_observation"
        )
        || a.observed_at.trim().is_empty()
        || a.observed_at.len() > 64
    {
        return Err(invalid());
    }
    if let Some(eid) = &a.test_evidence_id {
        if !identity(eid) {
            return Err(invalid());
        }
    }
    let mut test_digest: Option<String> = None;
    if let Some(eid) = &a.test_evidence_id {
        let ev = tx
            .query_opt(
                "SELECT work_id, contract_hash, digest FROM awr_team.evidence
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, eid],
            )
            .await?
            .ok_or(PgError::EvidenceInvalid)?;
        let w: String = ev.get(0);
        let c: String = ev.get(1);
        if w != command.work_id || c != contract_hash {
            return Err(PgError::EvidenceInvalid);
        }
        test_digest = Some(ev.get(2));
    }
    tx.execute(
        "UPDATE awr_team.pr_deliveries
         SET state='invalidated', invalidation_reason='superseded_by_new_registration'
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'",
        &[&tenant, &project, &command.work_id],
    )
    .await?;
    let id = crate::tx::new_id();
    let gh_submitted = a.gh_submitted.unwrap_or(true);
    let gh_approved = a.gh_approved.unwrap_or(false);
    let gh_merged = a.gh_merged.unwrap_or(false);
    tx.execute(
        "INSERT INTO awr_team.pr_deliveries(
            tenant_id, project_id, id, work_id, contract_hash, repository, pr_number, pr_url,
            head_sha, merge_sha, test_evidence_id, test_evidence_digest,
            gh_submitted, gh_approved, gh_merged, fact_source, observed_at,
            registered_by_actor_id, author_actor_id, owner_person_id, executor_actor_id, state)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,'active')",
        &[
            &tenant,
            &project,
            &id,
            &command.work_id,
            &contract_hash,
            &a.repository,
            &a.pr_number,
            &a.pr_url,
            &a.head_sha,
            &a.merge_sha,
            &a.test_evidence_id,
            &test_digest,
            &gh_submitted,
            &gh_approved,
            &gh_merged,
            &a.fact_source,
            &a.observed_at,
            &auth.actor_id,
            &a.author_actor_id,
            &a.owner_person_id,
            &a.executor_actor_id,
        ],
    )
    .await?;
    Ok(Applied {
        data: json!({
            "delivery_id": id,
            "work_id": command.work_id,
            "repository": a.repository,
            "pr_number": a.pr_number,
            "pr_url": a.pr_url,
            "head_sha": a.head_sha,
            "merge_sha": a.merge_sha,
            "gh_submitted": gh_submitted,
            "gh_approved": gh_approved,
            "gh_merged": gh_merged,
            "fact_source": a.fact_source,
            "observed_at": a.observed_at,
            "state": "active",
            "awr_acceptance_complete": false,
            "webhook_auto_sync": false,
            "task_complete": false,
        }),
        preceding_events: vec![(
            "delivery.pr_registered",
            json!({
                "delivery_id": id,
                "head_sha": a.head_sha,
                "fact_source": a.fact_source,
                "webhook_auto_sync": false,
            }),
        )],
    })
}

async fn observe_pr(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    _auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    a: ObservePr,
) -> PgResult<Applied> {
    if !identity(&a.delivery_id)
        || a.expected_head_sha.len() != 40
        || !a
            .expected_head_sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || !matches!(
            a.fact_source.as_str(),
            "authorized_human_github_verification" | "operator_recorded_observation"
        )
        || a.observed_at.trim().is_empty()
    {
        return Err(invalid());
    }
    let row = tx
        .query_opt(
            "SELECT work_id, contract_hash, head_sha, gh_approved, gh_merged, merge_sha, state
             FROM awr_team.pr_deliveries
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
            &[&tenant, &project, &a.delivery_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("pr delivery not found".into()))?;
    let work_id: String = row.get(0);
    let contract_hash: String = row.get(1);
    let head_sha: String = row.get(2);
    let mut gh_approved: bool = row.get(3);
    let mut gh_merged: bool = row.get(4);
    let mut merge_sha: Option<String> = row.get(5);
    let state: String = row.get(6);
    if work_id != command.work_id {
        return Err(PgError::Forbidden);
    }
    if state != "active" {
        return Err(PgError::PreconditionsChanged);
    }
    if head_sha != a.expected_head_sha {
        // Commit invalidation (do not roll back on mismatch): head change must
        // durably clear mismatched approvals even when the observe is refused.
        tx.execute(
            "UPDATE awr_team.pr_deliveries
             SET state='invalidated', invalidation_reason='head_sha_mismatch'
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &a.delivery_id],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.review_rounds SET state='invalidated'
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
               AND state IN ('open','approved') AND contract_hash=$4",
            &[&tenant, &project, &work_id, &contract_hash],
        )
        .await?;
        return Ok(Applied {
            data: json!({
                "delivery_id": a.delivery_id,
                "work_id": work_id,
                "state": "invalidated",
                "invalidation_reason": "head_sha_mismatch",
                "expected_head_sha": a.expected_head_sha,
                "recorded_head_sha": head_sha,
                "awr_acceptance_complete": false,
                "webhook_auto_sync": false,
                "task_complete": false,
            }),
            preceding_events: vec![(
                "delivery.pr_invalidated",
                json!({
                    "delivery_id": a.delivery_id,
                    "reason": "head_sha_mismatch",
                }),
            )],
        });
    }
    if let Some(v) = a.gh_approved {
        gh_approved = v;
    }
    if let Some(v) = a.gh_merged {
        gh_merged = v;
    }
    if let Some(m) = &a.merge_sha {
        if m.len() != 40
            || !m
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(invalid());
        }
        merge_sha = Some(m.clone());
    }
    tx.execute(
        "UPDATE awr_team.pr_deliveries
         SET gh_approved=$4, gh_merged=$5, merge_sha=$6, fact_source=$7, observed_at=$8
         WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[
            &tenant,
            &project,
            &a.delivery_id,
            &gh_approved,
            &gh_merged,
            &merge_sha,
            &a.fact_source,
            &a.observed_at,
        ],
    )
    .await?;
    Ok(Applied {
        data: json!({
            "delivery_id": a.delivery_id,
            "work_id": work_id,
            "head_sha": head_sha,
            "gh_approved": gh_approved,
            "gh_merged": gh_merged,
            "merge_sha": merge_sha,
            "fact_source": a.fact_source,
            "observed_at": a.observed_at,
            "awr_acceptance_complete": false,
            "webhook_auto_sync": false,
            "task_complete": false,
        }),
        preceding_events: vec![(
            "delivery.pr_observed",
            json!({
                "delivery_id": a.delivery_id,
                "gh_approved": gh_approved,
                "gh_merged": gh_merged,
                "awr_acceptance_complete": false,
            }),
        )],
    })
}

pub(crate) async fn inspect_delivery(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<Value> {
    let pr = tx
        .query_opt(
            "SELECT id, repository, pr_number, pr_url, head_sha, merge_sha,
                    gh_submitted, gh_approved, gh_merged, fact_source, observed_at,
                    contract_hash, state, test_evidence_id
             FROM awr_team.pr_deliveries
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'
             ORDER BY created_at DESC LIMIT 1",
            &[&tenant, &project, &work_id],
        )
        .await?;
    let runtime = tx
        .query_opt(
            "SELECT state, selected_completion_id FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &work_id],
        )
        .await?;
    let awr_complete = runtime.as_ref().is_some_and(|r| {
        r.get::<_, String>(0) == "completed" && r.get::<_, Option<String>>(1).is_some()
    });
    let pr = pr.as_ref().map(legacy_pr_snapshot);
    Ok(json!({
        "delivery": {
            "work_id": work_id,
            "pr": pr,
            "neutral_observation": pr.as_ref().map(|r| r.incomplete_observation()),
            "github": {
                "submitted": pr.as_ref().is_some_and(|r| r.submitted),
                "approved": pr.as_ref().is_some_and(|r| r.approved),
                "merged": pr.as_ref().is_some_and(|r| r.merged),
            },
            "awr_acceptance": {
                "complete": awr_complete,
                "runtime_state": runtime.as_ref().map(|r| r.get::<_,String>(0)),
                "selected_completion_id": runtime.as_ref().and_then(|r| r.get::<_,Option<String>>(1)),
            },
            "cannot_skip_acceptance_via": ["pr_url_alone","green_ci","admin_role","already_merged"],
            "webhook_auto_sync": false,
        }
    }))
}

pub(crate) async fn inspect_evidence(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    evidence_id: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT id, work_id, contract_hash, digest, trust_basis, execution_id,
                    output_digest, execution_result_digest, created_by, artifact_id
             FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &evidence_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("evidence not found".into()))?;
    Ok(json!({"evidence":{
        "evidence_id": row.get::<_,String>(0),
        "work_id": row.get::<_,String>(1),
        "contract_hash": row.get::<_,String>(2),
        "digest": row.get::<_,String>(3),
        "trust_basis": row.get::<_,String>(4),
        "execution_id": row.get::<_,Option<String>>(5),
        "artifact_digest": row.get::<_,Option<String>>(6),
        "execution_result_digest": row.get::<_,Option<String>>(7),
        "created_by": row.get::<_,String>(8),
        "artifact_id": row.get::<_,Option<String>>(9),
    }}))
}

pub(crate) async fn inspect_review(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    round_id: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT id, work_id, round_index, bundle_hash, contract_hash, state,
                    author_actor_id, author_person_id, evidence_id, execution_id,
                    artifact_digest, execution_result_digest, author_client_id
             FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &round_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("review round not found".into()))?;
    let decisions = tx
        .query(
            "SELECT decision, reviewer_actor_id, reviewer_person_id, independence_kind, reason,
                    approval_basis, reviewer_client_id
             FROM awr_team.review_decisions
             WHERE tenant_id=$1 AND project_id=$2 AND review_round_id=$3
             ORDER BY created_at ASC",
            &[&tenant, &project, &round_id],
        )
        .await?;
    Ok(json!({"review":{
        "round_id": row.get::<_,String>(0),
        "work_id": row.get::<_,String>(1),
        "round_index": row.get::<_,i32>(2),
        "bundle_hash": row.get::<_,String>(3),
        "contract_hash": row.get::<_,String>(4),
        "state": row.get::<_,String>(5),
        "author_actor_id": row.get::<_,String>(6),
        "author_person_id": row.get::<_,Option<String>>(7),
        "evidence_id": row.get::<_,Option<String>>(8),
        "execution_id": row.get::<_,Option<String>>(9),
        "artifact_digest": row.get::<_,Option<String>>(10),
        "execution_result_digest": row.get::<_,Option<String>>(11),
        "author_client_id": row.get::<_,Option<String>>(12),
        "decisions": decisions.iter().map(|d| json!({
            "decision": d.get::<_,String>(0),
            "reviewer_actor_id": d.get::<_,String>(1),
            "reviewer_person_id": d.get::<_,Option<String>>(2),
            "independence_kind": d.get::<_,String>(3),
            "reason": d.get::<_,String>(4),
            "approval_basis": d.get::<_,String>(5),
            "reviewer_client_id": d.get::<_,Option<String>>(6),
            "human_approval": matches!(d.get::<_,String>(5).as_str(), "human_independent_review" | "human_author_self_review") && d.get::<_,String>(0)=="approve",
            "team_independent_acceptance": d.get::<_,String>(3)=="team_independent" && d.get::<_,String>(0)=="approve",
        })).collect::<Vec<_>>(),
    }}))
}

pub(crate) async fn inspect_completion(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<Value> {
    let runtime = tx
        .query_opt(
            "SELECT state, selected_completion_id FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &work_id],
        )
        .await?;
    let Some(rt) = runtime else {
        return Ok(json!({"completion": Value::Null, "runtime_state": Value::Null}));
    };
    let state: String = rt.get(0);
    let selected: Option<String> = rt.get(1);
    let receipt = if let Some(id) = selected.as_deref() {
        tx.query_opt(
            "SELECT id, contract_hash, policy, independence_kind, evidence_id, execution_id,
                    approved_by_person_id, submitted_by_person_id, approved_by_json,
                    author_actor_id, executor_actor_id, final_submitter_actor_id
             FROM awr_team.completion_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &id],
        )
        .await?
    } else {
        None
    };
    Ok(json!({
        "runtime_state": state,
        "selected_completion_id": selected,
        "completion": receipt.map(|r| json!({
            "receipt_id": r.get::<_,String>(0),
            "contract_hash": r.get::<_,String>(1),
            "policy": r.get::<_,String>(2),
            "independence_kind": r.get::<_,Option<String>>(3),
            "evidence_id": r.get::<_,Option<String>>(4),
            "execution_id": r.get::<_,Option<String>>(5),
            "approved_by_person_id": r.get::<_,Option<String>>(6),
            "submitted_by_person_id": r.get::<_,Option<String>>(7),
            "approved_by": r.get::<_,Value>(8),
            "author_actor_id": r.get::<_,Option<String>>(9),
            "executor_actor_id": r.get::<_,Option<String>>(10),
            "final_submitter_actor_id": r.get::<_,Option<String>>(11),
            "provider_private_session": Value::Null,
        })),
    }))
}
