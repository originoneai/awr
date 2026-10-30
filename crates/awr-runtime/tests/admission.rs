//! Execution registration and start, stale writes and an unusable policy against a real indexed
//! project. The CLI tests cover claims; these cover the other gated entries and the store's own
//! guards, which no CLI flag reaches.
use awr_core::*;
use awr_runtime::admit_work;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    fs,
    path::{Path, PathBuf},
};

const ACC: &str = "Saving failure keeps the user's input";
const OUTCOME: &str = "Users keep their input when saving fails";

struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn definition(pin: &str, criterion: &str) -> Value {
    json!({
        "version": 1, "purpose": "implementation", "tier": "required",
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": pin}],
        "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge", "by": "agent-a"},
        "responsibility": {"name": "storage-module", "kind": "delivery"},
        "deliverable": "Local save with a failure fallback",
        "evidence_method": "Targeted tests compared before and after the change",
        "failure_recovery": "A failed save keeps the previous data",
        "examples": [
            {"criterion": criterion, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
            {"criterion": criterion, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"}
        ],
        "discrimination": {"method": "fail_before_pass_after"}
    })
}

fn pin(root: &Path) -> String {
    format!(
        "sha256:{:x}",
        sha2::Sha256::digest(fs::read(root.join("decisions/scope.md")).unwrap())
    )
}

fn write_ledger(root: &Path, criterion: &str) {
    let ledger = json!({
        "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
        "work_items": [{"id": "W", "title": "Save safely", "status": "planned", "goal": "G",
            "acceptance": [criterion], "paths": ["crates/storage/src"], "next_action": "Do the work",
            "definition": definition(&pin(root), criterion)}]
    });
    fs::write(
        root.join("work.yaml"),
        serde_json::to_vec_pretty(&ledger).unwrap(),
    )
    .unwrap();
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-admission-runtime-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::create_dir_all(root.join("decisions")).unwrap();
        fs::write(
            root.join(".awr/project.toml"),
            "[project]\nname = \"Admission runtime fixture\"\nexternal_key = \"admission-runtime-fixture\"\n\n[[sources]]\ndomain = \"ledger\"\nrole = \"primary\"\npath = \"work.yaml\"\nadapter = \"yaml-ledger-v1\"\n\n[sources.options.admission_policy]\nversion = 1\nmode = \"governed_enforce\"\nscope = \"all\"\n\n[[sources]]\ndomain = \"decisions\"\nrole = \"supporting\"\npath = \"decisions\"\nadapter = \"markdown-directory-v1\"\n",
        )
        .unwrap();
        fs::write(
            root.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        write_ledger(&root, ACC);
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let indexed =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(indexed.ok, "{indexed:?}");
        Self {
            root,
            store,
            project: indexed.project_id,
        }
    }

    fn write_ledger(&self, criterion: &str) {
        write_ledger(&self.root, criterion);
    }

    fn reindex(&mut self) {
        let indexed = index_project(
            &mut self.store,
            &self.root,
            &Manifest::load(&self.root).unwrap(),
            false,
        )
        .unwrap();
        assert!(indexed.ok, "{indexed:?}");
    }

    fn revision(&self) -> u64 {
        self.store.project(self.project).unwrap().project_revision
    }

    fn session(&mut self, agent: &str, work: Option<&str>) -> Id {
        let revision = self.revision();
        let draft = SessionDraft {
            work_item_key: work.map(String::from),
            agent_id: agent.into(),
            provider: "test".into(),
            model: "m".into(),
            branch_id: None,
            claim: false,
            claim_ttl_ms: None,
        };
        self.store
            .start_session(self.project, revision, draft)
            .unwrap()
            .0
            .session
            .id
    }

    fn request(&self, session: Id, key: &str) -> AdmissionRequest {
        AdmissionRequest {
            work: "W".into(),
            session: Some(session),
            agent: None,
            expected_revision: self.revision(),
            request_key: key.into(),
            decision: AdmissionDecision::Approved,
            contract_fingerprint: self
                .store
                .work_contract(self.project, "W")
                .unwrap()
                .fingerprint,
            judgment: Some(AdmissionJudgment {
                contribution: "Advances the saving outcome".into(),
                necessity: "The draft is lost today".into(),
                alternatives: "Reuse of the existing buffer was checked".into(),
                deliverable: "A fallback path".into(),
                exit: "Stop when the fallback is verified".into(),
                responsibility: "storage-module".into(),
            }),
            proposal_event: None,
            reason: None,
        }
    }

    fn approve(&mut self, reviewer: Id, key: &str) -> Value {
        let request = self.request(reviewer, key);
        admit_work(&mut self.store, &self.root.clone(), &request).unwrap()
    }

    fn intent(&self, key: &str) -> ExecutionIntent {
        ExecutionIntent {
            operation_key: key.into(),
            purpose: "Run the storage checks".into(),
            executor: ExecutorKind::ManagedLocal,
            command: vec!["true".into()],
            cwd: self.root.to_string_lossy().into(),
            external_reference: None,
        }
    }

    fn worker() -> WorkerIdentity {
        WorkerIdentity {
            nonce: Id::new(),
            pid: 4242,
            port: 4343,
            child_pid: None,
        }
    }
}

fn code(error: Error) -> String {
    match error {
        Error::Admission { code, .. } => code.to_string(),
        other => panic!("expected an admission refusal, got {other:?}"),
    }
}

#[test]
fn executions_need_a_current_decision_to_register_and_again_to_start() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    // A session bound to the work but holding no claim still cannot register an execution.
    let revision = f.revision();
    let refused = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap_err();
    assert_eq!(code(refused), "admission_missing");

    f.approve(reviewer, "approve-1");
    let revision = f.revision();
    let (registered, _) = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap();
    assert_eq!(registered.state, ExecutionState::Registered);
    // A retry of the same request registers nothing twice: one operation key, one execution.
    let revision = f.revision();
    let again = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap_err();
    assert!(matches!(again, Error::SourceConflict(_)), "{again:?}");
    assert_eq!(f.store.executions(f.project, None).unwrap().len(), 1);

    // The scope moves between registration and start: the start is refused and nothing is dispatched.
    f.write_ledger("Saving failure keeps all of the user's input");
    f.reindex();
    let revision = f.revision();
    let refused = f
        .store
        .start_execution(f.project, revision, registered.id, Fixture::worker())
        .unwrap_err();
    assert_eq!(code(refused), "admission_stale");
    let still = f.store.execution(f.project, registered.id).unwrap();
    assert_eq!(
        still.state,
        ExecutionState::Registered,
        "a refused start dispatches nothing"
    );

    // A new decision on the current scope lets the same execution start once, and only once.
    f.approve(reviewer, "approve-2");
    let revision = f.revision();
    let (started, _) = f
        .store
        .start_execution(f.project, revision, registered.id, Fixture::worker())
        .unwrap();
    assert_eq!(started.state, ExecutionState::Starting);
    let revision = f.revision();
    assert!(
        f.store
            .start_execution(f.project, revision, registered.id, Fixture::worker())
            .is_err(),
        "no second dispatch"
    );
}

#[test]
fn a_decision_computed_against_an_older_revision_is_rejected_as_a_stale_write() {
    let mut f = Fixture::new();
    let reviewer = f.session("reviewer", None);
    let request = f.request(reviewer, "late");
    // The source moves after the decider read the contract: the revision guard refuses the write.
    f.write_ledger("Saving failure keeps all of the user's input");
    f.reindex();
    let root = f.root.clone();
    let error = admit_work(&mut f.store, &root, &request).unwrap_err();
    assert!(
        matches!(
            error,
            Error::RevisionConflict { .. } | Error::SourceConflict(_)
        ),
        "{error:?}"
    );
    // Nothing was recorded, so the work stays unadmitted.
    let status = f.store.admission_status(f.project, "W", None).unwrap();
    assert_eq!(status.state, "missing");
}

#[test]
fn an_unusable_stored_policy_fails_closed_for_claims_and_executions() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "approve-1");
    // The manifest validates policies, so corrupt the stored source configuration directly.
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    let corrupted = db
        .execute(
            "UPDATE sources SET config_json=json_set(config_json,'$.adapter_options.admission_policy.version',9) WHERE domain='ledger'",
            [],
        )
        .unwrap();
    assert_eq!(corrupted, 1);
    drop(db);
    let status = f.store.admission_status(f.project, "W", None).unwrap();
    assert_eq!((status.state, status.enforced), ("policy_invalid", true));
    let revision = f.revision();
    let refused = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap_err();
    assert_eq!(code(refused), "admission_policy_invalid");
}

#[test]
fn a_state_that_cannot_be_established_is_unknown_and_never_an_error_or_a_pass() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "approve-1");
    // Events are append-only, so damaging one needs the guard removed first: this stands for a
    // corrupted store, the one way a recorded decision can fail to read back.
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    db.execute("DROP TRIGGER event_no_update", []).unwrap();
    let damaged = db
        .execute(
            "UPDATE events SET payload_json='{\"version\":1}' WHERE event_type='work.admission'",
            [],
        )
        .unwrap();
    assert_eq!(damaged, 1);
    drop(db);
    let revision = f.revision();
    let refused = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap_err();
    assert_eq!(code(refused), "admission_unknown");
    // Readiness reports the reason instead of failing, so every listing keeps answering.
    let readiness = f.store.work_readiness(f.project, "W", None, 0).unwrap();
    assert!(
        readiness
            .diagnostics
            .iter()
            .any(|d| d.code == "admission_unknown"),
        "{:?}",
        readiness.diagnostics
    );
    assert!(!readiness.ready);
}
