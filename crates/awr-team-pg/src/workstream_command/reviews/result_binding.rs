//! The exact result binding that completion and repository integration need, checked when it is still cheap to repair.
//!
//! A work completed under an agent review policy is verified against the workspace-settled execution it names: the
//! evidence input must match the execution input; the output digest the evidence declares and the sha256 of the
//! artifact the evidence stored must both equal the output
//! the execution reported. Completion and integration refuse otherwise (`complete`, `settlement::verify_receipt`).
//! This module evaluates the same relations when evidence is submitted (reported, never refused: one execution can
//! have auxiliary evidence), when a review round is bound to the evidence, and when that round is approved, so a
//! doomed round never uses up a review and the refusal can name the repair. Only evidence submitted by the
//! execution's own executor is judged: notes that another actor attaches to the execution (a reviewer's feedback, say)
//! are auxiliary and keep their former behaviour.
use super::*;

pub(super) const DECLARES_NO_OUTPUT_DIGEST: &str = "evidence_declares_no_output_digest";
pub(super) const DECLARED_DIFFERS: &str = "declared_output_digest_differs_from_execution_result";
pub(super) const NO_STORED_ARTIFACT: &str = "evidence_has_no_stored_artifact";
pub(super) const ARTIFACT_DIFFERS: &str = "artifact_digest_differs_from_execution_result";

/// Completion policies whose completion verifies review-bound evidence against a settled execution's exact result.
pub(super) fn applies_to(policy: &str) -> bool {
    policy == crate::review::AGENT_REVIEW_POLICY || policy == simulated_member::POLICY
}

/// What the stored digests say about one piece of evidence and the result its execution reported.
pub(super) struct Evaluation {
    execution_result: String,
    declared: Option<String>,
    artifact: Option<String>,
    reasons: Vec<&'static str>,
}

impl Evaluation {
    /// The failing relations as closed labels, in the order completion checks them; empty when the evidence binds.
    pub(super) fn reasons(&self) -> &[&'static str] {
        &self.reasons
    }

    pub(super) fn binds(&self) -> bool {
        self.reasons.is_empty()
    }

    /// The `result_binding` fact of the submit receipt. Digests are shown as 12-character prefixes.
    pub(super) fn report(&self) -> Value {
        json!({
            "status": if self.binds() { "matches" } else { "differs" },
            "reasons": self.reasons(),
            "execution_result_digest": prefix(Some(&self.execution_result)),
            "declared_output_digest": prefix(self.declared.as_deref()),
            "artifact_digest": prefix(self.artifact.as_deref()),
            "completion_needs": "evidence that a review round is bound to must declare a payload.output_digest and store an artifact whose sha256 both equal execution_result_digest; auxiliary evidence is not affected",
        })
    }

    pub(super) fn refusal(&self) -> PgError {
        PgError::EvidenceResultBinding {
            reasons: self.reasons.clone(),
            execution_result: prefix(Some(&self.execution_result)),
            declared: prefix(self.declared.as_deref()),
            artifact: prefix(self.artifact.as_deref()),
        }
    }
}

/// First 12 characters of a lowercase 64-digit sha256, or `none`: only validated hex is ever echoed.
fn prefix(digest: Option<&str>) -> String {
    match digest {
        Some(d)
            if d.len() == 64
                && d.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            d[..12].to_owned()
        }
        _ => "none".to_owned(),
    }
}

/// The four relations completion checks, in its order. `recorded` is the settled execution's result digest.
pub(super) fn evaluate_digests(
    recorded: &str,
    declared: Option<&str>,
    artifact: Option<&str>,
) -> Evaluation {
    let mut reasons = Vec::new();
    match declared {
        None => reasons.push(DECLARES_NO_OUTPUT_DIGEST),
        Some(d) if d != recorded => reasons.push(DECLARED_DIFFERS),
        Some(_) => {}
    }
    match artifact {
        None => reasons.push(NO_STORED_ARTIFACT),
        Some(a) if a != recorded => reasons.push(ARTIFACT_DIFFERS),
        Some(_) => {}
    }
    Evaluation {
        execution_result: recorded.to_owned(),
        declared: declared.map(str::to_owned),
        artifact: artifact.map(str::to_owned),
        reasons,
    }
}

/// The evidence whose digests are judged, as the three callers know it.
pub(super) struct Subject<'a> {
    /// Completion policy of the work contract.
    pub(super) policy: &'a str,
    pub(super) execution_id: Option<&'a str>,
    /// The actor that submitted the evidence.
    pub(super) submitter: &'a str,
    /// The top-level evidence input binding, separate from its arbitrary payload.
    pub(super) input: Option<&'a str>,
    /// `payload.output_digest` the evidence declares.
    pub(super) declared: Option<&'a str>,
    /// sha256 of the artifact the evidence stored.
    pub(super) artifact: Option<&'a str>,
}

/// Evaluate evidence that `submitter` attaches to an execution. `None` when the policy does not verify an exact
/// result, the evidence names no execution, the submitter is not the execution's executor, or the execution is not a
/// succeeded workspace-settled one (its result digest is recorded only then; the other completion paths keep their
/// own checks).
pub(super) async fn evaluate(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    subject: &Subject<'_>,
) -> PgResult<Option<Evaluation>> {
    let recorded = recorded_execution(tx, tenant, project, subject).await?;
    Ok(recorded.and_then(|row| {
        row.get::<_, Option<String>>(0)
            .map(|recorded| evaluate_digests(&recorded, subject.declared, subject.artifact))
    }))
}

async fn recorded_execution(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    subject: &Subject<'_>,
) -> PgResult<Option<tokio_postgres::Row>> {
    let Some(execution_id) = subject.execution_id.filter(|_| applies_to(subject.policy)) else {
        return Ok(None);
    };
    Ok(tx
        .query_opt(
            "SELECT result_digest,input_digest FROM awr_team.executions
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND executor_actor_id=$4
               AND state='succeeded' AND workspace_effects_settled",
            &[&tenant, &project, &execution_id, &subject.submitter],
        )
        .await?)
}

/// Refuse to bind or approve evidence that completion would refuse for its digests.
pub(super) async fn require(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    subject: &Subject<'_>,
) -> PgResult<()> {
    let Some(recorded) = recorded_execution(tx, tenant, project, subject).await? else {
        return Ok(());
    };
    let input: Option<String> = recorded.get(1);
    let reason = match (subject.input, input.as_deref()) {
        (_, None) => Some("execution_input_digest_not_recorded"),
        (None, Some(_)) => Some("evidence_input_digest_not_declared"),
        (Some(a), Some(b)) if a != b => Some("execution_input_digest_mismatch"),
        _ => None,
    };
    if let Some(reason) = reason {
        return Err(PgError::evidence_input_binding(reason));
    }
    if let Some(result) = recorded.get::<_, Option<String>>(0) {
        let evaluation = evaluate_digests(&result, subject.declared, subject.artifact);
        if !evaluation.binds() {
            return Err(evaluation.refusal());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESULT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const OTHER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn evidence_that_declares_and_stores_the_reported_result_binds() {
        assert!(evaluate_digests(RESULT, Some(RESULT), Some(RESULT)).binds());
    }

    #[test]
    fn every_failing_relation_is_named_in_completion_order() {
        assert_eq!(
            evaluate_digests(RESULT, None, Some(OTHER)).reasons(),
            [DECLARES_NO_OUTPUT_DIGEST, ARTIFACT_DIFFERS]
        );
        assert_eq!(
            evaluate_digests(RESULT, Some(OTHER), Some(RESULT)).reasons(),
            [DECLARED_DIFFERS]
        );
        assert_eq!(
            evaluate_digests(RESULT, Some(RESULT), None).reasons(),
            [NO_STORED_ARTIFACT]
        );
        assert_eq!(
            evaluate_digests(RESULT, Some(OTHER), None).reasons(),
            [DECLARED_DIFFERS, NO_STORED_ARTIFACT]
        );
    }

    #[test]
    fn the_receipt_fact_and_the_refusal_show_only_validated_prefixes() {
        let evaluation = evaluate_digests(RESULT, Some("not a digest"), Some(OTHER));
        let fact = evaluation.report();
        assert_eq!(fact["status"], "differs");
        assert_eq!(fact["execution_result_digest"], "bbbbbbbbbbbb");
        assert_eq!(fact["declared_output_digest"], "none");
        assert_eq!(fact["artifact_digest"], "aaaaaaaaaaaa");
        let PgError::EvidenceResultBinding { declared, .. } = evaluation.refusal() else {
            panic!("a failing evaluation refuses with EvidenceResultBinding");
        };
        assert_eq!(declared, "none");
    }
}
