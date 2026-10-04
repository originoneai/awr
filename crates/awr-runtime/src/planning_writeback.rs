//! Planning writeback activation gate and recovery phases (AWR-TMCP-022).
//!
//! Reuses WS-022 precise-patch recovery journals, WS-023 barrier/lock order
//! documentation, and WS-032 selective replan rules. Does not invent a second
//! dependency graph. Project-wide "any active claim blocks activation" is not
//! deleted: callers must emit an affected set, recheck read-sets, allow proven
//! unrelated work to continue, and require explicit stop/reconcile/replan for
//! affected work. Cancel / lease expiry / end-session do not prove an external
//! process has stopped.
use crate::selective_invalidation::{
    AdoptedConsumerEdge, CancelSplitRelation, DiscoverDependencyRequest, PlanningChangeApplication,
    PlanningChangeStatus, ProviderChangeKind, SelectiveInvalidationPlan,
    record_discovered_dependency_change, select_downstream_reevaluation,
};
use crate::source_concurrency::{SourceConcurrencyReport, activate_precise_patch};
use awr_core::{MutationProposal, Result, Revision};
use awr_source::{
    LedgerWritebackPatch, apply_planning_changes_to_ledger, fingerprint, refuse_external_overwrite,
};
use awr_store::Store;
use awr_team::{DraftChange, DraftOpKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Live runtime facts observed for one work item when gating activation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WorkRuntimeObservation {
    pub work_id: String,
    pub external_key: String,
    pub has_active_claim: bool,
    pub claim_cancelled: bool,
    pub claim_expired: bool,
    pub session_ended: bool,
    /// True only after an explicit stop/reconcile of the external process.
    pub external_process_stopped: bool,
    pub has_nonterminal_execution: bool,
    pub unknown_effect: bool,
    /// Operation read-set work ids this work observed (WS-020). Empty means
    /// impact via read-set cannot be proven from this observation alone.
    pub read_set_work_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationDisposition {
    ContinueUnrelated,
    RequireStopReconcileReplan,
    ConservativeRefuse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffectedWorkDecision {
    pub work_id: String,
    pub disposition: ActivationDisposition,
    pub reasons: Vec<String>,
    pub recovery_actions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationImpactReport {
    pub affected_work_ids: Vec<String>,
    pub unrelated_work_ids: Vec<String>,
    pub unknown_impact_work_ids: Vec<String>,
    pub decisions: Vec<AffectedWorkDecision>,
    pub allow_activation: bool,
    pub refuse_reason: Option<String>,
    pub recovery_actions: Vec<String>,
    /// Project-wide active-claim barrier is retained as the default when impact
    /// cannot be proven; selective continue is opt-in only after proof.
    pub retained_project_claim_barrier: bool,
}

/// Analyze which works a planning change affects and whether activation may
/// proceed. Graph changes reuse WS-032 selective replan — callers pass the
/// same affected set they would record via `record_discovered_dependency_change`.
pub fn analyze_activation_impact(
    changed_work_ids: &[String],
    all_project_work_ids: &[String],
    observations: &[WorkRuntimeObservation],
    impact_proven: bool,
) -> ActivationImpactReport {
    let changed: BTreeSet<_> = changed_work_ids.iter().cloned().collect();
    let all: BTreeSet<_> = all_project_work_ids.iter().cloned().collect();
    let obs_by_id: std::collections::BTreeMap<_, _> = observations
        .iter()
        .map(|o| (o.work_id.clone(), o))
        .collect();

    // Expand impact via observed read-sets: works whose read-set intersects the
    // changed set are also affected (WS-020 recheck).
    let mut affected = changed.clone();
    for o in observations {
        let rs: BTreeSet<_> = o.read_set_work_ids.iter().cloned().collect();
        if !rs.is_disjoint(&changed) {
            affected.insert(o.work_id.clone());
        }
    }

    let unrelated: Vec<String> = all
        .difference(&affected)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut unknown = Vec::new();
    let mut decisions = Vec::new();
    let mut recovery = Vec::new();
    let mut allow = true;
    let mut refuse_reason = None;
    let retained_barrier = !impact_proven;

    if !impact_proven {
        refuse_reason = Some(
            "activation impact cannot be proven; retaining project-wide active-claim barrier"
                .into(),
        );
        recovery.extend([
            "recompute affected set from planning candidate diffs".into(),
            "recheck operation read-sets for live claims and nonterminal executions".into(),
            "explicitly stop/reconcile/replan affected works before retry".into(),
        ]);
        for work_id in &affected {
            decisions.push(AffectedWorkDecision {
                work_id: work_id.clone(),
                disposition: ActivationDisposition::ConservativeRefuse,
                reasons: vec!["impact_unproven".into()],
                recovery_actions: recovery.clone(),
            });
        }
        return ActivationImpactReport {
            affected_work_ids: affected.into_iter().collect(),
            unrelated_work_ids: unrelated,
            unknown_impact_work_ids: all
                .into_iter()
                .filter(|w| !changed_work_ids.contains(w))
                .collect(),
            decisions,
            allow_activation: false,
            refuse_reason,
            recovery_actions: recovery,
            retained_project_claim_barrier: true,
        };
    }

    for work_id in &affected {
        let Some(obs) = obs_by_id.get(work_id) else {
            unknown.push(work_id.clone());
            allow = false;
            refuse_reason = Some(format!(
                "missing runtime observation for affected work {work_id}"
            ));
            let actions = vec![
                "inspect claim/session/execution for the affected work".into(),
                "explicit stop/reconcile if an external process may still be running".into(),
                "replan the affected work against the new source version".into(),
            ];
            recovery.extend(actions.clone());
            decisions.push(AffectedWorkDecision {
                work_id: work_id.clone(),
                disposition: ActivationDisposition::ConservativeRefuse,
                reasons: vec!["observation_missing".into()],
                recovery_actions: actions,
            });
            continue;
        };

        let mut reasons = Vec::new();
        // Cancel / expiry / end-session never prove the external process stopped.
        if obs.claim_cancelled {
            reasons.push("claim_cancelled_does_not_prove_process_stopped".into());
        }
        if obs.claim_expired {
            reasons.push("claim_expired_does_not_prove_process_stopped".into());
        }
        if obs.session_ended {
            reasons.push("session_ended_does_not_prove_process_stopped".into());
        }

        // Cancel / expiry / end-session never satisfy external_process_stopped.
        let pretended_stopped = obs.claim_cancelled || obs.claim_expired || obs.session_ended;
        let live = obs.has_active_claim
            || obs.has_nonterminal_execution
            || obs.unknown_effect
            || pretended_stopped;
        if live && !obs.external_process_stopped {
            allow = false;
            reasons.push("affected_work_requires_explicit_stop_reconcile_replan".into());
            let actions = vec![
                "explicitly stop the external process for this work".into(),
                "reconcile unknown effects via authorized execution.reconcile".into(),
                "replan the work against the new activated source".into(),
            ];
            recovery.extend(actions.clone());
            decisions.push(AffectedWorkDecision {
                work_id: work_id.clone(),
                disposition: ActivationDisposition::RequireStopReconcileReplan,
                reasons,
                recovery_actions: actions,
            });
            refuse_reason =
                Some("affected live work blocks activation until stop/reconcile/replan".into());
        } else {
            decisions.push(AffectedWorkDecision {
                work_id: work_id.clone(),
                disposition: ActivationDisposition::ContinueUnrelated,
                reasons: if reasons.is_empty() {
                    vec!["affected_but_quiescent".into()]
                } else {
                    reasons
                },
                recovery_actions: vec![],
            });
        }
    }

    for work_id in &unrelated {
        decisions.push(AffectedWorkDecision {
            work_id: work_id.clone(),
            disposition: ActivationDisposition::ContinueUnrelated,
            reasons: vec!["proven_unrelated_may_continue".into()],
            recovery_actions: vec![],
        });
    }

    ActivationImpactReport {
        affected_work_ids: affected.into_iter().collect(),
        unrelated_work_ids: unrelated,
        unknown_impact_work_ids: unknown,
        decisions,
        allow_activation: allow,
        refuse_reason,
        recovery_actions: {
            let u: BTreeSet<_> = recovery.into_iter().collect();
            u.into_iter().collect()
        },
        retained_project_claim_barrier: retained_barrier,
    }
}

/// Map draft changes onto WS-032 selective replan input. Does not create a
/// second graph system — only records the scoped planning change shape.
pub fn planning_changes_as_selective_replan(
    change_id: &str,
    project_id: &str,
    discovered_by: &str,
    old_graph_version: &str,
    new_graph_version: &str,
    old_acceptance: &str,
    new_acceptance: &str,
    changes: &[DraftChange],
    all_project_work_ids: &[String],
) -> std::result::Result<PlanningChangeApplication, String> {
    let mut affected = BTreeSet::new();
    let mut cancel_split = Vec::new();
    for change in changes {
        affected.insert(change.after.work_id.clone());
        if let Some(before) = &change.before {
            affected.insert(before.work_id.clone());
        }
        match change.op {
            DraftOpKind::Cancel => cancel_split.push(CancelSplitRelation {
                kind: "cancel".into(),
                from_work_id: change
                    .before
                    .as_ref()
                    .map(|b| b.work_id.clone())
                    .unwrap_or_else(|| change.after.work_id.clone()),
                to_work_id: change.after.work_id.clone(),
            }),
            DraftOpKind::Split => {
                for child in &change.after.split_children {
                    affected.insert(child.clone());
                    cancel_split.push(CancelSplitRelation {
                        kind: "split".into(),
                        from_work_id: change.after.work_id.clone(),
                        to_work_id: child.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    record_discovered_dependency_change(DiscoverDependencyRequest {
        change_id: change_id.into(),
        project_id: project_id.into(),
        discovered_by: discovered_by.into(),
        old_graph_version: old_graph_version.into(),
        new_graph_version: new_graph_version.into(),
        old_acceptance_contract: old_acceptance.into(),
        new_acceptance_contract: new_acceptance.into(),
        affected_work_ids: affected.into_iter().collect(),
        cancel_split_relations: cancel_split,
        continue_conditions: vec!["unrelated_read_sets_disjoint".into()],
        all_project_work_ids: all_project_work_ids.to_vec(),
    })
}

/// Recovery-journal phases for cross-system (source file + PG) activation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WritebackPhase {
    Planned,
    SourceWritten,
    PgActivating,
    Completed,
    Refused,
    RolledBack,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WritebackJournal {
    pub version: u32,
    pub request_id: String,
    pub phase: WritebackPhase,
    pub candidate_id: String,
    pub candidate_digest: String,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub source_version: Option<String>,
    pub activated_snapshot_id: Option<String>,
    pub authority_epoch: Option<String>,
    pub approver_actor_id: Option<String>,
    pub affected_work_ids: Vec<String>,
    pub unrelated_work_ids: Vec<String>,
    pub audit_receipt_id: Option<String>,
}

impl WritebackJournal {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

/// Build a precise ledger patch under fingerprint protection. Does not touch PG.
pub fn plan_ledger_writeback(
    ledger_bytes: &[u8],
    changes: &[DraftChange],
    observed_fingerprint: &str,
) -> Result<LedgerWritebackPatch> {
    let patch = apply_planning_changes_to_ledger(ledger_bytes, changes)?;
    refuse_external_overwrite(&patch.before_fingerprint, observed_fingerprint)?;
    Ok(patch)
}

/// Apply a prepared precise patch through the WS-022 recovery-log activator.
pub fn activate_ledger_writeback_precise(
    store: &mut Store,
    root: &Path,
    proposal: &MutationProposal,
    request_key: &str,
    expected_revision: Revision,
) -> Result<SourceConcurrencyReport> {
    activate_precise_patch(store, root, proposal, request_key, expected_revision)
}

/// Idempotent publish/activation identity: same request_id yields one effective
/// activation. Callers compare journals before starting a new attempt.
pub fn same_request_already_completed(
    existing: Option<&WritebackJournal>,
    request_id: &str,
) -> bool {
    existing
        .is_some_and(|j| j.request_id == request_id && matches!(j.phase, WritebackPhase::Completed))
}

/// Downstream consumers of graph edges still use WS-032 selection — exposed so
/// Team PG can recheck without forking the algorithm.
pub fn reevaluate_graph_consumers(
    provider_work_id: &str,
    change_kind: ProviderChangeKind,
    consumers: &[AdoptedConsumerEdge],
    all_project_work_ids: &[String],
) -> SelectiveInvalidationPlan {
    select_downstream_reevaluation(
        provider_work_id,
        change_kind,
        consumers,
        all_project_work_ids.iter().map(|s| s.as_str()),
    )
}

pub fn planning_change_blocks_until_confirmed(status: &PlanningChangeStatus) -> bool {
    matches!(status, PlanningChangeStatus::AffectedBlocked)
}

/// Helper for tests / callers assembling a journal path under `.awr/mutations`.
pub fn writeback_journal_path(root: &Path, request_id: &str) -> PathBuf {
    let digest = fingerprint(request_id.as_bytes());
    let short = digest.trim_start_matches("sha256:");
    root.join(".awr/mutations")
        .join(format!("tmcp022-{}", &short[..32.min(short.len())]))
        .join("writeback-journal.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_team::{DraftDefinitionState, TaskDraft};

    fn obs(id: &str) -> WorkRuntimeObservation {
        WorkRuntimeObservation {
            work_id: id.into(),
            external_key: id.into(),
            ..Default::default()
        }
    }

    #[test]
    fn unproven_impact_conservatively_refuses_and_keeps_barrier() {
        let report = analyze_activation_impact(
            &["a".into()],
            &["a".into(), "b".into()],
            &[obs("a"), obs("b")],
            false,
        );
        assert!(!report.allow_activation);
        assert!(report.retained_project_claim_barrier);
        assert!(!report.recovery_actions.is_empty());
        assert_eq!(report.unrelated_work_ids, vec!["b".to_string()]);
    }

    #[test]
    fn proven_unrelated_may_continue_while_affected_live_blocks() {
        let mut live = obs("a");
        live.has_active_claim = true;
        let quiet = obs("b");
        let report = analyze_activation_impact(
            &["a".into()],
            &["a".into(), "b".into()],
            &[live, quiet],
            true,
        );
        assert!(!report.allow_activation);
        assert!(!report.retained_project_claim_barrier);
        assert_eq!(report.unrelated_work_ids, vec!["b".to_string()]);
        let b = report.decisions.iter().find(|d| d.work_id == "b").unwrap();
        assert_eq!(b.disposition, ActivationDisposition::ContinueUnrelated);
        let a = report.decisions.iter().find(|d| d.work_id == "a").unwrap();
        assert_eq!(
            a.disposition,
            ActivationDisposition::RequireStopReconcileReplan
        );
    }

    #[test]
    fn cancel_expiry_session_end_do_not_prove_process_stopped() {
        let mut o = obs("a");
        o.has_active_claim = true;
        o.claim_cancelled = true;
        o.claim_expired = true;
        o.session_ended = true;
        o.external_process_stopped = false;
        let report = analyze_activation_impact(&["a".into()], &["a".into()], &[o], true);
        assert!(!report.allow_activation);
        let reasons = &report.decisions[0].reasons;
        assert!(reasons.iter().any(|r| r.contains("cancelled")));
        assert!(reasons.iter().any(|r| r.contains("expired")));
        assert!(reasons.iter().any(|r| r.contains("session_ended")));
    }

    #[test]
    fn explicit_stop_allows_affected_quiescent_activation() {
        let mut o = obs("a");
        o.has_active_claim = true;
        o.external_process_stopped = true;
        o.has_nonterminal_execution = false;
        let report = analyze_activation_impact(&["a".into()], &["a".into()], &[o], true);
        assert!(report.allow_activation);
    }

    #[test]
    fn read_set_intersection_expands_affected_set() {
        let mut reader = obs("b");
        reader.read_set_work_ids = vec!["a".into()];
        let report = analyze_activation_impact(
            &["a".into()],
            &["a".into(), "b".into(), "c".into()],
            &[obs("a"), reader, obs("c")],
            true,
        );
        assert!(report.affected_work_ids.contains(&"a".into()));
        assert!(report.affected_work_ids.contains(&"b".into()));
        assert_eq!(report.unrelated_work_ids, vec!["c".to_string()]);
    }

    #[test]
    fn selective_replan_reuses_ws032_without_second_graph() {
        let draft = TaskDraft {
            work_id: "a".into(),
            external_key: "a".into(),
            title: "A".into(),
            goals: vec!["g".into()],
            scope_paths: vec!["p".into()],
            acceptance: vec!["ok".into()],
            required_dependencies: vec![],
            completion_policy: "independent_review".into(),
            dependency_acceptance: None,
            definition_state: DraftDefinitionState::Enabled,
            workstream: None,
            split_from: None,
            split_children: vec![],
        };
        let app = planning_changes_as_selective_replan(
            "chg-1",
            "proj",
            "actor",
            "g1",
            "g2",
            "acc1",
            "acc2",
            &[DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft.clone()),
                after: draft,
            }],
            &["a".into(), "b".into()],
        )
        .unwrap();
        assert_eq!(app.change.status, PlanningChangeStatus::AffectedBlocked);
        assert!(app.unblocked_unrelated_work_ids.contains(&"b".into()));
    }

    #[test]
    fn request_replay_detects_completed_journal() {
        let j = WritebackJournal {
            version: 1,
            request_id: "req-1".into(),
            phase: WritebackPhase::Completed,
            candidate_id: "c1".into(),
            candidate_digest: "d1".into(),
            before_fingerprint: "b".into(),
            after_fingerprint: "a".into(),
            source_version: Some("sv".into()),
            activated_snapshot_id: Some("snap".into()),
            authority_epoch: Some("2".into()),
            approver_actor_id: Some("approver".into()),
            affected_work_ids: vec![],
            unrelated_work_ids: vec![],
            audit_receipt_id: Some("audit".into()),
        };
        assert!(same_request_already_completed(Some(&j), "req-1"));
        assert!(!same_request_already_completed(Some(&j), "other"));
    }
}
