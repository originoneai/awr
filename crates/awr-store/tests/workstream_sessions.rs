use awr_core::*;
use awr_store::{SourceRegistration, Store};
use rusqlite::{Connection, params};
use serde_json::json;
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
    source: Source,
    scopes: [Id; 2],
    works: [Id; 2],
    batch: ProjectionBatch,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-scope-session-{}", Id::new()));
        fs::create_dir(&root).unwrap();
        let mut store = Store::open(&root.join("state.db")).unwrap();
        let project = store
            .register_project(&root, "fixture", "Fixture")
            .unwrap()
            .id;
        let source = store
            .register_source(
                project,
                &SourceRegistration {
                    domain: "ledger",
                    role: "primary",
                    locator: "file:///fixture/work.yaml",
                    format: "yaml",
                    adapter: "yaml-workstream-ledger-v1",
                },
            )
            .unwrap();
        let scopes = [Id::new(), Id::new()];
        let works = [Id::new(), Id::new()];
        let work_items = works.iter().enumerate().map(|(i, id)| serde_json::from_value(json!({
            "id":id,"external_key":format!("W{i}"),"revision":1,
            "source_ref":{"source_id":source.id,"locator":source.locator,"source_revision":source.revision+1,"source_fingerprint":"v1"},
            "title":"Synthetic work","kind":null,"owner":null,"required":true,
            "raw_status":"ready","status":"ready","priority":null,"milestone":null,"score":null,"evidence_level":null,
            "summary":"Implement synthetic component","next_action":"Implement","blocker":null,"acceptance":[],"tags":[],"paths":[]
        })).unwrap()).collect();
        let catalog = WorkstreamCatalog {
            version: 1,
            project_id: project.to_string(),
            legacy_default: None,
            workstreams: scopes
                .iter()
                .enumerate()
                .map(|(i, id)| Workstream {
                    id: *id,
                    project_id: project.to_string(),
                    external_key: format!("S{i}"),
                    title: format!("Scope {i}"),
                    state: WorkstreamState::Active,
                    authority_version: 1,
                    goal_keys: vec![],
                    acceptance_contracts: vec![],
                })
                .collect(),
        };
        let batch = ProjectionBatch {
            work_items,
            workstream_projection: Some(WorkstreamProjection {
                catalog,
                ownership: works
                    .iter()
                    .zip(scopes)
                    .map(|(work, scope)| WorkstreamWorkBinding {
                        project_id: project.to_string(),
                        work_item_id: work.to_string(),
                        workstream_id: scope,
                    })
                    .collect(),
            }),
            ..Default::default()
        };
        let source = store
            .commit_source_projection(&source, "v1", batch.clone())
            .unwrap();
        Self {
            root,
            store,
            project,
            source,
            scopes,
            works,
            batch,
        }
    }
    fn rev(&self) -> Revision {
        self.store.project(self.project).unwrap().project_revision
    }
    fn draft(key: Option<&str>, claim: bool) -> SessionDraft {
        SessionDraft {
            work_item_key: key.map(str::to_owned),
            agent_id: "reviewer".into(),
            provider: "fixture".into(),
            model: "fixture".into(),
            branch_id: None,
            claim,
            claim_ttl_ms: None,
        }
    }
    fn start(&mut self, key: &str, claim: bool) -> SessionStarted {
        self.store
            .start_session(self.project, self.rev(), Self::draft(Some(key), claim))
            .unwrap()
            .0
    }
    fn checkpoint_draft() -> CheckpointDraft {
        CheckpointDraft {
            context_hash: "a".repeat(64),
            digest: "Retained work".into(),
            next_action: "Continue".into(),
            open_loops: vec!["Review".into()],
            changed_entities: vec![],
        }
    }
    fn checkpoint(&mut self, session: Id) -> Checkpoint {
        self.store
            .create_checkpoint(self.project, self.rev(), session, Self::checkpoint_draft())
            .unwrap()
            .0
    }
    fn end(&mut self, session: Id) {
        self.store
            .end_session(self.project, self.rev(), session, SessionOutcome::Ended)
            .unwrap();
    }
    fn resume_draft(session: Id, checkpoint: Option<Id>) -> SessionResumeDraft {
        SessionResumeDraft {
            from_session_id: session,
            checkpoint_id: checkpoint,
            agent_id: "successor".into(),
            provider: "fixture".into(),
            model: "fixture".into(),
            claim: ResumeClaim::Inherit,
            claim_ttl_ms: None,
            prepared_context_hash: "b".repeat(64),
        }
    }
    fn candidate(&self, fingerprint: &str) -> ProjectionBatch {
        let mut batch = self.batch.clone();
        for work in &mut batch.work_items {
            work.meta.source_ref.source_revision = self.source.revision + 1;
            work.meta.source_ref.source_fingerprint = fingerprint.into();
        }
        batch
    }
    fn movement(&self) -> WorkstreamMove {
        WorkstreamMove {
            work_item_id: self.works[0].to_string(),
            from: self.scopes[0],
            to: self.scopes[1],
            expected_ownership_revision: 1,
        }
    }
    fn move_candidate(&mut self) -> ProjectionBatch {
        self.source = self
            .store
            .mark_source_freshness(&self.source, Freshness::Stale)
            .unwrap();
        let mut candidate = self.candidate("moved");
        candidate.workstream_projection.as_mut().unwrap().ownership[0].workstream_id =
            self.scopes[1];
        candidate
    }
    fn sql(&self, sql: &str) {
        Connection::open(self.root.join("state.db"))
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn progress_checkpoint_requires_current_workstream_ownership() {
    let mut f = Fixture::new();
    let session = f.start("W0", false);
    let checkpoint = f.checkpoint(session.session.id);
    assert_eq!(
        f.store
            .latest_work_progress_checkpoint(f.project, f.works[0], None)
            .unwrap()
            .unwrap()
            .id,
        checkpoint.id
    );
    assert!(
        f.store
            .latest_work_progress_checkpoint(f.project, f.works[1], None)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .latest_work_progress_checkpoint(Id::new(), f.works[0], None)
            .unwrap()
            .is_none()
    );
    // Move through the reviewed source operation; the old immutable session
    // binding remains history, not progress for the new ownership epoch.
    f.end(session.session.id);
    let mut movement = f.movement();
    movement.expected_ownership_revision = f
        .store
        .workstream_ownership(f.project, f.works[0])
        .unwrap()
        .revision;
    let candidate = f.move_candidate();
    f.source = f
        .store
        .commit_source_projection_with_moves(f.rev(), &f.source, "moved", candidate, &[movement])
        .unwrap();
    assert!(
        f.store
            .latest_work_progress_checkpoint(f.project, f.works[0], None)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.store.checkpoint(f.project, checkpoint.id).unwrap().id,
        checkpoint.id
    );
    // Returning to the same workstream must not revive an older ownership epoch.
    let movement = WorkstreamMove {
        work_item_id: f.works[0].to_string(),
        from: f.scopes[1],
        to: f.scopes[0],
        expected_ownership_revision: f
            .store
            .workstream_ownership(f.project, f.works[0])
            .unwrap()
            .revision,
    };
    f.source = f
        .store
        .mark_source_freshness(&f.source, Freshness::Stale)
        .unwrap();
    let candidate = f.candidate("returned");
    f.source = f
        .store
        .commit_source_projection_with_moves(f.rev(), &f.source, "returned", candidate, &[movement])
        .unwrap();
    assert!(
        f.store
            .latest_work_progress_checkpoint(f.project, f.works[0], None)
            .unwrap()
            .is_none()
    );
    let successor = f.start("W0", false);
    let current = f.checkpoint(successor.session.id);
    assert_eq!(
        f.store
            .latest_work_progress_checkpoint(f.project, f.works[0], None)
            .unwrap()
            .unwrap()
            .id,
        current.id
    );
}
fn binding(client: &str, conversation: &str) -> McpSessionBinding {
    McpSessionBinding {
        client: client.into(),
        conversation: conversation.into(),
    }
}

#[test]
fn work_identity_controls_binding_and_ambiguous_workless_start_rolls_back() {
    let mut f = Fixture::new();
    let revision = f.rev();
    assert!(matches!(
        f.store
            .start_session(f.project, revision, Fixture::draft(None, false)),
        Err(Error::Workstream(WorkstreamError::ScopeRequired))
    ));
    assert!(matches!(
        f.store.start_session_in_workstream(
            f.project,
            revision,
            Fixture::draft(Some("W0"), true),
            None,
            f.scopes[1]
        ),
        Err(Error::Workstream(WorkstreamError::BindingMismatch))
    ));
    assert_eq!(f.rev(), revision);
    assert!(f.store.sessions(f.project, false, 10).unwrap().is_empty());
    let a = f.start("W0", true);
    let b = f.start("W1", true);
    let captured = f.store.session_workstream(f.project, a.session.id).unwrap();
    assert_eq!(captured.workstream_id, Some(f.scopes[0]));
    assert_eq!(captured.ownership_revision, Some(2)); // Initial explicit import changes legacy ownership once.
    assert_eq!(
        f.store
            .claim_workstream(f.project, a.claim.unwrap().id)
            .unwrap(),
        captured
    );
    assert_eq!(
        f.store
            .session_workstream(f.project, b.session.id)
            .unwrap()
            .workstream_id,
        Some(f.scopes[1])
    );
}

#[test]
fn conversation_defaults_are_client_local_and_never_rebind_sessions() {
    let mut f = Fixture::new();
    let a = binding("client-a", "same-conversation");
    let b = binding("client-b", "same-conversation");
    let c = binding("client-a", "other-conversation");
    for (binding, scope) in [(a.clone(), f.scopes[0]), (b.clone(), f.scopes[1])] {
        f.store
            .select_conversation_workstream(f.project, f.rev(), binding, scope)
            .unwrap();
    }
    assert_eq!(
        f.store.conversation_workstream(f.project, &c).unwrap(),
        None
    );
    let (started, _) = f
        .store
        .start_bound_session(f.project, f.rev(), Fixture::draft(None, false), a.clone())
        .unwrap();
    f.store
        .select_conversation_workstream(f.project, f.rev(), a.clone(), f.scopes[1])
        .unwrap();
    assert_eq!(
        f.store
            .session_workstream(f.project, started.session.id)
            .unwrap()
            .workstream_id,
        Some(f.scopes[0])
    );
    assert_eq!(
        f.store.conversation_workstream(f.project, &b).unwrap(),
        Some(f.scopes[1])
    );
    let (work, _) = f
        .store
        .start_bound_session(f.project, f.rev(), Fixture::draft(Some("W0"), false), c)
        .unwrap();
    assert_eq!(
        f.store
            .session_workstream(f.project, work.session.id)
            .unwrap()
            .workstream_id,
        Some(f.scopes[0])
    );
    let before = f.rev();
    let mut forged = EventDraft::new("workstream.conversation_selected", "Forged selection");
    forged.payload = json!({"binding":a,"workstream_id":f.scopes[1]});
    assert!(f.store.append_event(f.project, before, forged).is_err());
    assert_eq!(f.rev(), before);
}

#[test]
fn resume_ignores_changed_default_and_handoff_keeps_original_checkpoint_scope() {
    let mut f = Fixture::new();
    let client = binding("client-a", "original");
    let (sender, _) = f
        .store
        .start_bound_session(
            f.project,
            f.rev(),
            Fixture::draft(Some("W0"), true),
            client.clone(),
        )
        .unwrap();
    let cp = f.checkpoint(sender.session.id);
    f.store
        .select_conversation_workstream(f.project, f.rev(), client, f.scopes[1])
        .unwrap();
    let (resumed, _) = f
        .store
        .resume_session(
            f.project,
            f.rev(),
            Fixture::resume_draft(sender.session.id, Some(cp.id)),
        )
        .unwrap();
    let receiver = f.start("W0", false);
    let cp2 = f.checkpoint(resumed.session.id);
    let (handoff, _) = f
        .store
        .handoff(
            f.project,
            f.rev(),
            resumed.session.id,
            Some(receiver.session.id),
            None,
        )
        .unwrap();
    assert_eq!(
        f.store
            .session_workstream(f.project, receiver.session.id)
            .unwrap()
            .workstream_id,
        Some(f.scopes[0])
    );
    assert_eq!(
        f.store
            .checkpoint_workstream(f.project, cp.id)
            .unwrap()
            .session_id,
        sender.session.id.to_string()
    );
    assert_eq!(handoff.checkpoint.id, cp2.id);
    assert_eq!(
        f.store
            .incoming_handoff(f.project, receiver.session.id)
            .unwrap()
            .unwrap()
            .id,
        cp2.id
    );
    assert_eq!(
        f.store
            .claim_workstream(f.project, handoff.transferred_claim.unwrap().id)
            .unwrap()
            .workstream_id,
        Some(f.scopes[0])
    );
}

#[test]
fn paused_or_stale_authority_blocks_execution_but_allows_checkpoint_and_cleanup() {
    for pause in [false, true] {
        let mut f = Fixture::new();
        let sender = f.start("W0", true);
        let receiver = f.start("W0", false);
        let cp = f.checkpoint(sender.session.id);
        if pause {
            let mut candidate = f.candidate("paused");
            let scope = &mut candidate
                .workstream_projection
                .as_mut()
                .unwrap()
                .catalog
                .workstreams[0];
            scope.state = WorkstreamState::Paused;
            scope.authority_version += 1;
            f.source = f
                .store
                .commit_source_projection(&f.source, "paused", candidate)
                .unwrap();
        } else {
            f.source = f
                .store
                .mark_source_freshness(&f.source, Freshness::Unavailable)
                .unwrap();
        }
        let before = f.rev();
        assert!(
            f.store
                .resume_session(
                    f.project,
                    before,
                    Fixture::resume_draft(sender.session.id, Some(cp.id))
                )
                .is_err()
        );
        assert!(
            f.store
                .handoff(
                    f.project,
                    before,
                    sender.session.id,
                    Some(receiver.session.id),
                    None
                )
                .is_err()
        );
        assert_eq!(f.rev(), before);
        f.checkpoint(sender.session.id);
        f.store
            .handoff(f.project, f.rev(), sender.session.id, None, None)
            .unwrap();
        f.end(receiver.session.id);
        assert_eq!(
            f.store
                .claim_workstream(f.project, sender.claim.unwrap().id)
                .unwrap()
                .workstream_id,
            Some(f.scopes[0])
        );
    }
}

#[test]
fn reviewed_move_preserves_history_and_rejects_stale_resumes_and_receipts() {
    let mut f = Fixture::new();
    let old = f.start("W0", true);
    let cp = f.checkpoint(old.session.id);
    f.end(old.session.id);
    let old_scope = f
        .store
        .session_workstream(f.project, old.session.id)
        .unwrap();
    let reviewed = f.store.workstream_ownership(f.project, f.works[0]).unwrap();
    assert_eq!(Some(reviewed.revision), old_scope.ownership_revision);
    assert_eq!(
        Some(reviewed.binding.workstream_id),
        old_scope.workstream_id
    );

    let candidate = f.move_candidate();
    let mut movement = f.movement();
    movement.expected_ownership_revision = old_scope.ownership_revision.unwrap();
    let before = f.rev();
    assert!(
        f.store
            .commit_source_projection(&f.source, "moved", candidate.clone())
            .is_err()
    );
    assert_eq!(f.rev(), before);
    let mut invalid = movement.clone();
    invalid.expected_ownership_revision += 1;
    assert!(
        f.store
            .commit_source_projection_with_moves(
                before,
                &f.source,
                "moved",
                candidate.clone(),
                &[invalid]
            )
            .is_err()
    );
    assert!(
        f.store
            .commit_source_projection_with_moves(
                before - 1,
                &f.source,
                "moved",
                candidate.clone(),
                &[movement.clone()]
            )
            .is_err()
    );
    assert!(
        f.store
            .commit_source_projection_with_moves(
                before,
                &f.source,
                "moved",
                candidate.clone(),
                &[movement.clone(), movement.clone()]
            )
            .is_err()
    );
    assert_eq!(f.rev(), before);
    f.sql("CREATE TRIGGER reject_move_receipt BEFORE INSERT ON events WHEN NEW.event_type='source.projected' BEGIN SELECT RAISE(ABORT,'fixture rejection'); END;");
    assert!(
        f.store
            .commit_source_projection_with_moves(
                before,
                &f.source,
                "moved",
                candidate.clone(),
                &[movement.clone()]
            )
            .is_err()
    );
    assert_eq!(f.rev(), before);
    assert_eq!(
        f.store
            .session_workstream(f.project, old.session.id)
            .unwrap(),
        old_scope
    );
    f.sql("DROP TRIGGER reject_move_receipt;");
    f.source = f
        .store
        .commit_source_projection_with_moves(before, &f.source, "moved", candidate, &[movement])
        .unwrap();
    assert_eq!(
        f.store
            .workstream_binding(f.project, f.works[0])
            .unwrap()
            .workstream_id,
        f.scopes[1]
    );
    assert_eq!(
        f.store.checkpoint_workstream(f.project, cp.id).unwrap(),
        old_scope
    );
    assert_eq!(
        f.store
            .claim_workstream(f.project, old.claim.unwrap().id)
            .unwrap(),
        old_scope
    );
    assert!(
        f.store
            .latest_work_checkpoint(f.project, f.works[0], None)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .resume_candidates(f.project, Some("W0"), None)
            .unwrap()
            .is_empty()
    );
    assert!(
        f.store
            .resume_session(
                f.project,
                f.rev(),
                Fixture::resume_draft(old.session.id, Some(cp.id))
            )
            .is_err()
    );
    let new = f.start("W0", true);
    assert!(
        f.store
            .recovery_checkpoint(f.project, new.session.id)
            .unwrap()
            .is_none()
    );
    let current = f
        .store
        .session_workstream(f.project, new.session.id)
        .unwrap();
    assert_eq!(current.workstream_id, Some(f.scopes[1]));
    assert_eq!(
        current.ownership_revision,
        old_scope.ownership_revision.map(|v| v + 1)
    );
    let db = Connection::open(f.root.join("state.db")).unwrap();
    assert!(
        db.execute(
            "UPDATE session_workstreams SET workstream_id=?1 WHERE session_id=?2",
            params![f.scopes[1].to_string(), old.session.id.to_string()]
        )
        .is_err()
    );
    assert!(
        db.execute(
            "UPDATE sessions SET work_item_id=?1 WHERE id=?2",
            params![f.works[1].to_string(), old.session.id.to_string()]
        )
        .is_err()
    );
    assert!(f.store.doctor().unwrap().ok);
}

#[test]
fn moves_wait_for_sessions_checkpoint_recovery_and_unverified_execution_outcomes() {
    for blocker in [
        "session",
        "incomplete",
        "interrupted",
        "checkpoint",
        "external",
        "managed",
    ] {
        let mut f = Fixture::new();
        let started = f.start("W0", true);
        let scope = f
            .store
            .session_workstream(f.project, started.session.id)
            .unwrap();
        if blocker == "checkpoint" {
            f.store
                .begin_checkpoint_save(
                    f.project,
                    f.rev(),
                    started.session.id,
                    Fixture::checkpoint_draft(),
                )
                .unwrap();
        }
        if ["external", "managed"].contains(&blocker) {
            let managed = blocker == "managed";
            f.store
                .register_execution(
                    f.project,
                    f.rev(),
                    started.session.id,
                    ExecutionIntent {
                        operation_key: "fixture-operation".into(),
                        purpose: "Synthetic fixture operation".into(),
                        executor: if managed {
                            ExecutorKind::ManagedLocal
                        } else {
                            ExecutorKind::External
                        },
                        command: if managed {
                            vec!["synthetic-worker".into()]
                        } else {
                            vec![]
                        },
                        cwd: f.root.to_string_lossy().into_owned(),
                        external_reference: (!managed).then(|| "fixture-external-job".into()),
                        inputs: Vec::new(),
                        input_binding: awr_core::InputBinding::None,
                    },
                )
                .unwrap();
        }
        match blocker {
            "session" => {}
            "incomplete" => {
                f.store
                    .end_session(
                        f.project,
                        f.rev(),
                        started.session.id,
                        SessionOutcome::Incomplete,
                    )
                    .unwrap();
            }
            "interrupted" => {
                f.store
                    .end_session(
                        f.project,
                        f.rev(),
                        started.session.id,
                        SessionOutcome::Interrupted,
                    )
                    .unwrap();
            }
            _ => f.end(started.session.id),
        }
        let candidate = f.move_candidate();
        let mut movement = f.movement();
        movement.expected_ownership_revision = scope.ownership_revision.unwrap();
        let before = f.rev();
        assert!(
            matches!(
                f.store.commit_source_projection_with_moves(
                    before,
                    &f.source,
                    "moved",
                    candidate,
                    &[movement]
                ),
                Err(Error::InvalidTransition(_))
            ),
            "{blocker}"
        );
        assert_eq!(f.rev(), before);
        assert_eq!(
            f.store
                .session_workstream(f.project, started.session.id)
                .unwrap(),
            scope
        );
    }
}

#[test]
fn moves_accept_sessions_after_their_recovery_responsibility_is_resolved() {
    for recovery in ["resume", "handoff"] {
        let mut f = Fixture::new();
        let started = f.start("W0", true);
        let scope = f
            .store
            .session_workstream(f.project, started.session.id)
            .unwrap();
        let checkpoint = f.checkpoint(started.session.id);
        let successor = if recovery == "resume" {
            f.store
                .end_session(
                    f.project,
                    f.rev(),
                    started.session.id,
                    SessionOutcome::Incomplete,
                )
                .unwrap();
            f.store
                .resume_session(
                    f.project,
                    f.rev(),
                    Fixture::resume_draft(started.session.id, Some(checkpoint.id)),
                )
                .unwrap()
                .0
                .session
        } else {
            let receiver = f.start("W0", false).session;
            f.store
                .handoff(
                    f.project,
                    f.rev(),
                    started.session.id,
                    Some(receiver.id),
                    None,
                )
                .unwrap();
            receiver
        };
        f.end(successor.id);

        let candidate = f.move_candidate();
        let mut movement = f.movement();
        movement.expected_ownership_revision = scope.ownership_revision.unwrap();
        f.store
            .commit_source_projection_with_moves(
                f.rev(),
                &f.source,
                "moved",
                candidate,
                &[movement],
            )
            .unwrap();
    }
}

const DROP_V6: &str = "PRAGMA foreign_keys=OFF; DROP TABLE IF EXISTS eta_ingest_receipts; DROP TABLE IF EXISTS eta_acceptance_data; DROP TABLE IF EXISTS eta_sample_ledger; DROP TABLE IF EXISTS eta_forecasts; DROP TABLE IF EXISTS usage_ingest_receipts; DROP TABLE IF EXISTS usage_execution_intervals; DROP TABLE IF EXISTS usage_counter_snapshots; DROP TABLE IF EXISTS usage_allocation_records; DROP TABLE IF EXISTS usage_corrections; DROP TABLE IF EXISTS usage_receipts; DROP TABLE IF EXISTS adoption_credentials; DROP TABLE IF EXISTS delivery_credential_receipts; DROP TABLE IF EXISTS export_authorizations; DROP TABLE IF EXISTS hard_delivery_dependencies; DROP TABLE IF EXISTS team_handoff_receipts; DROP TABLE IF EXISTS team_handoffs; DROP TABLE IF EXISTS agent_authorization_receipts; DROP TABLE IF EXISTS agent_authorizations; DROP TABLE IF EXISTS operation_readset_receipts; DROP TABLE IF EXISTS responsibility_receipts; DROP TABLE IF EXISTS responsibility_events; DROP TABLE IF EXISTS task_collaborators; DROP TABLE IF EXISTS task_responsibilities; DROP TABLE IF EXISTS person_agent_bindings; DROP TABLE IF EXISTS persons; DROP TABLE source_content_reviews; DROP TABLE conversation_workstreams; DROP TABLE session_workstreams; DROP TRIGGER session_identity_no_update; DROP TRIGGER workstream_claim_exclusive_insert; DROP TRIGGER workstream_claim_exclusive_update; DELETE FROM schema_migrations WHERE version>=6; PRAGMA user_version=5;";

#[test]
fn migration_retains_ambiguous_workless_history_without_guessing_and_rolls_back() {
    let mut f = Fixture::new();
    let (workless, _) = f
        .store
        .start_session_in_workstream(
            f.project,
            f.rev(),
            Fixture::draft(None, false),
            None,
            f.scopes[0],
        )
        .unwrap();
    let work = f.start("W0", true);
    let cp = f.checkpoint(work.session.id);
    f.sql(DROP_V6);
    let path = f.root.join("state.db");
    f.sql("CREATE TRIGGER reject_scope_fixture BEFORE INSERT ON schema_migrations WHEN NEW.version=6 BEGIN SELECT RAISE(ABORT,'fixture rejection'); END;");
    assert!(Store::open(&path).is_err());
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='session_workstreams'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    f.sql("DROP TRIGGER reject_scope_fixture;");
    let preview = Store::preview_snapshot(&path, 32 * 1024 * 1024).unwrap();
    assert_eq!(
        preview
            .session_workstream(f.project, workless.session.id)
            .unwrap()
            .workstream_id,
        None
    );
    assert_eq!(
        preview
            .checkpoint_workstream(f.project, cp.id)
            .unwrap()
            .workstream_id,
        Some(f.scopes[0])
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    let migrated = Store::open(&path).unwrap();
    assert_eq!(
        json!(migrated.session(f.project, workless.session.id).unwrap()),
        json!(workless.session)
    );
    assert_eq!(
        migrated
            .session_workstream(f.project, workless.session.id)
            .unwrap()
            .workstream_id,
        None
    );
    assert_eq!(
        migrated
            .claim_workstream(f.project, work.claim.unwrap().id)
            .unwrap()
            .workstream_id,
        Some(f.scopes[0])
    );
}

#[test]
fn migration_refuses_simultaneous_legacy_claims_without_changing_either_owner() {
    let mut f = Fixture::new();
    let first = f.start("W0", true);
    f.sql(DROP_V6);
    let db = Connection::open(f.root.join("state.db")).unwrap();
    let branch = Id::new();
    let session = Id::new();
    db.execute("INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision) VALUES(?1,?2,'legacy-branch',0,'active',1)",params![branch.to_string(),f.project.to_string()]).unwrap();
    db.execute("INSERT INTO sessions SELECT ?1,project_id,work_item_id,?2,agent_id,provider,model,status,started_at,ended_at,start_project_revision,end_project_revision,last_checkpoint_id,revision FROM sessions WHERE id=?3",params![session.to_string(),branch.to_string(),first.session.id.to_string()]).unwrap();
    db.execute("INSERT INTO claims SELECT ?1,project_id,work_item_id,?2,agent_id,?3,status,acquired_at,expires_at,released_at,revision FROM claims WHERE id=?4",params![Id::new().to_string(),session.to_string(),branch.to_string(),first.claim.unwrap().id.to_string()]).unwrap();
    assert!(matches!(
        Store::open(&f.root.join("state.db")),
        Err(Error::ClaimConflict(_))
    ));
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM claims WHERE status='active'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}
