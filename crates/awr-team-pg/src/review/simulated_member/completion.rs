//! Reuse original review provenance for acceptance and neutral integration.
//! Live bindings may grant current authority, but cannot reconstruct old authors.
use super::*;
use std::collections::BTreeMap;

pub(crate) struct CompletionBind<'a> {
    pub work: &'a str,
    pub contract_hash: &'a str,
    pub evidence: &'a str,
    pub round: &'a str,
    pub decision: &'a str,
    pub snapshot: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewAuthority {
    delegation_id: String,
    membership_version: String,
    workstream_grant_versions: BTreeMap<String, i64>,
    snapshot_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewBasis {
    codec: String,
    policy: String,
    origins: RoundOrigins,
    reviewer: Origin,
    authority: ReviewAuthority,
    approval_basis: String,
    human_approval: bool,
    team_independent_acceptance: bool,
}

fn validate_basis(basis: &ReviewBasis) -> PgResult<()> {
    if basis.codec != "awr-simulated-member-review-v1"
        || basis.policy != POLICY
        || basis.approval_basis != APPROVAL_BASIS
        || basis.human_approval
        || basis.team_independent_acceptance
        || basis.origins.codec != "awr-member-review-origins-v1"
        || basis.authority.snapshot_id.is_empty()
        || basis.authority.snapshot_id.len() > 128
        || basis.authority.snapshot_id.chars().any(char::is_control)
        || basis.authority.membership_version != basis.reviewer.actor_membership_version
        || basis.authority.delegation_id.is_empty()
        || basis.authority.delegation_id.len() > 128
        || basis.authority.delegation_id.chars().any(char::is_control)
        || basis.authority.workstream_grant_versions.is_empty()
        || basis
            .authority
            .workstream_grant_versions
            .values()
            .any(|v| *v <= 0)
    {
        return Err(PgError::ReviewRequired);
    }
    basis.origins.executor.require_simulated_agent()?;
    basis.origins.submitter.validate()?;
    basis.origins.opener.validate()?;
    basis.reviewer.require_simulated_agent()?;
    require_independent(&basis.reviewer, &basis.origins)
}

/// The caller already holds the project transaction lock and has validated
/// current finalizer authority, artifact bytes and execution settlement.
pub(crate) async fn verify(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    bind: CompletionBind<'_>,
) -> PgResult<Value> {
    let row = tx.query_opt(
        "SELECT x.executor_origin_json,e.member_origins_json,r.member_origins_json,d.member_review_basis_json,
            x.executor_actor_id,x.executor_client_id,e.created_by,r.author_actor_id,r.author_client_id,r.author_person_id,
            d.reviewer_actor_id,d.reviewer_client_id,d.reviewer_person_id,d.independence_kind,d.approval_basis
         FROM awr_team.evidence e JOIN awr_team.executions x
           ON x.tenant_id=e.tenant_id AND x.project_id=e.project_id AND x.id=e.execution_id
         JOIN awr_team.review_rounds r ON r.tenant_id=e.tenant_id AND r.project_id=e.project_id
           AND r.evidence_id=e.id AND r.work_id=e.work_id AND r.bundle_hash=e.digest
         JOIN awr_team.review_decisions d ON d.tenant_id=r.tenant_id AND d.project_id=r.project_id AND d.review_round_id=r.id
         WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.id=$3 AND r.id=$4 AND d.id=$5
           AND e.work_id=$6 AND x.work_id=e.work_id AND d.work_id=e.work_id
           AND e.contract_hash=$7 AND x.contract_hash=e.contract_hash AND r.contract_hash=e.contract_hash
           AND r.execution_id=x.id AND r.artifact_digest=e.output_digest
           AND r.execution_result_digest=e.execution_result_digest AND d.bundle_hash=e.digest
           AND x.state='succeeded' AND x.scope_id='main' AND r.state='approved' AND d.decision='approve'",
        &[&tenant,&project,&bind.evidence,&bind.round,&bind.decision,&bind.work,&bind.contract_hash],
    ).await?.ok_or(PgError::ReviewRequired)?;
    let required = |index| {
        row.get::<_, Option<Value>>(index)
            .ok_or(PgError::EvidenceInvalid)
    };
    let executor = decode_origin(required(0)?)?;
    let evidence: EvidenceOrigins =
        serde_json::from_value(required(1)?).map_err(|_| PgError::EvidenceInvalid)?;
    let origins: RoundOrigins =
        serde_json::from_value(required(2)?).map_err(|_| PgError::EvidenceInvalid)?;
    let value = required(3)?;
    let basis: ReviewBasis =
        serde_json::from_value(value.clone()).map_err(|_| PgError::ReviewRequired)?;
    validate_basis(&basis)?;
    // Keep the review's original source authority. An unrelated publication may
    // advance the project snapshot without changing this task's reviewed contract.
    // Both archived and current definitions must still prove the exact same hash.
    let contracts = tx
        .query_opt(
            "SELECT original.contract_json,current.contract_json
         FROM awr_team.work_contracts original JOIN awr_team.work_contracts current
           ON current.tenant_id=original.tenant_id AND current.project_id=original.project_id
           AND current.scope_id=original.scope_id AND current.work_id=original.work_id
           AND current.contract_hash=original.contract_hash
         WHERE original.tenant_id=$1 AND original.project_id=$2 AND original.scope_id='main'
           AND original.work_id=$3 AND original.snapshot_id=$4 AND current.snapshot_id=$5
           AND original.contract_hash=$6",
            &[
                &tenant,
                &project,
                &bind.work,
                &basis.authority.snapshot_id,
                &bind.snapshot,
                &bind.contract_hash,
            ],
        )
        .await?
        .ok_or(PgError::ReviewRequired)?;
    for index in [0, 1] {
        let contract: awr_team::WorkContract =
            serde_json::from_value(contracts.get(index)).map_err(|_| PgError::ReviewRequired)?;
        if contract.work_id.as_str() != bind.work
            || contract.completion_policy != POLICY
            || contract.hash().map_err(|_| PgError::ReviewRequired)? != bind.contract_hash
        {
            return Err(PgError::ReviewRequired);
        }
    }
    if evidence.codec != "awr-member-evidence-origins-v1"
        || evidence.executor != executor
        || origins.executor != evidence.executor
        || origins.submitter != evidence.submitter
        || basis.origins != origins
        || executor.actor_id != row.get::<_, String>(4)
        || Some(executor.client_id.as_str()) != row.get::<_, Option<String>>(5).as_deref()
        || evidence.submitter.actor_id != row.get::<_, String>(6)
        || origins.opener.actor_id != row.get::<_, String>(7)
        || Some(origins.opener.client_id.as_str()) != row.get::<_, Option<String>>(8).as_deref()
        || Some(origins.opener.member_id.as_str()) != row.get::<_, Option<String>>(9).as_deref()
        || basis.reviewer.actor_id != row.get::<_, String>(10)
        || Some(basis.reviewer.client_id.as_str()) != row.get::<_, Option<String>>(11).as_deref()
        || Some(basis.reviewer.member_id.as_str()) != row.get::<_, Option<String>>(12).as_deref()
        || row.get::<_, String>(13) != INDEPENDENCE
        || row.get::<_, String>(14) != APPROVAL_BASIS
    {
        return Err(PgError::ReviewRequired);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basis() -> Value {
        let origin = |name: &str| {
            json!({
            "codec":"awr-member-origin-v1","actor_id":format!("agent-{name}"),
            "client_id":format!("client-{name}"),"actor_kind":"agent","actor_membership_version":"2",
            "member_id":format!("member-{name}"),"member_actor_kind":"agent","member_membership_version":"3",
            "binding_id":format!("binding-{name}"),
            "member_identity":{"kind":"simulated_member","controller_ref":"same-controller"}})
        };
        json!({"codec":"awr-simulated-member-review-v1","policy":POLICY,
            "origins":{"codec":"awr-member-review-origins-v1","executor":origin("author"),
                "submitter":origin("submitter"),"opener":origin("opener")},
            "reviewer":origin("reviewer"),
            "authority":{"delegation_id":"review-grant","membership_version":"2",
                "workstream_grant_versions":{"stream":3},"snapshot_id":"snapshot"},
            "approval_basis":APPROVAL_BASIS,"human_approval":false,"team_independent_acceptance":false})
    }

    #[test]
    fn completion_basis_requires_original_authority_and_preserves_simulation_flags() {
        let value = basis();
        let original: ReviewBasis = serde_json::from_value(value.clone()).unwrap();
        validate_basis(&original).unwrap();
        for snapshot in ["", "invalid\nsnapshot"] {
            let mut changed = value.clone();
            changed["authority"]["snapshot_id"] = json!(snapshot);
            let parsed: ReviewBasis = serde_json::from_value(changed).unwrap();
            assert!(validate_basis(&parsed).is_err());
        }
        for pointer in ["/human_approval", "/team_independent_acceptance"] {
            let mut changed = value.clone();
            *changed.pointer_mut(pointer).unwrap() = json!(true);
            let parsed: ReviewBasis = serde_json::from_value(changed).unwrap();
            assert!(validate_basis(&parsed).is_err());
        }
    }

    #[test]
    fn malformed_or_self_authored_completion_basis_never_becomes_approval() {
        let mut value = basis();
        let mut malformed = value.clone();
        malformed["authority"]["workstream_grant_versions"] = json!({"stream":"3"});
        assert!(serde_json::from_value::<ReviewBasis>(malformed).is_err());
        value["reviewer"]["client_id"] = value["origins"]["submitter"]["client_id"].clone();
        let parsed: ReviewBasis = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(
            validate_basis(&parsed),
            Err(PgError::AuthorCannotReview)
        ));
        value["caller_approved"] = json!(true);
        assert!(serde_json::from_value::<ReviewBasis>(value).is_err());
    }
}
