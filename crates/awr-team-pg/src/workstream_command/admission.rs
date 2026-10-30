//! Admission decisions of governed work, recorded under the verified identity of the decider. The
//! rules are the Core's; this command supplies the Team's facts and the attribution. A source,
//! an import or a restore never reaches this path.
use super::*;
use awr_core::{AdmissionDecision, AdmissionJudgment};
use awr_team::binding_fingerprint;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Decide {
    admission_decision: AdmissionDecision,
    /// The fingerprint of the scope the decider read (`work.admission`), checked against the scope now.
    binding_fingerprint: String,
    admission_judgment: Option<AdmissionJudgment>,
    admission_reason: Option<String>,
    /// The proposal an approval answers.
    admission_proposal_id: Option<String>,
}

pub(super) enum Action {
    Decide(Decide),
}

impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        if op != "admission.decide" {
            return Err(invalid());
        }
        let a: Decide = serde_json::from_value(args).map_err(|_| invalid())?;
        if !digest(&a.binding_fingerprint) {
            return Err(invalid());
        }
        if a.admission_proposal_id
            .as_deref()
            .is_some_and(|id| !identity(id))
        {
            return Err(invalid());
        }
        if let Some(judgment) = &a.admission_judgment {
            judgment.validate().map_err(|_| invalid())?;
        }
        match a.admission_decision {
            // A proposal is a request for a decision: it states its judgment and answers nothing.
            AdmissionDecision::Proposed => {
                if a.admission_judgment.is_none()
                    || a.admission_reason.is_some()
                    || a.admission_proposal_id.is_some()
                {
                    return Err(invalid());
                }
            }
            // An approval states its judgment inline or answers a proposal, never neither.
            AdmissionDecision::Approved => {
                if a.admission_reason.is_some()
                    || (a.admission_judgment.is_none() && a.admission_proposal_id.is_none())
                {
                    return Err(invalid());
                }
            }
            AdmissionDecision::Denied => {
                let reason = a.admission_reason.as_deref().ok_or_else(invalid)?;
                if reason.trim().is_empty()
                    || reason.len() > 2048
                    || reason.chars().any(char::is_control)
                    || a.admission_judgment.is_some()
                    || a.admission_proposal_id.is_some()
                {
                    return Err(invalid());
                }
            }
        }
        Ok(Self::Decide(a))
    }
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    a: Action,
) -> PgResult<Applied> {
    let Action::Decide(a) = a;
    let (status, current, governed) =
        crate::governance::status(tx, tenant, project, &auth.snapshot, &command.work_id)
            .await?
            .ok_or_else(|| PgError::Unsupported("the work is not governed".into()))?;
    if !status.governed {
        return Err(PgError::Unsupported("the work is not governed".into()));
    }
    // The decider read one scope; deciding on another is a stale review.
    if binding_fingerprint(&current).map_err(|_| PgError::SourceDivergence)?
        != a.binding_fingerprint
    {
        return Err(PgError::PreconditionsChanged);
    }
    // Attribution is the verified actor; a person is recorded when the actor is linked to one.
    let person = crate::review::resolve_person_id(tx, tenant, project, &auth.actor_id)
        .await
        .ok();
    if a.admission_decision == AdmissionDecision::Approved {
        let proposal = match &a.admission_proposal_id {
            Some(id) => {
                let row = tx
                    .query_opt(
                        "SELECT decision, binding_json, person_id FROM awr_team.work_admissions
                        WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND work_id=$4",
                        &[&tenant, &project, id, &command.work_id],
                    )
                    .await?
                    .ok_or_else(invalid)?;
                let decision: String = row.get(0);
                let bound: Value = row.get(1);
                // A proposal answers only for the scope it was made for.
                if decision != "proposed" || bound != json!(current) {
                    return Err(PgError::PreconditionsChanged);
                }
                Some(row.get::<_, Option<String>>(2))
            }
            None => None,
        };
        if status.independent_review_required {
            // Independence the program can show: another person than the proposer.
            let proposer = proposal.ok_or_else(invalid)?;
            match (&person, &proposer) {
                (Some(decider), Some(proposer)) if decider != proposer => {}
                _ => return Err(PgError::AuthorCannotReview),
            }
        }
    }
    let id = crate::tx::new_id();
    let binding = json!(current);
    let judgment = a.admission_judgment.as_ref().map(|j| json!(j));
    tx.execute(
        "INSERT INTO awr_team.work_admissions(tenant_id,project_id,id,scope_id,work_id,request_id,decision,
            contract_hash,binding_json,reason,judgment_json,proposal_id,actor_id,person_id,project_revision)
        VALUES($1,$2,$3,'main',$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        &[
            &tenant,
            &project,
            &id,
            &command.work_id,
            &command.request_id,
            &decision_name(a.admission_decision),
            &current.contract_fingerprint,
            &binding,
            &a.admission_reason,
            &judgment,
            &a.admission_proposal_id,
            &auth.actor_id,
            &person,
            &auth.revision,
        ],
    )
    .await?;
    let after = crate::governance::status(tx, tenant, project, &auth.snapshot, &command.work_id)
        .await?
        .ok_or(PgError::SourceDivergence)?
        .0;
    Ok(Applied {
        data: json!({
            "admission_id": id, "decision": decision_name(a.admission_decision), "state": after.state,
            "admitted": after.facts.admitted, "diagnostics": after.diagnostics,
            "binding_fingerprint": a.binding_fingerprint, "governance_hash": governed.governance_hash,
            "decider": {"actor_id": auth.actor_id, "person_id": person, "identity": "verified"},
            "completion_claimed": false
        }),
        preceding_events: vec![],
    })
}

fn decision_name(decision: AdmissionDecision) -> &'static str {
    match decision {
        AdmissionDecision::Proposed => "proposed",
        AdmissionDecision::Approved => "approved",
        AdmissionDecision::Denied => "denied",
    }
}
