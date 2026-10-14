mod support;

use awr_core::*;
use awr_store::{BranchFilter, EventQuery, Store};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use support::Fixture;

const FP_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FP_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const FP_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

struct Seed {
    goal_id: Id,
    plan_id: Id,
    rule_id: Id,
    work_id: Id,
    other_work_id: Id,
    decision_id: Id,
    evidence_id: Id,
    ambiguous_id: Id,
}

fn meta(
    fixture: &Fixture,
    id: Id,
    key: &str,
    source_revision: Revision,
    fp: &str,
) -> ProjectionMeta {
    ProjectionMeta {
        id,
        external_key: key.into(),
        revision: 1,
        source_ref: SourceRef {
            source_id: fixture.source.id,
            locator: format!("git://immutable-{source_revision}:{key}"),
            source_revision,
            source_fingerprint: fp.into(),
            pointer: Some(format!("/objects/{key}")),
            start_line: Some(1),
            end_line: Some(2),
            section_fingerprint: Some("section-1".into()),
        },
    }
}

fn work(meta: ProjectionMeta, next_action: &str) -> WorkItem {
    WorkItem {
        ordinary_completion: None,
        archived: false,
        meta,
        title: "Work".into(),
        kind: Some("feature".into()),
        owner: None,
        required: true,
        raw_status: "planned".into(),
        status: WorkStatus::Planned,
        priority: Some("P0".into()),
        milestone: Some("M6".into()),
        score: None,
        evidence_level: None,
        summary: String::new(),
        next_action: next_action.into(),
        blocker: None,
        acceptance: vec!["Mutation is reviewable".into()],
        tags: vec![],
        paths: vec![],
    }
}

fn seed(fixture: &mut Fixture) -> Seed {
    let seed = Seed {
        goal_id: Id::new(),
        plan_id: Id::new(),
        rule_id: Id::new(),
        work_id: Id::new(),
        other_work_id: Id::new(),
        decision_id: Id::new(),
        evidence_id: Id::new(),
        ambiguous_id: Id::new(),
    };
    let mut batch = ProjectionBatch::default();
    batch.goals = vec![
        Goal {
            meta: meta(fixture, seed.goal_id, "G-1", 1, FP_A),
            title: "Goal".into(),
            status: "active".into(),
            priority: None,
            success_criteria: vec!["Done".into()],
            summary: String::new(),
        },
        Goal {
            meta: meta(
                fixture,
                seed.ambiguous_id,
                &seed.goal_id.to_string(),
                1,
                FP_A,
            ),
            title: "Ambiguous by key".into(),
            status: "active".into(),
            priority: None,
            success_criteria: vec!["Done".into()],
            summary: String::new(),
        },
    ];
    batch.plans.push(Plan {
        meta: meta(fixture, seed.plan_id, "P-1", 1, FP_A),
        title: "Plan".into(),
        status: "active".into(),
        scope: vec!["G-1".into()],
        summary: String::new(),
        kind: Some("delivery".into()),
        acceptance: vec!["Done".into()],
    });
    batch.rules.push(Rule {
        meta: meta(fixture, seed.rule_id, "R-1", 1, FP_A),
        text: "Preserve bindings".into(),
        severity: Some(Severity::Hard),
        scope: None,
        unresolved: vec![],
    });
    batch.work_items = vec![
        work(meta(fixture, seed.work_id, "W-1", 1, FP_A), "Continue"),
        work(
            meta(fixture, seed.other_work_id, "W-2", 1, FP_A),
            "Continue other",
        ),
    ];
    batch.decisions.push(Decision {
        adoption: None,
        superseded_by: None,
        meta: meta(fixture, seed.decision_id, "D-1", 1, FP_A),
        title: "Decision".into(),
        status: DecisionStatus::Accepted,
        raw_status: "accepted".into(),
        decision: "Use proposals".into(),
        rationale: String::new(),
        affected_keys: vec!["W-1".into()],
        paths: vec![],
    });
    batch.evidence.push(Evidence {
        id: seed.evidence_id,
        project_id: fixture.project.id,
        work_item_id: Some(seed.work_id),
        external_key: "E-1".into(),
        evidence_type: "source_receipt".into(),
        level: EvidenceLevel::Designed,
        summary: "Source evidence".into(),
        locator: "ledger.yaml".into(),
        sha256: None,
        source_sha: None,
        section_fingerprint: None,
        command: None,
        scope: vec!["W-1".into()],
        source_ref: Some(meta(fixture, seed.evidence_id, "E-1", 1, FP_A).source_ref),
        branch_id: None,
        revision: 1,
        verified_at: None,
    });
    fixture.source = fixture
        .store
        .commit_source_projection(&fixture.source, FP_A, batch)
        .unwrap();
    seed
}

fn target(fixture: &Fixture, kind: EntityKind, reference: &str) -> Projected<Value> {
    fixture
        .store
        .mutation_target(fixture.project.id, kind, reference)
        .unwrap()
}

fn proposal_draft(target: &Projected<Value>, changes: Value, session: Option<Id>) -> MutationDraft {
    let meta: ProjectionMeta = serde_json::from_value(target.item.clone()).unwrap();
    MutationDraft {
        source_id: target.source.id,
        base_fingerprint: target.source.fingerprint.clone(),
        mutation_type: "update_fields".into(),
        patch: MutationPatch {
            version: 1,
            host_edit: None,
            work_action: None,
            target: MutationTarget {
                kind: match target.item.get("raw_status") {
                    Some(_) => EntityKind::WorkItem,
                    None => EntityKind::Goal,
                },
                meta,
            },
            source_config: target.source.config.clone(),
            intent: "Update one source-owned field".into(),
            changes,
        },
        created_by_session: session,
    }
}

fn draft_for_kind(
    target: &Projected<Value>,
    kind: EntityKind,
    changes: Value,
    session: Option<Id>,
) -> MutationDraft {
    let mut draft = proposal_draft(target, changes, session);
    draft.patch.target.kind = kind;
    draft
}

fn revision(fixture: &Fixture) -> Revision {
    fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision
}

fn start_session(fixture: &mut Fixture, key: Option<&str>) -> Session {
    start_session_on_branch(fixture, key, None)
}

fn start_session_on_branch(
    fixture: &mut Fixture,
    key: Option<&str>,
    branch_id: Option<Id>,
) -> Session {
    let expected = revision(fixture);
    fixture
        .store
        .start_session(
            fixture.project.id,
            expected,
            SessionDraft {
                work_item_key: key.map(str::to_string),
                agent_id: "mutation-test".into(),
                provider: "local".into(),
                model: "fixture".into(),
                branch_id,
                claim: false,
                claim_ttl_ms: None,
            },
        )
        .unwrap()
        .0
        .session
}

fn reproject_work(fixture: &mut Fixture, id: Id, fp: &str, next_action: &str) {
    let source_revision = fixture.source.revision + 1;
    let batch = ProjectionBatch {
        work_items: vec![work(
            meta(fixture, id, "W-1", source_revision, fp),
            next_action,
        )],
        ..ProjectionBatch::default()
    };
    fixture.source = fixture
        .store
        .commit_source_projection(&fixture.source, fp, batch)
        .unwrap();
}

#[test]
fn mutation_targets_cover_six_source_kinds_and_exclude_runtime_evidence() {
    let mut fixture = Fixture::new();
    let seed = seed(&mut fixture);
    for (kind, key, id) in [
        (EntityKind::Goal, "G-1", seed.goal_id),
        (EntityKind::Plan, "P-1", seed.plan_id),
        (EntityKind::Rule, "R-1", seed.rule_id),
        (EntityKind::WorkItem, "W-1", seed.work_id),
        (EntityKind::Decision, "D-1", seed.decision_id),
        (EntityKind::Evidence, "E-1", seed.evidence_id),
    ] {
        let by_key = target(&fixture, kind, key);
        assert_eq!(by_key.item["id"], json!(id));
        assert_eq!(by_key.source.id, fixture.source.id);
        if kind != EntityKind::Goal {
            let by_id = target(&fixture, kind, &id.to_string());
            assert_eq!(by_id.item["external_key"], json!(key));
        }
    }
    assert_eq!(
        target(&fixture, EntityKind::Goal, &seed.ambiguous_id.to_string()).item["id"],
        json!(seed.ambiguous_id)
    );

    let connection = Connection::open(fixture.root.join("state.db")).unwrap();
    let ambiguous_reference: String = connection
        .query_row(
            "SELECT external_key FROM goals WHERE id=?1",
            [seed.ambiguous_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        fixture.store.mutation_target(
            fixture.project.id,
            EntityKind::Goal,
            &ambiguous_reference
        ),
        Err(Error::InvalidInput(message)) if message.contains("ambiguous")
    ));

    let expected = revision(&fixture);
    let (runtime_evidence, _) = fixture
        .store
        .record_evidence(
            fixture.project.id,
            expected,
            EvidenceDraft {
                external_key: "runtime-only".into(),
                work_item_key: None,
                evidence_type: "report".into(),
                level: EvidenceLevel::Designed,
                summary: "Runtime evidence".into(),
                locator: "report.json".into(),
                sha256: None,
                source_sha: None,
                command: None,
                scope: vec!["*".into()],
                branch_id: None,
                verified_at: None,
            },
        )
        .unwrap();
    assert!(matches!(
        fixture.store.mutation_target(
            fixture.project.id,
            EntityKind::Evidence,
            &runtime_evidence.id.to_string()
        ),
        Err(Error::NotFound(_))
    ));

    let source_evidence = target(&fixture, EntityKind::Evidence, "E-1");
    let expected = revision(&fixture);
    let (proposal, _) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &source_evidence,
                EntityKind::Evidence,
                json!({"summary":"Updated source evidence"}),
                None,
            ),
        )
        .unwrap();
    assert_eq!(proposal.work_item_id, Some(seed.work_id));
}

#[test]
fn proposal_creation_enforces_project_source_target_and_session_bindings() {
    let mut fixture = Fixture::new();
    let seed = seed(&mut fixture);
    let work_session = start_session(&mut fixture, Some("W-1"));
    let other = target(&fixture, EntityKind::WorkItem, "W-2");
    let other_meta: ProjectionMeta = serde_json::from_value(other.item.clone()).unwrap();
    assert_ne!(other.source.locator, other_meta.source_ref.locator);
    let before = revision(&fixture);
    assert!(matches!(
        fixture.store.create_proposal(
            fixture.project.id,
            before,
            draft_for_kind(
                &other,
                EntityKind::WorkItem,
                json!({"next_action":"Mismatch"}),
                Some(work_session.id)
            )
        ),
        Err(Error::InvalidInput(message)) if message.contains("session")
    ));
    assert_eq!(revision(&fixture), before);

    let other_work_session = start_session(&mut fixture, Some("W-2"));
    let evidence = target(&fixture, EntityKind::Evidence, "E-1");
    let before = revision(&fixture);
    assert!(matches!(
        fixture.store.create_proposal(
            fixture.project.id,
            before,
            draft_for_kind(
                &evidence,
                EntityKind::Evidence,
                json!({"summary":"Wrong work session"}),
                Some(other_work_session.id)
            )
        ),
        Err(Error::InvalidInput(message)) if message.contains("session")
    ));
    assert_eq!(revision(&fixture), before);

    let project_session = start_session(&mut fixture, None);
    let expected = revision(&fixture);
    let (work_proposal, event) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &other,
                EntityKind::WorkItem,
                json!({"next_action":"Project session"}),
                Some(project_session.id),
            ),
        )
        .unwrap();
    assert_eq!(work_proposal.status, ProposalStatus::Draft);
    assert_eq!(work_proposal.expected_revision, expected);
    assert_eq!(work_proposal.revision, 1);
    assert_eq!(work_proposal.work_item_id, Some(seed.other_work_id));
    assert_eq!(work_proposal.created_by_session, Some(project_session.id));
    assert_eq!(event.event_type, "proposal.created");
    assert_eq!(event.session_id, Some(project_session.id));
    assert_eq!(event.work_item_id, Some(seed.other_work_id));

    let goal = target(&fixture, EntityKind::Goal, "G-1");
    let expected = revision(&fixture);
    let (goal_proposal, event) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &goal,
                EntityKind::Goal,
                json!({"summary":"Scoped through work"}),
                Some(work_session.id),
            ),
        )
        .unwrap();
    assert_eq!(goal_proposal.work_item_id, Some(seed.work_id));
    assert_eq!(event.session_id, Some(work_session.id));
    let (ready_goal, _) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            event.project_revision,
            goal_proposal.id,
            ProposalAction::Submit,
            "author",
            "Exact binding survives an ID and external-key collision",
        )
        .unwrap();
    assert_eq!(ready_goal.status, ProposalStatus::Ready);

    let mut other_project = Fixture::new();
    self::seed(&mut other_project);
    let foreign = target(&other_project, EntityKind::Goal, "G-1");
    let before = revision(&fixture);
    assert!(matches!(
        fixture.store.create_proposal(
            fixture.project.id,
            before,
            draft_for_kind(
                &foreign,
                EntityKind::Goal,
                json!({"summary":"Cross project"}),
                None
            )
        ),
        Err(Error::NotFound(_)) | Err(Error::SourceConflict(_))
    ));
    assert_eq!(revision(&fixture), before);
}

#[test]
fn proposal_lifecycle_is_cas_guarded_immutable_and_has_no_applied_bypass() {
    let mut fixture = Fixture::new();
    seed(&mut fixture);
    let work = target(&fixture, EntityKind::WorkItem, "W-1");
    let expected = revision(&fixture);
    let (created, created_event) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &work,
                EntityKind::WorkItem,
                json!({"next_action":"PRIVATE_PATCH_SENTINEL"}),
                None,
            ),
        )
        .unwrap();
    assert!(
        !created_event
            .payload
            .to_string()
            .contains("PRIVATE_PATCH_SENTINEL")
    );
    assert!(created_event.payload.get("patch").is_none());

    let after_create = created_event.project_revision;
    assert!(matches!(
        fixture.store.review_proposal(
            fixture.project.id,
            after_create - 1,
            created.id,
            ProposalAction::Submit,
            "author",
            "Stale project revision"
        ),
        Err(Error::RevisionConflict { .. })
    ));
    assert_eq!(revision(&fixture), after_create);
    assert!(matches!(
        fixture.store.review_proposal(
            fixture.project.id,
            after_create,
            created.id,
            ProposalAction::Approve,
            "reviewer",
            "Cannot approve a draft"
        ),
        Err(Error::InvalidTransition(_))
    ));
    assert_eq!(revision(&fixture), after_create);

    let (ready, ready_event) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            after_create,
            created.id,
            ProposalAction::Submit,
            "author",
            "Ready for review",
        )
        .unwrap();
    assert_eq!(ready.status, ProposalStatus::Ready);
    assert_eq!(ready.revision, 2);
    assert_eq!(ready_event.event_type, "proposal.ready");
    assert_eq!(ready_event.payload["from"], json!(ProposalStatus::Draft));
    assert_eq!(ready_event.payload["to"], json!(ProposalStatus::Ready));

    let (approved, approved_event) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            ready_event.project_revision,
            created.id,
            ProposalAction::Approve,
            "reviewer",
            "Bindings reviewed",
        )
        .unwrap();
    assert_eq!(approved.status, ProposalStatus::Approved);
    assert_eq!(approved.revision, 3);
    assert_eq!(approved_event.event_type, "proposal.approved");

    let (required, required_event) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            approved_event.project_revision,
            created.id,
            ProposalAction::RequireManualApply,
            "runtime",
            "No source writer is available",
        )
        .unwrap();
    assert_eq!(required.status, ProposalStatus::Approved);
    assert_eq!(required.revision, 4);
    assert_eq!(required_event.event_type, "proposal.required");
    assert_eq!(required.expected_revision, created.expected_revision);
    assert_eq!(required.source_id, created.source_id);
    assert_eq!(required.base_fingerprint, created.base_fingerprint);
    assert_eq!(required.patch, created.patch);

    let (rejected, rejected_event) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            required_event.project_revision,
            created.id,
            ProposalAction::Reject,
            "reviewer",
            "Do not apply",
        )
        .unwrap();
    assert_eq!(rejected.status, ProposalStatus::Rejected);
    assert_eq!(rejected.revision, 5);
    assert_eq!(rejected_event.event_type, "proposal.rejected");
    assert_ne!(rejected.status, ProposalStatus::Applied);
    assert!(matches!(
        fixture.store.review_proposal(
            fixture.project.id,
            rejected_event.project_revision,
            created.id,
            ProposalAction::Fail,
            "runtime",
            "No bypass"
        ),
        Err(Error::InvalidTransition(_))
    ));
    assert_eq!(revision(&fixture), rejected_event.project_revision);

    for (actor, reason) in [
        ("", "reason".to_string()),
        (&"x".repeat(257), "reason".to_string()),
        ("actor", "x".repeat(16_385)),
    ] {
        assert!(matches!(
            fixture.store.review_proposal(
                fixture.project.id,
                rejected_event.project_revision,
                created.id,
                ProposalAction::Reject,
                actor,
                &reason
            ),
            Err(Error::InvalidInput(_))
        ));
    }

    let mut forged = EventDraft::new("proposal.applied", "Forged source write receipt");
    forged.payload = json!({"proposal_id":created.id});
    assert!(matches!(
        fixture
            .store
            .append_event(fixture.project.id, rejected_event.project_revision, forged),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(revision(&fixture), rejected_event.project_revision);
}

#[test]
fn forward_actions_revalidate_but_cleanup_actions_accept_stale_or_legacy_rows() {
    let mut fixture = Fixture::new();
    let seed = seed(&mut fixture);
    let work = target(&fixture, EntityKind::WorkItem, "W-1");
    let expected = revision(&fixture);
    let (stale_proposal, event) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &work,
                EntityKind::WorkItem,
                json!({"next_action":"Stale"}),
                None,
            ),
        )
        .unwrap();
    fixture.source = fixture
        .store
        .mark_source_freshness(&fixture.source, Freshness::Stale)
        .unwrap();
    let before = revision(&fixture);
    assert!(matches!(
        fixture.store.review_proposal(
            fixture.project.id,
            before,
            stale_proposal.id,
            ProposalAction::Submit,
            "author",
            "Try stale source"
        ),
        Err(Error::SourceStale(_))
    ));
    assert_eq!(revision(&fixture), before);
    assert_eq!(
        fixture
            .store
            .proposal(fixture.project.id, stale_proposal.id)
            .unwrap()
            .status,
        ProposalStatus::Draft
    );
    let (conflicted, _) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            before,
            stale_proposal.id,
            ProposalAction::Conflict,
            "runtime",
            "Source became stale",
        )
        .unwrap();
    assert_eq!(conflicted.status, ProposalStatus::Conflict);
    assert_eq!(conflicted.expected_revision, event.project_revision - 1);

    reproject_work(&mut fixture, seed.work_id, FP_B, "Fresh again");
    let work = target(&fixture, EntityKind::WorkItem, "W-1");
    let expected = revision(&fixture);
    let (legacy, _) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &work,
                EntityKind::WorkItem,
                json!({"next_action":"Legacy"}),
                None,
            ),
        )
        .unwrap();
    let connection = Connection::open(fixture.root.join("state.db")).unwrap();
    connection
        .execute(
            "UPDATE mutation_proposals SET patch_json='{}' WHERE id=?1",
            [legacy.id.to_string()],
        )
        .unwrap();
    drop(connection);
    let before = revision(&fixture);
    assert_eq!(
        fixture
            .store
            .proposal(fixture.project.id, legacy.id)
            .unwrap()
            .patch,
        json!({})
    );
    assert!(matches!(
        fixture.store.review_proposal(
            fixture.project.id,
            before,
            legacy.id,
            ProposalAction::Submit,
            "author",
            "Legacy patch"
        ),
        Err(Error::InvalidInput(_))
    ));
    let (rejected, _) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            before,
            legacy.id,
            ProposalAction::Reject,
            "reviewer",
            "Clean up legacy row",
        )
        .unwrap();
    assert_eq!(rejected.status, ProposalStatus::Rejected);

    let work = target(&fixture, EntityKind::WorkItem, "W-1");
    let expected = revision(&fixture);
    let (changed, _) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &work,
                EntityKind::WorkItem,
                json!({"next_action":"Before projection change"}),
                None,
            ),
        )
        .unwrap();
    reproject_work(&mut fixture, seed.work_id, FP_C, "Projection changed");
    let before = revision(&fixture);
    assert!(matches!(
        fixture.store.review_proposal(
            fixture.project.id,
            before,
            changed.id,
            ProposalAction::Submit,
            "author",
            "Revalidate changed source"
        ),
        Err(Error::SourceConflict(_))
    ));
    assert_eq!(revision(&fixture), before);
    let (failed, _) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            before,
            changed.id,
            ProposalAction::Fail,
            "runtime",
            "Projection changed before submit",
        )
        .unwrap();
    assert_eq!(failed.status, ProposalStatus::Failed);
}

#[test]
fn proposal_reads_are_bounded_and_event_failure_rolls_back_status_and_project_revision() {
    let mut fixture = Fixture::new();
    seed(&mut fixture);
    let work = target(&fixture, EntityKind::WorkItem, "W-1");
    let expected = revision(&fixture);
    let (first, event) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &work,
                EntityKind::WorkItem,
                json!({"next_action":"First"}),
                None,
            ),
        )
        .unwrap();
    let work = target(&fixture, EntityKind::WorkItem, "W-1");
    let (second, _) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            event.project_revision,
            draft_for_kind(
                &work,
                EntityKind::WorkItem,
                json!({"next_action":"Second"}),
                None,
            ),
        )
        .unwrap();
    let connection = Connection::open(fixture.root.join("state.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_proposal_receipt BEFORE INSERT ON events
             WHEN NEW.event_type='proposal.ready'
             BEGIN SELECT RAISE(ABORT,'test proposal receipt failure'); END;",
        )
        .unwrap();
    drop(connection);
    let before = revision(&fixture);
    assert!(matches!(
        fixture.store.review_proposal(
            fixture.project.id,
            before,
            first.id,
            ProposalAction::Submit,
            "author",
            "Trigger rollback"
        ),
        Err(Error::Storage(message)) if message.contains("test proposal receipt failure")
    ));
    assert_eq!(revision(&fixture), before);
    let rolled_back = fixture
        .store
        .proposal(fixture.project.id, first.id)
        .unwrap();
    assert_eq!(rolled_back.status, ProposalStatus::Draft);
    assert_eq!(rolled_back.revision, 1);

    let connection = Connection::open(fixture.root.join("state.db")).unwrap();
    connection
        .execute_batch("DROP TRIGGER reject_proposal_receipt")
        .unwrap();
    drop(connection);
    let (ready, receipt) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            before,
            first.id,
            ProposalAction::Submit,
            "author",
            "Retry after storage recovery",
        )
        .unwrap();
    assert_eq!(ready.revision, 2);
    let read_revision = revision(&fixture);
    let all = fixture
        .store
        .proposals(fixture.project.id, None, 100)
        .unwrap();
    assert_eq!(all[0].id, first.id);
    assert_eq!(all[1].id, second.id);
    let ready_rows = fixture
        .store
        .proposals(fixture.project.id, Some(ProposalStatus::Ready), 10)
        .unwrap();
    assert_eq!(ready_rows.len(), 1);
    assert_eq!(ready_rows[0].id, ready.id);
    assert_eq!(ready_rows[0].revision, ready.revision);
    assert!(matches!(
        fixture.store.proposals(fixture.project.id, None, 0),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        fixture.store.proposals(fixture.project.id, None, 101),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(revision(&fixture), read_revision);

    let readonly = Store::open_readonly(&fixture.root.join("state.db")).unwrap();
    assert_eq!(
        readonly
            .proposal(fixture.project.id, first.id)
            .unwrap()
            .status,
        ProposalStatus::Ready
    );
    assert_eq!(
        readonly
            .proposals(fixture.project.id, None, 1)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(revision(&fixture), receipt.project_revision);
}

#[test]
fn review_receipts_retain_creator_branch_without_impersonating_creator_session() {
    let mut fixture = Fixture::new();
    seed(&mut fixture);
    let branch_id = Id::new();
    let connection = Connection::open(fixture.root.join("state.db")).unwrap();
    connection
        .execute(
            "INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision)
             VALUES(?1,?2,'proposal-branch',0,'active',1)",
            params![branch_id.to_string(), fixture.project.id.to_string()],
        )
        .unwrap();
    drop(connection);
    let creator = start_session_on_branch(&mut fixture, Some("W-1"), Some(branch_id));
    let work = target(&fixture, EntityKind::WorkItem, "W-1");
    let expected = revision(&fixture);
    let (proposal, created_event) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &work,
                EntityKind::WorkItem,
                json!({"next_action":"Branch scoped"}),
                Some(creator.id),
            ),
        )
        .unwrap();
    assert_eq!(created_event.branch_id, Some(branch_id));
    let (_, ended_event) = fixture
        .store
        .end_session(
            fixture.project.id,
            created_event.project_revision,
            creator.id,
            SessionOutcome::Ended,
        )
        .unwrap();

    let mut expected = ended_event.project_revision;
    let mut receipts = Vec::new();
    for (action, actor, reason) in [
        (
            ProposalAction::Submit,
            "author",
            "Submit from another actor",
        ),
        (
            ProposalAction::Approve,
            "reviewer",
            "Approve on retained branch",
        ),
        (
            ProposalAction::RequireManualApply,
            "runtime",
            "Writer is unavailable",
        ),
        (ProposalAction::Reject, "reviewer", "Close without applying"),
    ] {
        let (_, event) = fixture
            .store
            .review_proposal(
                fixture.project.id,
                expected,
                proposal.id,
                action,
                actor,
                reason,
            )
            .unwrap();
        assert_eq!(event.branch_id, Some(branch_id));
        assert_eq!(event.session_id, None);
        expected = event.project_revision;
        receipts.push(event.event_type);
    }
    for event_type in receipts {
        let page = fixture
            .store
            .query_events(
                fixture.project.id,
                &EventQuery {
                    branch: BranchFilter::Branch(branch_id),
                    event_type: Some(event_type),
                    ..EventQuery::default()
                },
            )
            .unwrap();
        assert_eq!(page.events.len(), 1);
    }
}

#[test]
fn a_conflict_proposal_can_be_explicitly_rejected() {
    let mut fixture = Fixture::new();
    seed(&mut fixture);
    let work = target(&fixture, EntityKind::WorkItem, "W-1");
    let expected = revision(&fixture);
    let (created, _) = fixture
        .store
        .create_proposal(
            fixture.project.id,
            expected,
            draft_for_kind(
                &work,
                EntityKind::WorkItem,
                json!({"next_action":"Blocked elsewhere"}),
                None,
            ),
        )
        .unwrap();
    let (ready, _) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            revision(&fixture),
            created.id,
            ProposalAction::Submit,
            "author",
            "Ready for review",
        )
        .unwrap();
    assert_eq!(ready.status, ProposalStatus::Ready);
    // An observed source conflict moves the proposal to Conflict.
    let (conflicted, conflict_event) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            revision(&fixture),
            created.id,
            ProposalAction::Conflict,
            "reviewer",
            "Source bytes moved on",
        )
        .unwrap();
    assert_eq!(conflicted.status, ProposalStatus::Conflict);
    assert_eq!(conflict_event.event_type, "proposal.conflict");
    // Rejection is the auditable exit; the proposal cannot be reopened afterwards.
    let (rejected, reject_event) = fixture
        .store
        .review_proposal(
            fixture.project.id,
            revision(&fixture),
            created.id,
            ProposalAction::Reject,
            "reviewer",
            "Superseded by a fresh proposal from current facts",
        )
        .unwrap();
    assert_eq!(rejected.status, ProposalStatus::Rejected);
    assert_eq!(reject_event.event_type, "proposal.rejected");
    assert!(matches!(
        fixture.store.review_proposal(
            fixture.project.id,
            revision(&fixture),
            created.id,
            ProposalAction::Reject,
            "reviewer",
            "Already terminal"
        ),
        Err(Error::InvalidTransition(_))
    ));
}
