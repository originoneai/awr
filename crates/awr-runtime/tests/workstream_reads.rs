#[allow(dead_code)]
#[path = "../../awr-store/tests/support/workstreams.rs"]
mod fixture;
use awr_core::*;
use awr_runtime::{
    MainlineNavAudience, MainlineNavExtras, MainlineNavScope, PrepareWorkRequest, ResumeRequest,
    Runtime, mainline_nav, mainline_nav_for, prepare_work, resume_session,
};
use awr_source::{Manifest, index_project};
use awr_store::{Store, WorkstreamReadSelection};
use fixture::Fixture;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const API: &str = "01K00000000000000000000001";
const CLIENT: &str = "01K00000000000000000000002";

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

fn reader(streams: &[&str]) -> MainlineNavAudience {
    MainlineNavAudience::Workstreams(streams.iter().map(|id| id.parse().unwrap()).collect())
}

fn nav_nodes(nav: &Value) -> Vec<&str> {
    let mut keys = nav["mainline_graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["work_key"].as_str().unwrap())
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

#[test]
fn mainline_nav_narrows_a_reader_before_assembly_and_leaves_the_owner_view_whole() {
    let (root, store, project_id) = scoped_project();
    let project = store.project(project_id).unwrap();
    let scope = MainlineNavScope::default();
    let nav = |scope: &MainlineNavScope, audience: &MainlineNavAudience| {
        mainline_nav_for(
            &store,
            &project,
            scope,
            MainlineNavExtras::default(),
            audience,
        )
    };

    // The owner (the CLI's entry point) sees every workstream and the cross-stream edge.
    let owner = mainline_nav(&store, &project, &scope, MainlineNavExtras::default()).unwrap();
    assert_eq!(nav_nodes(&owner), ["API-1", "CLIENT-1"]);
    assert_eq!(owner["cross_dependencies"].as_array().unwrap().len(), 1);
    assert!(owner.to_string().contains("PRIVATE_CLIENT_TASK"));
    assert_eq!(owner, nav(&scope, &MainlineNavAudience::Owner).unwrap());
    // A reader of every stream has nothing to hide: it is the owner's view, byte for byte.
    assert_eq!(owner, nav(&scope, &reader(&[API, CLIENT])).unwrap());

    // CLIENT-1 depends on API-1. The api-only reader loses the consumer and the edge...
    let api = nav(&scope, &reader(&[API])).unwrap();
    assert_eq!(nav_nodes(&api), ["API-1"]);
    assert_eq!(api["scope"]["workstream"]["external_key"], "api");
    assert_eq!(api["cross_dependencies"], json!([]));
    for hidden in ["CLIENT-1", "PRIVATE_CLIENT", CLIENT] {
        assert!(!api.to_string().contains(hidden), "{hidden}: {api}");
    }
    // ...and the client-only reader keeps an opaque wait for the producer it cannot read.
    let client = nav(&scope, &reader(&[CLIENT])).unwrap();
    assert_eq!(nav_nodes(&client), ["CLIENT-1"]);
    let waits = client["mainline_graph"]["nodes"][0]["explainable_waits"]
        .as_array()
        .unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0]["kind"], "dependency_outside_scope");
    for hidden in ["API-1", "Define the interface", "Verify the interface", API] {
        assert!(!client.to_string().contains(hidden), "{hidden}: {client}");
    }

    // Explicit work outside the grants is a denial, never a partial or empty answer.
    for work in ["CLIENT-1", "absent"] {
        let scope = MainlineNavScope {
            work: vec![work.into()],
            ..Default::default()
        };
        assert!(matches!(
            nav(&scope, &reader(&[API])),
            Err(Error::Workstream(WorkstreamError::AccessDenied))
        ));
    }
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_unreadable_goal_key_answers_like_an_unknown_goal() {
    let (root, store, project_id) = scoped_project();
    let project = store.project(project_id).unwrap();
    let nav = |goal: &str, audience: &MainlineNavAudience| {
        mainline_nav_for(
            &store,
            &project,
            &MainlineNavScope {
                goal: Some(goal.into()),
                ..Default::default()
            },
            MainlineNavExtras::default(),
            audience,
        )
    };
    // `client-delivery` is the goal of the client workstream, which this reader cannot read.
    let unreadable = nav("client-delivery", &reader(&[API])).unwrap_err();
    let unknown = nav("no-such-goal", &reader(&[API])).unwrap_err();
    assert!(matches!(unreadable, Error::NotFound(_)), "{unreadable:?}");
    assert!(matches!(unknown, Error::NotFound(_)), "{unknown:?}");
    // Apart from the key the caller supplied, the two answers are identical.
    assert_eq!(
        unreadable.to_string().replace("client-delivery", "KEY"),
        unknown.to_string().replace("no-such-goal", "KEY")
    );
    // The reader's own goal still resolves, and the owner still knows the project-wide goal.
    assert!(nav("api-delivery", &reader(&[API])).is_ok());
    assert!(nav("client-delivery", &MainlineNavAudience::Owner).is_ok());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
