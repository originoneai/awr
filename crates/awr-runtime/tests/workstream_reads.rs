#[allow(dead_code)]
#[path = "../../awr-store/tests/support/workstreams.rs"]
mod fixture;
use awr_core::*;
use awr_runtime::{PrepareWorkRequest, ResumeRequest, Runtime, prepare_work, resume_session};
use awr_source::{Manifest, index_project};
use awr_store::{Store, WorkstreamReadSelection};
use fixture::Fixture;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn scoped_project() -> (PathBuf, Store, Id) {
    let root = std::env::temp_dir().join(format!("awr-scoped-runtime-{}", Id::new()));
    fs::create_dir_all(root.join(".awr")).unwrap();
    fs::write(
        root.join("work.yaml"),
        include_str!("../../../tests/fixtures/workstreams/context.yaml"),
    )
    .unwrap();
    fs::write(
        root.join("rules.md"),
        "# Shared {#shared severity=hard scope=project value=*}\n\nPreserve approved contracts.\n",
    )
    .unwrap();
    fs::write(
        root.join(".awr/project.toml"),
        include_str!("../../../tests/fixtures/workstreams/context.toml"),
    )
    .unwrap();
    let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
    let indexed = index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
    assert!(indexed.ok, "{indexed:?}");
    (root, store, indexed.project_id)
}

#[test]
fn scoped_file_read_checks_visibility_before_io_and_keeps_digest_guards() {
    let mut f = Fixture::new();
    let event = f.event(1, None);
    let body = b"Synthetic private report";
    let artifact = f
        .store
        .record_artifact(
            f.project,
            f.rev(),
            ArtifactDraft {
                artifact_type: "report".into(),
                locator: "private.txt".into(),
                sha256: format!("{:x}", Sha256::digest(body)),
                size: body.len() as u64,
                mime: "text/plain".into(),
                source_event_id: event.id,
            },
        )
        .unwrap()
        .0;
    let a = f.access(&[0]);
    let b = f.access(&[1]);
    let selector = WorkstreamReadSelection::default();
    // The path does not exist: denied scope must fail without observing that.
    let runtime = Runtime::attach(&mut f.store, f.project).unwrap();
    assert!(matches!(
        runtime.read_artifact_in_workstream(&a, &selector, artifact.id, 100),
        Err(Error::Workstream(WorkstreamError::AccessDenied))
    ));
    assert!(matches!(
        runtime.read_artifact_in_workstream(&b, &selector, artifact.id, 100),
        Err(Error::Io(_))
    ));
    std::fs::write(f.root.join("private.txt"), body).unwrap();
    let runtime = Runtime::attach(&mut f.store, f.project).unwrap();
    let (record, bytes) = runtime
        .read_artifact_in_workstream(&b, &selector, artifact.id, 100)
        .unwrap();
    assert_eq!(record.id, artifact.id);
    assert_eq!(bytes, body);
    assert!(matches!(
        runtime.read_artifact_in_workstream(&b, &selector, artifact.id, 2),
        Err(Error::InvalidInput(_))
    ));
    std::fs::write(f.root.join("private.txt"), vec![b'X'; body.len()]).unwrap();
    assert!(matches!(
        runtime.read_artifact_in_workstream(&b, &selector, artifact.id, 100),
        Err(Error::SourceConflict(_))
    ));
}

#[test]
fn scoped_evidence_read_uses_same_boundary_without_fabricating_verification() {
    let mut f = Fixture::new();
    let evidence = f.evidence(1);
    let a = f.access(&[0]);
    let b = f.access(&[1]);
    let selector = WorkstreamReadSelection::default();
    let runtime = Runtime::attach(&mut f.store, f.project).unwrap();
    assert!(matches!(
        runtime.read_evidence_report_in_workstream(&a, &selector, &evidence.external_key, 100),
        Err(Error::Workstream(WorkstreamError::AccessDenied))
    ));
    std::fs::write(f.root.join(&evidence.locator), b"Synthetic report").unwrap();
    let runtime = Runtime::attach(&mut f.store, f.project).unwrap();
    let (record, bytes) = runtime
        .read_evidence_report_in_workstream(&b, &selector, &evidence.external_key, 100)
        .unwrap();
    assert_eq!(bytes, b"Synthetic report");
    assert_eq!(record.item.level, EvidenceLevel::Designed);
    assert!(record.item.sha256.is_none());
}

#[test]
fn prepare_uses_the_scoped_context_branch_for_readiness_and_management() {
    let (root, mut store, project) = scoped_project();
    let branch = store
        .create_branch(
            project,
            store.project(project).unwrap().project_revision,
            BranchDraft {
                name: "client-branch".into(),
                parent_branch_id: None,
                git_binding: None,
                actor: "fixture".into(),
                reason: "Exercise another workstream branch".into(),
            },
        )
        .unwrap()
        .0;
    store
        .start_session(
            project,
            store.project(project).unwrap().project_revision,
            SessionDraft {
                work_item_key: Some("CLIENT-1".into()),
                agent_id: "client-agent".into(),
                provider: "fixture".into(),
                model: "fixture".into(),
                branch_id: Some(branch.id),
                claim: false,
                claim_ttl_ms: None,
            },
        )
        .unwrap();
    store
        .switch_branch(
            project,
            store.project(project).unwrap().project_revision,
            Some(branch.id),
            "fixture",
            "Select the client branch globally",
        )
        .unwrap();

    let prepared = prepare_work(
        &mut store,
        &root,
        &PrepareWorkRequest {
            work: "API-1".into(),
            session: None,
            branch: None,
            goals: vec![],
            source_sha: None,
            budget: Some(8000),
        },
    )
    .unwrap();
    assert!(prepared["context"]["completeness"]["branch_id"].is_null());
    assert!(prepared["management"]["branch_id"].is_null());
    assert_eq!(
        store.project(project).unwrap().current_branch_id,
        Some(branch.id)
    );

    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resume_preflight_uses_the_source_sessions_scoped_branch() {
    let (root, mut store, project) = scoped_project();
    let branch = store
        .create_branch(
            project,
            store.project(project).unwrap().project_revision,
            BranchDraft {
                name: "api-branch".into(),
                parent_branch_id: None,
                git_binding: None,
                actor: "fixture".into(),
                reason: "Exercise scoped recovery".into(),
            },
        )
        .unwrap()
        .0;
    let from = store
        .start_session(
            project,
            store.project(project).unwrap().project_revision,
            SessionDraft {
                work_item_key: Some("API-1".into()),
                agent_id: "first-api-agent".into(),
                provider: "fixture".into(),
                model: "fixture".into(),
                branch_id: Some(branch.id),
                claim: false,
                claim_ttl_ms: None,
            },
        )
        .unwrap()
        .0
        .session;
    store
        .switch_branch(
            project,
            store.project(project).unwrap().project_revision,
            Some(branch.id),
            "fixture",
            "Select the recovery branch",
        )
        .unwrap();
    let expected_revision = store.project(project).unwrap().project_revision;
    let report = resume_session(
        &mut store,
        &root,
        &ResumeRequest {
            from_session_id: Some(from.id),
            work_item_key: Some("API-1".into()),
            agent_id: "second-api-agent".into(),
            provider: "fixture".into(),
            model: "fixture".into(),
            claim: ResumeClaim::None,
            claim_ttl_ms: None,
            expected_revision,
            token_budget: 8000,
            paths: None,
            tags: None,
            goal_keys: vec![],
            source_sha: None,
        },
    )
    .unwrap();
    assert!(report.context_error.is_none(), "{:?}", report.context_error);
    assert!(report.resumed.is_some());
    assert_eq!(
        report.context.unwrap().completeness.branch_id,
        Some(branch.id)
    );

    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn evidence_report_read_rejects_every_byte_change_with_actionable_guidance() {
    let mut f = Fixture::new();
    let body: &[u8] = b"Synthetic report";
    let locator = "report-newline.txt";
    let evidence = f
        .store
        .record_evidence(
            f.project,
            f.rev(),
            EvidenceDraft {
                work_item_key: Some("W1".into()),
                external_key: "E-NL".into(),
                evidence_type: "report".into(),
                level: EvidenceLevel::Designed,
                summary: "Byte-exact evidence".into(),
                locator: locator.into(),
                sha256: Some(format!("{:x}", Sha256::digest(body))),
                source_sha: None,
                command: None,
                scope: vec!["W1".into()],
                branch_id: None,
                verified_at: None,
            },
        )
        .unwrap()
        .0;
    let denied = f.access(&[0]);
    let allowed = f.access(&[1]);
    let selector = WorkstreamReadSelection::default();
    let runtime = Runtime::attach(&mut f.store, f.project).unwrap();
    assert!(matches!(
        runtime.read_evidence_report_in_workstream(&denied, &selector, &evidence.external_key, 100),
        Err(Error::Workstream(WorkstreamError::AccessDenied))
    ));
    // The binding is byte-exact: every change after registration, including
    // a single appended LF, CRLF or a bare CR, is rejected.
    for changed in [
        &b"Synthetic report\n"[..],
        &b"Synthetic report\r\n"[..],
        &b"Synthetic report\r"[..],
        &b"Synthetic report\n\n"[..],
        &b"Synthetic report\nX"[..],
    ] {
        fs::write(f.root.join(locator), changed).unwrap();
        let runtime = Runtime::attach(&mut f.store, f.project).unwrap();
        match runtime.read_evidence_report_in_workstream(
            &allowed,
            &selector,
            &evidence.external_key,
            100,
        ) {
            Err(Error::SourceConflict(message)) => {
                assert!(message.contains("expected"), "{message}");
                assert!(message.contains("actual"), "{message}");
                assert!(message.contains("byte-identical"), "{message}");
            }
            other => panic!("expected a source conflict for {changed:?}, got {other:?}"),
        }
    }
    // Registering the digest of the current bytes restores the binding.
    let current = fs::read(f.root.join(locator)).unwrap();
    f.store
        .record_evidence(
            f.project,
            f.rev(),
            EvidenceDraft {
                work_item_key: Some("W1".into()),
                external_key: "E-NL2".into(),
                evidence_type: "report".into(),
                level: EvidenceLevel::Designed,
                summary: "Byte-exact evidence".into(),
                locator: locator.into(),
                sha256: Some(format!("{:x}", Sha256::digest(&current))),
                source_sha: None,
                command: None,
                scope: vec!["W1".into()],
                branch_id: None,
                verified_at: None,
            },
        )
        .unwrap();
    let runtime = Runtime::attach(&mut f.store, f.project).unwrap();
    let (_, bytes) = runtime
        .read_evidence_report_in_workstream(&allowed, &selector, "E-NL2", 100)
        .unwrap();
    assert_eq!(bytes, current);
}
