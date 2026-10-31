//! These tests run only in awr-runtime's test executable, never in the shipped CLI.
use super::*;
use crate::{CreateProposalRequest, ReviewProposalAction, create_proposal, review_proposal};
use awr_source::{Manifest, index_project};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const WORK: &str = "work_items:\n- id: W\n  title: Review the customer report\n  status: in_progress\n  next_action: Read the draft\n  acceptance: [Deliver the reviewed report]\n";
const NEXT: &str = "Read the revised report";
struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-recovery-fixture-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("fixture-owner"), "awr-recovery-test").unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(root.join("other.yaml"), WORK.replace("id: W", "id: X")).unwrap();
        fs::write(root.join(".awr/project.toml"), "[project]\nname='Recovery fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='ledger'\nrole='supporting'\npath='other.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let report =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(report.ok);
        Self {
            root,
            store,
            project: report.project_id,
        }
    }
    fn revision(&self) -> Revision {
        self.store.project(self.project).unwrap().project_revision
    }
    fn reopen(&mut self) {
        self.store = Store::open_existing(&self.root.join(".awr/state.db")).unwrap();
    }
    fn approve(&mut self, target: &str) -> Id {
        let revision = self.revision();
        let result = create_proposal(
            &mut self.store,
            &self.root,
            &CreateProposalRequest {
                kind: EntityKind::WorkItem,
                target: target.into(),
                intent: "Continue the report review".into(),
                changes: json!({"next_action":NEXT}),
                session_id: None,
                expected_revision: revision,
            },
        )
        .unwrap();
        let id = result.proposal.id;
        for action in [ReviewProposalAction::Submit, ReviewProposalAction::Approve] {
            let revision = self.revision();
            assert!(
                review_proposal(&mut self.store, &self.root, &request(id, revision, action))
                    .unwrap()
                    .ok
            );
        }
        id
    }
    fn action(&mut self, id: Id, action: ReviewProposalAction) -> Result<ProposalReport> {
        let revision = self.revision();
        review_proposal(&mut self.store, &self.root, &request(id, revision, action))
    }
    fn observed(
        &mut self,
        id: Id,
        recover: bool,
        hook: impl FnMut(&'static str) -> Result<()>,
    ) -> Result<ProposalReport> {
        let revision = self.revision();
        apply_observed(
            &mut self.store,
            &self.root,
            &request(
                id,
                revision,
                if recover {
                    ReviewProposalAction::Recover
                } else {
                    ReviewProposalAction::Apply
                },
            ),
            recover,
            None,
            hook,
        )
    }
    fn assert_done(&self, id: Id) {
        let proposal = self.store.proposal(self.project, id).unwrap();
        assert_eq!(proposal.status, ProposalStatus::Applied);
        let attempt = self
            .store
            .proposal_apply_attempt(self.project, id)
            .unwrap()
            .unwrap();
        let resolved = attempt.resolved_event_id.unwrap();
        let event = self.store.event(self.project, resolved).unwrap();
        assert_eq!(event.event_type, "proposal.applied");
        let source = self.store.source(self.project, proposal.source_id).unwrap();
        assert_eq!(source.freshness, Freshness::Fresh);
        assert_eq!(source.fingerprint, attempt.plan.after_fingerprint);
        let patch = proposal.bound_patch().unwrap();
        let (_, _, snapshot) = inspect_mutation_source(&self.root, &source, &patch).unwrap();
        assert_eq!(snapshot.fingerprint, attempt.plan.after_fingerprint);
        assert!(self.store.doctor().unwrap().ok);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn request(id: Id, revision: Revision, action: ReviewProposalAction) -> ReviewProposalRequest {
    ReviewProposalRequest {
        proposal_id: id,
        expected_revision: revision,
        action,
        actor: "reviewer".into(),
        reason: "Apply the reviewed report change".into(),
    }
}
fn temporary_io() -> Error {
    std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "temporary fixture I/O interruption",
    )
    .into()
}
fn temporary_access() -> Error {
    Error::SourceUnavailable("temporary fixture source access interruption".into())
}

#[test]
fn case_temporary_io_after_write_retains_recovery() {
    for temporary_error in [temporary_io as fn() -> Error, temporary_access] {
        let mut f = Fixture::new();
        let id = f.approve("W");
        let result = f
            .observed(id, false, |phase| {
                if phase == "after_rename" {
                    Err(temporary_error())
                } else {
                    Ok(())
                }
            })
            .unwrap();
        assert!(!result.ok);
        assert_eq!(result.source_write_performed, Some(true));
        assert_eq!(result.write_outcome, "pending_recovery");
        assert!(
            result
                .apply_attempt
                .as_ref()
                .unwrap()
                .resolved_event_id
                .is_none()
        );
        assert_eq!(
            fs::read_to_string(f.root.join("work.yaml")).unwrap(),
            fs::read_to_string(
                f.root
                    .join(result.recovery_directory.unwrap())
                    .join("after.yaml")
            )
            .unwrap()
        );
        f.reopen();
        let recovered = f.action(id, ReviewProposalAction::Recover).unwrap();
        assert!(recovered.ok);
        assert_eq!(recovered.source_write_performed, Some(false));
        f.assert_done(id);
    }
    println!("AWR_RECOVERY_CASE temporary_io_after_write");
}

#[test]
fn case_temporary_io_during_recovery_does_not_resolve_the_attempt() {
    for temporary_error in [temporary_io as fn() -> Error, temporary_access] {
        let mut f = Fixture::new();
        let id = f.approve("W");
        // A storage failure already has recovery semantics in the existing implementation.
        let initial = f
            .observed(id, false, |phase| {
                if phase == "after_rename" {
                    Err(Error::Storage("temporary fixture storage failure".into()))
                } else {
                    Ok(())
                }
            })
            .unwrap();
        assert_eq!(initial.write_outcome, "pending_recovery");
        let attempt = initial.apply_attempt.unwrap().event_id;
        f.reopen();
        let retry = f
            .observed(id, true, |phase| {
                if phase == "before_projection" {
                    Err(temporary_error())
                } else {
                    Ok(())
                }
            })
            .unwrap();
        assert_eq!(retry.write_outcome, "pending_recovery");
        assert!(!retry.ok);
        assert_eq!(retry.apply_attempt.as_ref().unwrap().event_id, attempt);
        assert!(retry.apply_attempt.unwrap().resolved_event_id.is_none());
        f.reopen();
        let result = f.action(id, ReviewProposalAction::Recover).unwrap();
        assert!(result.ok);
        f.assert_done(id);
    }
    println!("AWR_RECOVERY_CASE temporary_io_during_recovery");
}

struct Writer {
    child: Child,
    lines: mpsc::Receiver<String>,
}
impl Writer {
    fn start(f: &Fixture, specification: Value) -> Self {
        let input = f.root.join(format!("worker-{}.json", Id::new()));
        fs::write(&input, serde_json::to_vec(&specification).unwrap()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mutation_apply::recovery_tests::fixture_worker",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("AWR_RECOVERY_FIXTURE_INPUT", &input)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if send.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let mut writer = Self { child, lines };
        assert_eq!(
            writer.wait_for("AWR_RECOVERY_BOUNDARY "),
            specification["stop_at"].as_str().unwrap()
        );
        assert!(
            writer.child.try_wait().unwrap().is_none(),
            "boundary must belong to a live writer"
        );
        writer
    }
    fn wait_for(&mut self, prefix: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(remaining)
                .expect("writer did not reach the requested boundary");
            if let Some((_, tail)) = line.split_once(prefix) {
                return tail.to_owned();
            }
        }
    }
    fn resume(&mut self) {
        self.child.stdin.as_mut().unwrap().write_all(&[1]).unwrap();
    }
    fn kill_at_boundary(&mut self) {
        assert!(self.child.try_wait().unwrap().is_none());
        self.child.kill().unwrap();
        assert!(!self.child.wait().unwrap().success());
    }
    fn finish(&mut self) -> Value {
        let result = self.wait_for("AWR_RECOVERY_RESULT ");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        serde_json::from_str(&result).unwrap()
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
fn pause(phase: &str) -> Result<()> {
    println!("AWR_RECOVERY_BOUNDARY {phase}");
    std::io::stdout().flush()?;
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte)?;
    Ok(())
}

#[test]
#[ignore = "Child-process fixture, invoked explicitly by the recovery cases"]
fn fixture_worker() {
    let input = PathBuf::from(
        std::env::var_os("AWR_RECOVERY_FIXTURE_INPUT").expect("requires a fixture specification"),
    );
    let root = input.parent().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("fixture-owner")).unwrap(),
        "awr-recovery-test"
    );
    let config: Value = serde_json::from_slice(&fs::read(&input).unwrap()).unwrap();
    let phase = config["stop_at"].as_str().unwrap();
    if phase == "start" {
        pause(phase).unwrap();
    }
    let mut store = Store::open_existing(&root.join(".awr/state.db")).unwrap();
    let project = store.project_by_root(root).unwrap().id;
    let revision = config["expected_revision"].as_u64().unwrap();
    let result = if config["mode"] == "append" {
        store
            .append_event(
                project,
                revision,
                EventDraft::new("report.observed", config["summary"].as_str().unwrap()),
            )
            .map(|e| json!({"revision":e.project_revision,"event_id":e.id}))
    } else {
        let id = config["proposal_id"].as_str().unwrap().parse().unwrap();
        // Match the public boundary before observing the internal durability steps.
        let actual = store.project(project).unwrap().project_revision;
        if revision != actual {
            Err(Error::RevisionConflict {
                expected: revision,
                actual,
            })
        } else {
            apply_observed(&mut store,root,&request(id,revision,ReviewProposalAction::Apply),false,None,|reached|if reached==phase{pause(reached)}else{Ok(())}).map(|r|json!({"ok":r.ok,"code":r.code,"revision":r.project_revision,"proposal_status":r.proposal.status,"write_outcome":r.write_outcome}))
        }
    };
    let result = match result {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(error) => json!({"ok":false,"code":error.code()}),
    };
    println!("AWR_RECOVERY_RESULT {result}");
    std::io::stdout().flush().unwrap();
}

#[test]
fn case_process_death_at_each_durable_boundary_reopens_and_recovers_exactly() {
    for phase in [
        "before_journal",
        "after_journal",
        "after_temp",
        "after_rename",
        "after_sync",
        "after_projection",
        "after_finalization",
    ] {
        let mut f = Fixture::new();
        let id = f.approve("W");
        let revision = f.revision();
        let mut writer = Writer::start(
            &f,
            json!({"mode":"apply","proposal_id":id,"expected_revision":revision,"stop_at":phase}),
        );
        writer.kill_at_boundary();
        f.reopen();
        let before = fs::read(f.root.join("work.yaml")).unwrap();
        let before_write = matches!(phase, "before_journal" | "after_journal" | "after_temp");
        if before_write {
            assert_eq!(before, WORK.as_bytes());
        } else {
            assert_ne!(before, WORK.as_bytes());
        }
        let attempt = f.store.proposal_apply_attempt(f.project, id).unwrap();
        if phase == "before_journal" {
            assert!(attempt.is_none());
            assert!(f.action(id, ReviewProposalAction::Apply).unwrap().ok);
        } else if phase == "after_finalization" {
            let attempt = attempt.unwrap();
            assert!(attempt.resolved_event_id.is_some());
            f.assert_done(id);
            let revision = f.revision();
            assert!(matches!(
                f.action(id, ReviewProposalAction::Recover),
                Err(Error::InvalidTransition(_))
            ));
            assert_eq!(f.revision(), revision);
        } else {
            let attempt = attempt.unwrap();
            assert!(attempt.resolved_event_id.is_none());
            let modified = fs::metadata(f.root.join("work.yaml"))
                .unwrap()
                .modified()
                .unwrap();
            let report = f.action(id, ReviewProposalAction::Recover).unwrap();
            assert!(report.ok);
            assert_eq!(report.source_write_performed, Some(before_write));
            assert_eq!(report.apply_attempt.unwrap().event_id, attempt.event_id);
            if !before_write {
                assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), before);
                assert_eq!(
                    fs::metadata(f.root.join("work.yaml"))
                        .unwrap()
                        .modified()
                        .unwrap(),
                    modified
                );
            }
            if phase == "after_projection" {
                assert!(!report.source_refresh_performed);
            }
        }
        f.assert_done(id);
        let condition = if phase == "after_finalization" {
            "crash_after_finalize".to_owned()
        } else {
            format!("crash_{phase}")
        };
        println!("AWR_RECOVERY_CASE {condition}");
    }
}

#[test]
fn case_two_runtime_writers_compare_one_revision_inside_the_transaction() {
    let f = Fixture::new();
    let revision = f.revision();
    let mut first = Writer::start(
        &f,
        json!({"mode":"append","stop_at":"start","summary":"First writer reviewed the report","expected_revision":revision}),
    );
    let mut second = Writer::start(
        &f,
        json!({"mode":"append","stop_at":"start","summary":"Second writer reviewed the report","expected_revision":revision}),
    );
    first.resume();
    second.resume();
    let results = [first.finish(), second.finish()];
    assert_eq!(results.iter().filter(|r| r["ok"] == true).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| r["code"] == "RevisionConflict")
            .count(),
        1
    );
    assert_eq!(f.revision(), revision + 1);
    assert_eq!(
        f.store
            .events_since(f.project, revision, 100)
            .unwrap()
            .len(),
        1
    );
    println!("AWR_RECOVERY_CASE revision_cas_writers");
}

#[test]
fn case_two_source_writers_do_not_replace_each_others_results() {
    let mut f = Fixture::new();
    let first_id = f.approve("W");
    let second_id = f.approve("W");
    let mut first = Writer::start(
        &f,
        json!({"mode":"apply","proposal_id":first_id,"expected_revision":f.revision(),"stop_at":"after_temp"}),
    );
    // The second caller uses the now-current revision: it must still respect the source lock.
    let before = fs::read(f.root.join("work.yaml")).unwrap();
    let second = f.action(second_id, ReviewProposalAction::Apply);
    assert!(matches!(second, Err(Error::MutationConflict(_))));
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), before);
    first.resume();
    assert_eq!(first.finish()["value"]["ok"], true);
    f.reopen();
    f.assert_done(first_id);
    let after = fs::read(f.root.join("work.yaml")).unwrap();
    let second = f.action(second_id, ReviewProposalAction::Apply).unwrap();
    assert!(!second.ok);
    assert_eq!(second.code, "SourceConflict");
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), after);
    println!("AWR_RECOVERY_CASE same_source_apply_writers");
}

#[test]
fn case_independent_sources_preserve_both_updates_after_revision_retry() {
    let mut f = Fixture::new();
    let first_id = f.approve("W");
    let second_id = f.approve("X");
    let mut first = Writer::start(
        &f,
        json!({"mode":"apply","proposal_id":first_id,"expected_revision":f.revision(),"stop_at":"after_temp"}),
    );
    // A different source is not blocked by the first source's file lock.
    assert!(f.action(second_id, ReviewProposalAction::Apply).unwrap().ok);
    let second_bytes = fs::read(f.root.join("other.yaml")).unwrap();
    first.resume();
    let conflict = first.finish();
    assert_eq!(conflict["value"]["ok"], false);
    assert_eq!(conflict["value"]["write_outcome"], "pending_recovery");
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), WORK.as_bytes());
    f.reopen();
    let attempt = f
        .store
        .proposal_apply_attempt(f.project, first_id)
        .unwrap()
        .unwrap();
    assert!(attempt.resolved_event_id.is_none());
    let recovery = f.action(first_id, ReviewProposalAction::Recover).unwrap();
    assert!(recovery.ok);
    assert_eq!(recovery.source_write_performed, Some(true));
    assert_eq!(recovery.apply_attempt.unwrap().event_id, attempt.event_id);
    assert_eq!(fs::read(f.root.join("other.yaml")).unwrap(), second_bytes);
    f.assert_done(first_id);
    f.assert_done(second_id);
    println!("AWR_RECOVERY_CASE independent_sources_progress");
    println!("AWR_RECOVERY_CASE revision_before_replace");
}

#[test]
fn case_source_fingerprint_is_checked_at_entry_and_immediately_before_replace() {
    for after_prepare in [false, true] {
        let mut f = Fixture::new();
        let id = f.approve("W");
        let path = f.root.join("work.yaml");
        let newer = format!("{WORK}# Human added another review requirement\n");
        let result = if after_prepare {
            f.observed(id, false, |phase| {
                if phase == "after_temp" {
                    fs::write(&path, &newer)?;
                }
                Ok(())
            })
            .unwrap()
        } else {
            fs::write(&path, &newer).unwrap();
            f.action(id, ReviewProposalAction::Apply).unwrap()
        };
        assert!(!result.ok);
        assert_eq!(result.code, "SourceConflict");
        assert_eq!(result.proposal.status, ProposalStatus::Conflict);
        assert_eq!(result.source_write_performed, Some(false));
        assert_eq!(fs::read_to_string(&path).unwrap(), newer);
        if after_prepare {
            assert!(result.apply_attempt.unwrap().resolved_event_id.is_some());
        } else {
            assert!(result.apply_attempt.is_none());
        }
        println!(
            "AWR_RECOVERY_CASE {}",
            if after_prepare {
                "fingerprint_before_replace"
            } else {
                "fingerprint_before_apply"
            }
        );
    }
}

#[test]
fn case_mapping_change_cannot_redirect_a_prepared_write() {
    let mut f = Fixture::new();
    let id = f.approve("W");
    let manifest_path = f.root.join(".awr/project.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let alternate = f.root.join("alternate.yaml");
    fs::write(&alternate, WORK).unwrap();
    let report = f
        .observed(id, false, |phase| {
            if phase == "after_temp" {
                fs::write(
                    &manifest_path,
                    manifest.replace("path='work.yaml'", "path='alternate.yaml'"),
                )?;
            }
            Ok(())
        })
        .unwrap();
    assert!(!report.ok);
    assert_eq!(report.code, "SourceConflict");
    assert_eq!(report.source_write_performed, Some(false));
    assert_eq!(fs::read_to_string(f.root.join("work.yaml")).unwrap(), WORK);
    assert_eq!(fs::read_to_string(alternate).unwrap(), WORK);
    assert!(report.apply_attempt.unwrap().resolved_event_id.is_some());
    println!("AWR_RECOVERY_CASE mapping_change_during_apply");
}

#[test]
fn case_projection_and_receipt_transaction_failures_recover_once() {
    for event_type in [
        "proposal.apply_started",
        "source.projected",
        "proposal.applied",
    ] {
        let mut f = Fixture::new();
        let id = f.approve("W");
        let revision = f.revision();
        let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
        db.execute_batch(&format!("CREATE TRIGGER reject_recovery_event BEFORE INSERT ON events WHEN NEW.event_type='{event_type}' BEGIN SELECT RAISE(ABORT,'injected recovery event failure'); END;")).unwrap();
        let failed = f.action(id, ReviewProposalAction::Apply);
        if event_type == "proposal.apply_started" {
            assert!(matches!(failed, Err(Error::Storage(_))));
            assert_eq!(f.revision(), revision);
            assert!(
                f.store
                    .proposal_apply_attempt(f.project, id)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(fs::read_to_string(f.root.join("work.yaml")).unwrap(), WORK);
        } else {
            let failed = failed.unwrap();
            assert!(!failed.ok);
            assert_eq!(failed.write_outcome, "pending_recovery");
            assert_eq!(failed.source_write_performed, Some(true));
            assert_eq!(failed.proposal.status, ProposalStatus::Approved);
            assert!(failed.apply_attempt.unwrap().resolved_event_id.is_none());
            let events = f.store.events_since(f.project, revision, 100).unwrap();
            assert!(!events.iter().any(|e| e.event_type == event_type));
            assert_eq!(f.revision(), revision + events.len() as u64);
            let attempt = f
                .store
                .proposal_apply_attempt(f.project, id)
                .unwrap()
                .unwrap();
            let source = f.store.source(f.project, attempt.source_id).unwrap();
            if event_type == "source.projected" {
                // Invalidation is a separate committed event; the rejected projection
                // transaction must retain the old facts and fingerprint, explicitly stale.
                assert_eq!(source.freshness, Freshness::Stale);
                assert_eq!(source.fingerprint, attempt.plan.before_fingerprint);
            } else {
                assert_eq!(source.freshness, Freshness::Fresh);
                assert_eq!(source.fingerprint, attempt.plan.after_fingerprint);
            }
        }
        // Only remove the trigger installed above in this isolated fixture.
        db.execute_batch("DROP TRIGGER reject_recovery_event")
            .unwrap();
        f.reopen();
        let report = f
            .action(
                id,
                if event_type == "proposal.apply_started" {
                    ReviewProposalAction::Apply
                } else {
                    ReviewProposalAction::Recover
                },
            )
            .unwrap();
        assert!(report.ok);
        f.assert_done(id);
        let events = f.store.events_since(f.project, revision, 100).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "proposal.applied")
                .count(),
            1
        );
        let final_revision = f.revision();
        assert!(matches!(
            f.action(id, ReviewProposalAction::Recover),
            Err(Error::InvalidTransition(_))
        ));
        assert_eq!(f.revision(), final_revision);
    }
    println!("AWR_RECOVERY_CASE projection_or_receipt_storage_failure");
}

#[test]
fn case_recovery_keeps_a_newer_source_and_the_original_snapshots() {
    let mut f = Fixture::new();
    let id = f.approve("W");
    let mut writer = Writer::start(
        &f,
        json!({"mode":"apply","proposal_id":id,"expected_revision":f.revision(),"stop_at":"after_rename"}),
    );
    writer.kill_at_boundary();
    f.reopen();
    let attempt = f
        .store
        .proposal_apply_attempt(f.project, id)
        .unwrap()
        .unwrap();
    let directory = f.root.join(attempt.plan.recovery_directory());
    let snapshots = ["before.yaml", "after.yaml", "plan.json"]
        .map(|name| fs::read(directory.join(name)).unwrap());
    let path = f.root.join("work.yaml");
    let newer = format!(
        "{}# Human revised the report after the interruption\n",
        fs::read_to_string(&path).unwrap()
    );
    fs::write(&path, &newer).unwrap();
    let report = f.action(id, ReviewProposalAction::Recover).unwrap();
    assert!(!report.ok);
    assert_eq!(report.code, "SourceConflict");
    assert_eq!(report.source_write_performed, Some(false));
    assert_eq!(fs::read_to_string(&path).unwrap(), newer);
    assert_eq!(
        ["before.yaml", "after.yaml", "plan.json"]
            .map(|name| fs::read(directory.join(name)).unwrap()),
        snapshots
    );
    assert_ne!(report.proposal.status, ProposalStatus::Applied);
    println!("AWR_RECOVERY_CASE recovery_newer_source_preserved");
}

#[test]
fn case_recovery_rejects_either_corrupted_snapshot_before_installation() {
    for name in ["before.yaml", "after.yaml"] {
        let mut f = Fixture::new();
        let id = f.approve("W");
        let mut writer = Writer::start(
            &f,
            json!({"mode":"apply","proposal_id":id,"expected_revision":f.revision(),"stop_at":"after_temp"}),
        );
        writer.kill_at_boundary();
        f.reopen();
        let attempt = f
            .store
            .proposal_apply_attempt(f.project, id)
            .unwrap()
            .unwrap();
        let directory = f.root.join(attempt.plan.recovery_directory());
        fs::write(directory.join(name), b"A corrupted fixture snapshot").unwrap();
        let report = f.action(id, ReviewProposalAction::Recover).unwrap();
        assert!(!report.ok);
        assert_eq!(report.code, "SourceConflict");
        assert_eq!(report.source_write_performed, Some(false));
        assert_eq!(fs::read_to_string(f.root.join("work.yaml")).unwrap(), WORK);
        assert_eq!(report.proposal.status, ProposalStatus::Conflict);
        assert!(report.apply_attempt.unwrap().resolved_event_id.is_some());
    }
    println!("AWR_RECOVERY_CASE recovery_snapshot_corruption");
}

#[test]
#[ignore = "Requires AWR_RECOVERY_CLI from tests/recovery/mutations/verify.py"]
fn cli_recovery_after_process_death_binds_original_attempt_and_resolves_once() {
    let binary =
        PathBuf::from(std::env::var_os("AWR_RECOVERY_CLI").expect("build and supply the real CLI"));
    assert!(binary.is_absolute() && binary.is_file());
    for phase in ["after_temp", "after_rename", "after_projection"] {
        let mut f = Fixture::new();
        let id = f.approve("W");
        let mut writer = Writer::start(
            &f,
            json!({"mode":"apply","proposal_id":id,"expected_revision":f.revision(),"stop_at":phase}),
        );
        writer.kill_at_boundary();
        f.reopen();
        let attempt = f
            .store
            .proposal_apply_attempt(f.project, id)
            .unwrap()
            .unwrap();
        let bytes = fs::read(f.root.join("work.yaml")).unwrap();
        let invoke = |args: &[&str]| {
            let output = Command::new(&binary)
                .arg("--project")
                .arg(&f.root)
                .arg("--json")
                .args(args)
                .output()
                .unwrap();
            let payload = if output.stdout.is_empty() {
                &output.stderr
            } else {
                &output.stdout
            };
            let value: Value = serde_json::from_slice(payload).unwrap_or_else(|_| {
                panic!(
                    "CLI failed without JSON: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
            });
            (output.status.success(), value)
        };
        let (ok, doctor) = invoke(&["doctor"]);
        assert!(!ok);
        assert!(
            doctor["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["code"] == "incomplete_mutation")
        );
        let revision = f.revision();
        assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), bytes);
        let (ok, stale) = invoke(&[
            "proposal",
            "recover",
            &id.to_string(),
            "--actor",
            "reviewer",
            "--reason",
            "Resume the interrupted report review",
            "--expected-revision",
            "0",
        ]);
        assert!(!ok);
        assert_eq!(stale["code"], "RevisionConflict");
        assert_eq!(f.revision(), revision);
        let (ok, receipt) = invoke(&[
            "proposal",
            "recover",
            &id.to_string(),
            "--actor",
            "reviewer",
            "--reason",
            "Resume the interrupted report review",
            "--expected-revision",
            &revision.to_string(),
        ]);
        assert!(ok);
        assert_eq!(receipt["ok"], true);
        assert_eq!(receipt["write_outcome"], "recovered");
        assert_eq!(receipt["source_write_performed"], phase == "after_temp");
        assert_eq!(
            receipt["source_refresh_performed"],
            phase != "after_projection"
        );
        assert_eq!(
            receipt["apply_attempt"]["event_id"],
            json!(attempt.event_id)
        );
        assert_eq!(
            receipt["apply_attempt"]["resolved_event_id"],
            receipt["event"]["id"]
        );
        if phase != "after_temp" {
            assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), bytes);
        }
        let revision = f.revision();
        let (ok, repeated) = invoke(&[
            "proposal",
            "recover",
            &id.to_string(),
            "--actor",
            "reviewer",
            "--reason",
            "Check the delivered report receipt",
            "--expected-revision",
            &revision.to_string(),
        ]);
        assert!(!ok);
        assert_eq!(repeated["code"], "InvalidTransition");
        assert_eq!(f.revision(), revision);
        assert_eq!(
            f.store
                .events_since(f.project, attempt.project_revision, 100)
                .unwrap()
                .iter()
                .filter(|e| e.event_type == "proposal.applied")
                .count(),
            1
        );
        f.assert_done(id);
    }
    println!("AWR_RECOVERY_CASE recovery_once_and_cli_receipts");
}

#[test]
fn case_source_lock_release_does_not_wait_for_a_duplicated_descriptor() {
    let f = Fixture::new();
    let source = Id::new();
    let lock = source_lock(&f.root, source).unwrap();
    // Models the file-description reference inherited by a concurrently spawning
    // child before close-on-exec runs. It is not another authorized source writer.
    let duplicate = lock.0.try_clone().unwrap();
    drop(lock);
    let next = source_lock(&f.root, source);
    assert!(
        next.is_ok(),
        "the completed writer must release its reservation"
    );
    drop(next);
    drop(duplicate);
}
