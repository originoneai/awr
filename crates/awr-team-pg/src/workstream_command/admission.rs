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
    /// How many decisions the decider read (`work.admission`): one recorded since is one this
    /// decider has not seen.
    expected_admission_version: String,
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
        version(&a.expected_admission_version)?;
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
    // Two deciders who read the same history get one decision; the other reads again.
    if crate::governance::admission_version(tx, tenant, project, &command.work_id).await?
        != version(&a.expected_admission_version)?
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

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn judgment() -> Value {
        json!({"contribution":"c","necessity":"n","alternatives":"a","deliverable":"d","exit":"e","responsibility":"r"})
    }

    fn parse(args: Value) -> PgResult<Action> {
        Action::parse("admission.decide", args)
    }

    #[test]
    fn each_decision_has_its_own_shape() {
        let base = |decision: &str| {
            json!({"admission_decision": decision, "binding_fingerprint": FINGERPRINT,
                   "expected_admission_version": "0"})
        };
        let with = |mut value: Value, key: &str, field: Value| {
            value[key] = field;
            value
        };
        // A proposal states a judgment and answers nothing.
        assert!(parse(with(base("proposed"), "admission_judgment", judgment())).is_ok());
        assert!(parse(base("proposed")).is_err());
        assert!(
            parse(with(
                with(base("proposed"), "admission_judgment", judgment()),
                "admission_reason",
                json!("no")
            ))
            .is_err()
        );
        assert!(
            parse(with(
                with(base("proposed"), "admission_judgment", judgment()),
                "admission_proposal_id",
                json!("p-1")
            ))
            .is_err()
        );
        // An approval states its judgment inline or answers a proposal, never neither.
        assert!(parse(with(base("approved"), "admission_judgment", judgment())).is_ok());
        assert!(
            parse(with(
                base("approved"),
                "admission_proposal_id",
                json!("p-1")
            ))
            .is_ok()
        );
        assert!(parse(base("approved")).is_err());
        assert!(
            parse(with(
                with(base("approved"), "admission_judgment", judgment()),
                "admission_reason",
                json!("why")
            ))
            .is_err()
        );
        // A denial needs a reason and nothing else.
        assert!(
            parse(with(
                base("denied"),
                "admission_reason",
                json!("The examples restate the acceptance")
            ))
            .is_ok()
        );
        assert!(parse(base("denied")).is_err());
        assert!(parse(with(base("denied"), "admission_reason", json!("  "))).is_err());
        assert!(
            parse(with(
                with(base("denied"), "admission_reason", json!("no")),
                "admission_judgment",
                judgment()
            ))
            .is_err()
        );
    }

    #[test]
    fn the_decider_is_never_named_by_the_request() {
        let mut args = json!({"admission_decision":"approved","binding_fingerprint":FINGERPRINT,
            "expected_admission_version":"0","admission_judgment":judgment()});
        assert!(parse(args.clone()).is_ok());
        // A label in the body is not an identity: unknown fields, whoever they claim to be, are refused.
        for field in ["agent", "actor_id", "decider", "person_id", "session"] {
            args[field] = json!("someone-else");
            assert!(parse(args.clone()).is_err(), "{field}");
            args.as_object_mut().unwrap().remove(field);
        }
    }

    #[test]
    fn a_denial_reason_and_a_proposal_id_are_bounded_text_without_control_characters() {
        let denial = |reason: &str| {
            json!({"admission_decision":"denied","binding_fingerprint":FINGERPRINT,
                   "expected_admission_version":"0","admission_reason":reason})
        };
        assert!(parse(denial(&"r".repeat(2048))).is_ok());
        assert!(parse(denial(&"r".repeat(2049))).is_err());
        assert!(parse(denial("two\nlines")).is_err());
        assert!(parse(denial("bell\u{7}")).is_err());
        let approval = |id: &str| {
            json!({"admission_decision":"approved","binding_fingerprint":FINGERPRINT,
                   "expected_admission_version":"0","admission_proposal_id":id})
        };
        assert!(parse(approval("proposal-1")).is_ok());
        assert!(parse(approval(&"p".repeat(128))).is_ok());
        for bad in ["", "bell\u{7}", &"p".repeat(129)] {
            assert!(parse(approval(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_history_read_is_a_canonical_version_and_is_never_left_out() {
        let good = json!({"admission_decision":"approved","binding_fingerprint":FINGERPRINT,
            "expected_admission_version":"3","admission_judgment":judgment()});
        assert!(parse(good.clone()).is_ok());
        for bad in ["", "03", "-1", "x", "1.5", "99999999999999999999"] {
            let mut args = good.clone();
            args["expected_admission_version"] = json!(bad);
            assert!(parse(args).is_err(), "{bad}");
        }
        // A decision that does not say what it read cannot be checked, so it is not accepted.
        let mut args = good;
        args.as_object_mut()
            .unwrap()
            .remove("expected_admission_version");
        assert!(parse(args).is_err());
    }

    #[test]
    fn the_scope_read_is_a_digest_and_the_judgment_is_complete() {
        let good = json!({"admission_decision":"approved","binding_fingerprint":FINGERPRINT,
            "expected_admission_version":"0","admission_judgment":judgment()});
        assert!(parse(good.clone()).is_ok());
        for bad in [
            "",
            "sha256:abc",
            &FINGERPRINT.to_uppercase(),
            &FINGERPRINT[1..],
        ] {
            let mut args = good.clone();
            args["binding_fingerprint"] = json!(bad);
            assert!(parse(args).is_err(), "{bad}");
        }
        let mut args = good.clone();
        args["admission_judgment"]["exit"] = json!("");
        assert!(parse(args).is_err());
        let mut args = good;
        args["admission_judgment"]["extra"] = json!("x");
        assert!(parse(args).is_err());
        assert!(Action::parse("admission.propose", json!({})).is_err());
    }
}
