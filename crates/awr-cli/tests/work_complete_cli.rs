use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const C1: &str = "The report contains the requested analysis";
const C2: &str = "The report is reviewed and delivered";
const WORK: &str = "work_items:\n- id: W\n  title: Deliver the report\n  status: in_progress\n  owner: business-coordinator\n  depends_on: [D]\n  evidence: [old-report.json]\n  verification:\n    evidence_level: none\n    reviewed_by: coordinator\n  acceptance: [The report contains the requested analysis, The report is reviewed and delivered]\n- id: OTHER\n  status: ready\n";
struct Fixture {
    root: PathBuf,
    session: String,
    claim: String,
}
impl Fixture {
    fn new() -> Self {
        let mut f = Self {
            root: std::env::temp_dir().join(format!("awr-work-complete-{}", Id::new())),
            session: String::new(),
            claim: String::new(),
        };
        fs::create_dir(&f.root).unwrap();
        fs::write(f.root.join("work.yaml"), WORK).unwrap();
        fs::write(
            f.root.join("deps.yaml"),
            "work_items:\n- id: D\n  status: completed\n",
        )
        .unwrap();
        fs::write(f.root.join("sources.toml"),"[project]\nname='Completion'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='ledger'\nrole='supporting'\npath='deps.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        let s = f.ok(&[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "worker-a",
            "--provider",
            "fixture",
            "--model",
            "local",
            "--claim",
            "--expected-revision",
            &f.revision(),
        ]);
        f.session = s["session"]["id"].as_str().unwrap().into();
        f.claim = s["claim"]["id"].as_str().unwrap().into();
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        Self::success(&r)
    }
    fn success(r: &Output) -> Value {
        assert!(
            r.status.success(),
            "{} {}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn error(r: &Output, code: &str) {
        assert!(
            !r.status.success(),
            "{}",
            String::from_utf8_lossy(&r.stdout)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stderr).unwrap()["code"],
            code,
            "{} {}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
    }
    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }
    fn store(&self) -> (Store, Id) {
        let s = Store::open_existing(&self.root.join(".awr/state.db")).unwrap();
        let p = s
            .project_by_root(&self.root.canonicalize().unwrap())
            .unwrap()
            .id;
        (s, p)
    }
    fn work(&self) -> Value {
        self.ok(&["object", "show", "work", "W", "--cached", "--full"])["object"].clone()
    }
    fn report(&self) -> Value {
        json!({"version":1,"work_item":"W","source_sha":SHA,"command":"verify report and delivery","scope":["W"],"verified_at":now_millis().unwrap(),"checks":[{"name":"content and delivery","passed":true,"details":"Checked the analysis and delivery receipt","criteria":[C1,C2]}]})
    }
    fn organize(&self) {
        let body = fs::read_to_string(self.root.join("work.yaml")).unwrap();
        let body = body.replace(
            "  title: Deliver the report\n",
            "  title: Deliver the report\n  goal: G\n",
        );
        fs::write(self.root.join("work.yaml"), format!("{body}\ngoals:\n- id: G\n  title: Deliver the requested analysis\n  status: active\n  summary: The user requested an analysis and its delivery\n  success_criteria: [The requested analysis is delivered]\n")).unwrap();
        let path = self.root.join(".awr/project.toml");
        let manifest = fs::read_to_string(&path).unwrap();
        let manifest = if manifest.contains("context_profile = \"standard\"") {
            manifest.replace(
                "context_profile = \"standard\"",
                "context_profile = \"minimal\"",
            )
        } else if manifest.contains("context_profile = \"minimal\"") {
            manifest
        } else {
            manifest.replace("[project]\n", "[project]\ncontext_profile = \"minimal\"\n")
        };
        fs::write(path, manifest).unwrap();
        self.ok(&["source", "reindex"]);
    }
    fn verified_completed(&self) -> u64 {
        self.ok(&["intake", "inspect", "--source-sha", SHA])["organization"]["verified_completed"]
            .as_u64()
            .unwrap()
    }
    fn input(&self) -> Value {
        json!({"version":1,"source_sha":SHA,"acceptance":[{"criterion":C1,"evidence":["E"]},{"criterion":C2,"evidence":["E"]}]})
    }
    fn evidence(
        &self,
        key: &str,
        report: &Value,
        change: impl FnOnce(&mut EvidenceDraft),
    ) -> Evidence {
        let bytes = serde_json::to_vec_pretty(report).unwrap();
        let locator = format!("report-{}.json", Id::new());
        fs::write(self.root.join(&locator), &bytes).unwrap();
        let mut draft = EvidenceDraft {
            external_key: key.into(),
            work_item_key: Some("W".into()),
            evidence_type: "completion_report".into(),
            level: EvidenceLevel::LocallyVerified,
            summary: "Checked report".into(),
            locator,
            sha256: Some(
                awr_source::fingerprint(&bytes)
                    .trim_start_matches("sha256:")
                    .into(),
            ),
            source_sha: Some(SHA.into()),
            command: Some("verify report and delivery".into()),
            scope: vec!["W".into()],
            branch_id: None,
            verified_at: report["verified_at"].as_i64(),
        };
        change(&mut draft);
        let (mut s, p) = self.store();
        let r = s.project(p).unwrap().project_revision;
        s.record_evidence(p, r, draft).unwrap().0
    }
    fn complete(&self, input: &Value) -> Output {
        let path = self.root.join("completion.json");
        fs::write(&path, serde_json::to_vec(input).unwrap()).unwrap();
        self.run(&[
            "work",
            "complete",
            "W",
            "--session",
            &self.session,
            "--reason",
            "Reviewed the evidence for every acceptance criterion",
            "--input",
            path.to_str().unwrap(),
            "--expected-revision",
            &self.revision(),
        ])
    }
    fn proposal_action(&self, action: &str, id: Id) -> Output {
        self.run(&[
            "proposal",
            action,
            &id.to_string(),
            "--actor",
            "worker-a",
            "--reason",
            "Review the bound completion",
            "--expected-revision",
            &self.revision(),
        ])
    }
    fn claim_status(&self) -> Value {
        self.ok(&["session", "show", &self.session])["claims"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == self.claim)
            .unwrap()["status"]
            .clone()
    }
    fn draft(&self, evidence: Evidence, changes: Option<Value>) -> MutationDraft {
        let (s, p) = self.store();
        let target = s.mutation_target(p, EntityKind::WorkItem, "W").unwrap();
        let work: WorkItem = serde_json::from_value(target.item).unwrap();
        let binding = CompletionBinding {
            version: 1,
            source_sha: SHA.into(),
            minimum_level: EvidenceLevel::LocallyVerified,
            acceptance: vec![
                AcceptanceEvidenceBinding {
                    criterion: C1.into(),
                    evidence: vec![evidence.id],
                },
                AcceptanceEvidenceBinding {
                    criterion: C2.into(),
                    evidence: vec![evidence.id],
                },
            ],
            evidence: vec![evidence],
        };
        let mut patch = MutationPatch {
            version: 1,
            target: MutationTarget {
                kind: EntityKind::WorkItem,
                meta: work.meta,
            },
            source_config: target.source.config.clone(),
            intent: "Complete the reviewed report".into(),
            changes: json!({"status":"completed"}),
            host_edit: None,
            work_action: None,
        };
        let record =
            awr_source::read_yaml_mutation_record(&self.root, &target.source, &patch).unwrap();
        patch.changes =
            changes.unwrap_or_else(|| completion_source_changes(&record, &binding).unwrap());
        patch.work_action = Some(WorkActionBinding {
            action: WorkAction::Complete,
            from: work.status,
            to: WorkStatus::Completed,
            completion: Some(binding),
        });
        MutationDraft {
            source_id: target.source.id,
            base_fingerprint: target.source.fingerprint,
            mutation_type: "work.complete".into(),
            patch,
            created_by_session: Some(self.session.parse().unwrap()),
        }
    }
    fn approved(&self, draft: MutationDraft) -> Id {
        let (mut s, p) = self.store();
        let r = s.project(p).unwrap().project_revision;
        let (proposal, e) = s.create_proposal(p, r, draft).unwrap();
        let (_, e) = s
            .review_proposal(
                p,
                e.project_revision,
                proposal.id,
                ProposalAction::Submit,
                "worker-a",
                "Review proof",
            )
            .unwrap();
        s.review_proposal(
            p,
            e.project_revision,
            proposal.id,
            ProposalAction::Approve,
            "worker-a",
            "Approve proof",
        )
        .unwrap();
        proposal.id
    }
}

#[test]
fn shared_ledger_completion_preserves_stamped_evidence_and_selected_reports() {
    let f = Fixture::new();
    f.organize();
    let body = fs::read_to_string(f.root.join("work.yaml")).unwrap();
    fs::write(
        f.root.join("work.yaml"),
        body.replace("- id: OTHER\n  status: ready\n", &format!(
            "- id: OTHER\n  title: Deliver another report\n  goal: G\n  status: in_progress\n  next_action: Verify the other report\n  acceptance: [{C1}, {C2}]\n"
        )),
    ).unwrap();
    f.ok(&["source", "reindex"]);
    let other_session = f.ok(&[
        "session",
        "start",
        "--work",
        "OTHER",
        "--agent",
        "worker-b",
        "--provider",
        "fixture",
        "--model",
        "local",
        "--claim",
        "--expected-revision",
        &f.revision(),
    ])["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ledger_sha = || {
        awr_source::fingerprint(&fs::read(f.root.join("work.yaml")).unwrap())
            .trim_start_matches("sha256:")
            .to_owned()
    };
    let original_sha = ledger_sha();
    let mut report = f.report();
    report["source_sha"] = json!(original_sha);
    let selected = f.evidence("E", &report, |d| d.source_sha = Some(original_sha.clone()));
    let required = f.evidence("R", &report, |d| d.source_sha = Some(original_sha.clone()));
    report["work_item"] = json!("OTHER");
    report["scope"] = json!(["OTHER"]);
    let other = f.evidence("other-report", &report, |d| {
        d.work_item_key = Some("OTHER".into());
        d.scope = vec!["OTHER".into()];
        d.source_sha = Some(original_sha.clone());
    });
    for record in [&selected, &required, &other] {
        assert!(record.section_fingerprint.is_some());
    }
    let mut input = f.input();
    input["source_sha"] = json!(original_sha);
    input["required_evidence"] = json!(["R"]);
    Fixture::success(&f.complete(&input));
    assert_ne!(ledger_sha(), original_sha);
    // The second worker uses its original report, recorded before W completed.
    // No report bytes or verification revision are rewritten for the new ledger.
    input["required_evidence"] = json!([]);
    for mapping in input["acceptance"].as_array_mut().unwrap() {
        mapping["evidence"] = json!(["other-report"]);
    }
    let other_input = f.root.join("other-completion.json");
    fs::write(&other_input, serde_json::to_vec(&input).unwrap()).unwrap();
    f.ok(&[
        "work",
        "complete",
        "OTHER",
        "--session",
        &other_session,
        "--reason",
        "Verified the independently assigned report",
        "--input",
        other_input.to_str().unwrap(),
        "--expected-revision",
        &f.revision(),
    ]);
    let current_sha = ledger_sha();
    let verified = || {
        f.ok(&["intake", "inspect", "--source-sha", &current_sha])
        ["organization"]["verified_completed"].as_u64().unwrap()
    };
    assert_eq!(verified(), 2);
    let (store, project) = f.store();
    for (work, key) in [("W", "E"), ("OTHER", "other-report")] {
        let evidence = store
            .evidence_for_work(project, work, Some(&current_sha), None)
            .unwrap();
        let found = evidence
            .iter()
            .find(|e| e.evidence.item.external_key == key)
            .unwrap();
        assert_eq!(found.currency, EvidenceCurrency::Current);
        assert_eq!(
            found.evidence.item.source_sha.as_deref(),
            Some(original_sha.as_str())
        );
    }
    drop(store);

    // Even across ledger drift, an unselected new passing report cannot replace
    // a damaged selected report (including extra required evidence).
    let mut replacement = f.report();
    replacement["source_sha"] = json!(current_sha);
    f.evidence("replacement", &replacement, |d| {
        d.source_sha = Some(current_sha.clone())
    });
    let bytes = fs::read(f.root.join(&required.locator)).unwrap();
    // Preserve all binding fields and passing criteria, but replace the reported
    // observation. Re-parsing alone cannot enforce the selected proof's digest.
    let mut changed_report: Value = serde_json::from_slice(&bytes).unwrap();
    changed_report["checks"][0]["details"] = json!("A different passing observation");
    fs::write(
        f.root.join(&required.locator),
        serde_json::to_vec_pretty(&changed_report).unwrap(),
    )
    .unwrap();
    assert_eq!(verified(), 1);
    fs::write(f.root.join(&required.locator), bytes).unwrap();
    assert_eq!(verified(), 2);

    // Changing this task's acceptance still invalidates its original proof.
    let body = fs::read_to_string(f.root.join("work.yaml")).unwrap();
    fs::write(
        f.root.join("work.yaml"),
        body.replacen(C2, "A new review is required", 1),
    )
    .unwrap();
    let changed_sha = ledger_sha();
    let status = f.ok(&["intake", "inspect", "--source-sha", &changed_sha]);
    assert_eq!(status["organization"]["verified_completed"], 1);
}

#[test]
fn ledger_drift_cannot_drop_unstamped_extra_required_evidence() {
    let f = Fixture::new();
    f.organize();
    let ledger_sha = || {
        awr_source::fingerprint(&fs::read(f.root.join("work.yaml")).unwrap())
            .trim_start_matches("sha256:")
            .to_owned()
    };
    let original = ledger_sha();
    let mut report = f.report();
    report["source_sha"] = json!(original);
    let selected = f.evidence("E", &report, |d| d.source_sha = Some(original.clone()));
    let required = f.evidence("R", &report, |d| {
        d.source_sha = Some(original.clone());
        // Scope links this proof to W, but it was not stamped against W's contract.
        d.work_item_key = None;
    });
    assert!(selected.section_fingerprint.is_some());
    assert!(required.section_fingerprint.is_none());
    let mut input = f.input();
    input["source_sha"] = json!(original);
    input["required_evidence"] = json!(["R"]);
    Fixture::success(&f.complete(&input));
    let current = ledger_sha();
    assert_ne!(current, original);
    // E can cross the ledger change, but R cannot. Keep R mandatory instead of
    // falling back to E alone just because E covers every acceptance criterion.
    let checked = f.ok(&["intake", "inspect", "--source-sha", &current]);
    assert_eq!(checked["organization"]["verified_completed"], 0);
    let original_checked = f.ok(&["intake", "inspect", "--source-sha", &original]);
    assert_eq!(original_checked["organization"]["verified_completed"], 1);
}

#[test]
fn completion_query_uses_applied_selection_and_keeps_rejected_attempts() {
    let f = Fixture::new();
    f.organize();
    let mut malformed = f.report();
    malformed["checks"][0]
        .as_object_mut()
        .unwrap()
        .remove("name");
    let rejected = f.evidence("rejected-attempt", &malformed, |_| {});
    let rejected_bytes = fs::read(f.root.join(&rejected.locator)).unwrap();
    let selected = f.evidence("E", &f.report(), |_| {});
    Fixture::success(&f.complete(&f.input()));
    assert_eq!(f.verified_completed(), 1);
    let (store, project) = f.store();
    let binding = store
        .current_completion_binding(project, "W", None)
        .unwrap()
        .unwrap();
    assert_eq!(binding.evidence.len(), 1);
    assert_eq!(binding.evidence[0].id, selected.id);
    assert_eq!(
        store.evidence(project, "rejected-attempt").unwrap().item.id,
        rejected.id
    );
    assert_eq!(
        fs::read(f.root.join(&rejected.locator)).unwrap(),
        rejected_bytes
    );
    assert_eq!(f.claim_status(), "released");
}

#[test]
fn completion_query_cannot_replace_damaged_selected_or_extra_required_evidence() {
    let f = Fixture::new();
    f.organize();
    let selected = f.evidence("E", &f.report(), |_| {});
    let required = f.evidence("R", &f.report(), |_| {});
    let required_bytes = fs::read(f.root.join(&required.locator)).unwrap();
    let mut input = f.input();
    input["required_evidence"] = json!(["R"]);
    Fixture::success(&f.complete(&input));
    f.evidence("unselected-replacement", &f.report(), |_| {});
    assert_eq!(f.verified_completed(), 1);
    fs::write(f.root.join(&required.locator), "{}").unwrap();
    assert_eq!(f.verified_completed(), 0);
    fs::write(f.root.join(&required.locator), required_bytes).unwrap();
    assert_eq!(f.verified_completed(), 1);
    fs::remove_file(f.root.join(&selected.locator)).unwrap();
    assert_eq!(f.verified_completed(), 0);
}

#[test]
fn completion_selection_is_scoped_to_current_work_facts_branch_and_dependencies() {
    let f = Fixture::new();
    f.organize();
    f.evidence("E", &f.report(), |_| {});
    Fixture::success(&f.complete(&f.input()));
    assert_eq!(f.verified_completed(), 1);
    let branch = f.ok(&[
        "branch",
        "create",
        "separate-review",
        "--actor",
        "worker-a",
        "--reason",
        "Review separate runtime scope",
        "--expected-revision",
        &f.revision(),
    ]);
    let branch_id: Id = branch["branch"]["id"].as_str().unwrap().parse().unwrap();
    let (store, project) = f.store();
    assert!(
        store
            .current_completion_binding(project, "W", Some(branch_id))
            .unwrap()
            .is_none()
    );
    drop(store);
    let body = fs::read_to_string(f.root.join("work.yaml")).unwrap();
    fs::write(
        f.root.join("work.yaml"),
        body.replace("- id: OTHER\n", "- id: OTHER\n  title: Unrelated work\n"),
    )
    .unwrap();
    f.ok(&["source", "reindex"]);
    assert!(
        f.store()
            .0
            .current_completion_binding(project, "W", None)
            .unwrap()
            .is_some()
    );
    assert_eq!(f.verified_completed(), 1);
    fs::write(
        f.root.join("deps.yaml"),
        "work_items:\n- id: D\n  status: blocked\n",
    )
    .unwrap();
    assert_eq!(f.verified_completed(), 0);
    fs::write(
        f.root.join("deps.yaml"),
        "work_items:\n- id: D\n  status: completed\n",
    )
    .unwrap();
    assert_eq!(f.verified_completed(), 1);
    fs::write(
        f.root.join("work.yaml"),
        body.replace(C2, "The report receives a new independent review"),
    )
    .unwrap();
    f.ok(&["source", "reindex"]);
    assert!(
        f.store()
            .0
            .current_completion_binding(project, "W", None)
            .unwrap()
            .is_none()
    );
    assert_eq!(f.verified_completed(), 0);
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn report_diagnostics_agree_between_preflight_and_completion() {
    for (case, pointer, code) in [
        ("missing_passed", "/checks/0/passed", "InvalidInput"),
        ("source_sha", "/source_sha", "EvidenceMissing"),
        ("failed_check", "/checks/0/passed", "EvidenceMissing"),
    ] {
        let f = Fixture::new();
        let mut report = f.report();
        match case {
            "missing_passed" => {
                report["checks"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("passed");
            }
            "source_sha" => report["source_sha"] = json!("b".repeat(40)),
            _ => report["checks"][0]["passed"] = json!(false),
        }
        let evidence = f.evidence("E", &report, |_| {});
        let source = fs::read(f.root.join("work.yaml")).unwrap();
        let revision = f.revision();
        let preflight = f.run(&[
            "work",
            "prepare-completion",
            "W",
            "--report",
            &evidence.locator,
            "--evidence-key",
            "E",
            "--source-sha",
            SHA,
        ]);
        Fixture::error(&preflight, code);
        let completion = f.complete(&f.input());
        Fixture::error(&completion, "EvidenceMissing");
        let preflight: Value = serde_json::from_slice(&preflight.stderr).unwrap();
        let completion: Value = serde_json::from_slice(&completion.stderr).unwrap();
        assert_eq!(
            preflight["details"]["location"]["pointer"], pointer,
            "{case}: {preflight}"
        );
        assert_eq!(preflight["details"], completion["details"], "{case}");
        assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), source);
        assert_eq!(f.revision(), revision);
        assert_eq!(f.claim_status(), "active");
    }
}

#[test]
fn report_semantic_locations_follow_aliases_and_positional_representations() {
    for (form, pointer) in [
        ("result_alias", "/checks/0/result"),
        ("check_sequence", "/checks/0/1"),
        ("report_sequence", "/6/0/passed"),
        ("nested_sequences", "/6/0/1"),
        ("sequence_sha", "/2"),
    ] {
        let f = Fixture::new();
        let mut report = f.report();
        let verified_at = report["verified_at"].as_i64().unwrap();
        if form == "result_alias" {
            let check = report["checks"][0].as_object_mut().unwrap();
            let details = check.remove("details").unwrap();
            check.insert("result".into(), details);
        } else {
            if matches!(form, "check_sequence" | "nested_sequences") {
                let check = &report["checks"][0];
                report["checks"][0] = json!([
                    check["name"],
                    check["passed"],
                    check["details"],
                    check["criteria"]
                ]);
            }
            if matches!(
                form,
                "report_sequence" | "nested_sequences" | "sequence_sha"
            ) {
                report = json!([
                    report["version"],
                    report["work_item"],
                    report["source_sha"],
                    report["command"],
                    report["scope"],
                    report["verified_at"],
                    report["checks"]
                ]);
            }
        }
        let valid = report.clone();
        *report.pointer_mut(pointer).unwrap() = match form {
            "result_alias" => json!(""),
            "sequence_sha" => json!("b".repeat(40)),
            _ => json!(false),
        };
        // These remain supported serde representations; rejection is semantic.
        assert!(serde_json::from_value::<CompletionReport>(report.clone()).is_ok());
        assert!(report.pointer(pointer).is_some());
        let evidence = f.evidence("E", &report, |draft| draft.verified_at = Some(verified_at));
        let source = fs::read(f.root.join("work.yaml")).unwrap();
        let revision = f.revision();
        let preflight = f.run(&[
            "work",
            "prepare-completion",
            "W",
            "--report",
            &evidence.locator,
            "--evidence-key",
            "E",
            "--source-sha",
            SHA,
        ]);
        Fixture::error(&preflight, "EvidenceMissing");
        let completion = f.complete(&f.input());
        Fixture::error(&completion, "EvidenceMissing");
        let preflight: Value = serde_json::from_slice(&preflight.stderr).unwrap();
        let completion: Value = serde_json::from_slice(&completion.stderr).unwrap();
        assert_eq!(
            preflight["details"]["location"]["pointer"], pointer,
            "{form}"
        );
        assert_eq!(preflight["details"], completion["details"], "{form}");
        assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), source);
        assert_eq!(f.revision(), revision);
        assert_eq!(f.claim_status(), "active");
        let corrected = f.evidence("corrected", &valid, |draft| {
            draft.verified_at = Some(verified_at)
        });
        let prepared = f.ok(&[
            "work",
            "prepare-completion",
            "W",
            "--report",
            &corrected.locator,
            "--evidence-key",
            "corrected",
            "--source-sha",
            SHA,
        ]);
        Fixture::success(&f.complete(&prepared["completion"]));
        assert_eq!(f.work()["status"], "completed");
        assert_eq!(f.claim_status(), "released");
    }
}

#[test]
fn duplicate_report_fields_keep_parser_diagnostics_across_completion_paths() {
    for alias in [false, true] {
        let f = Fixture::new();
        f.organize();
        let report = f.report();
        let bytes = serde_json::to_string(&report).unwrap();
        let bytes = if alias {
            bytes.replace(
                "\"details\":\"Checked the analysis and delivery receipt\"",
                "\"result\":\"synthetic-private-value\",\"details\":false",
            )
        } else {
            bytes.replace("\"version\":1", "\"version\":1,\"version\":false")
        };
        let evidence = f.evidence("E", &report, |draft| {
            draft.sha256 = Some(
                awr_source::fingerprint(bytes.as_bytes())
                    .trim_start_matches("sha256:")
                    .into(),
            );
        });
        fs::write(f.root.join(&evidence.locator), &bytes).unwrap();
        let source = fs::read_to_string(f.root.join("work.yaml")).unwrap();
        let revision = f.revision();
        let preflight = f.run(&[
            "work",
            "prepare-completion",
            "W",
            "--report",
            &evidence.locator,
            "--evidence-key",
            "E",
            "--source-sha",
            SHA,
        ]);
        Fixture::error(&preflight, "InvalidInput");
        let completion = f.complete(&f.input());
        Fixture::error(&completion, "EvidenceMissing");
        let preflight: Value = serde_json::from_slice(&preflight.stderr).unwrap();
        let completion: Value = serde_json::from_slice(&completion.stderr).unwrap();
        assert_eq!(preflight["details"], completion["details"]);
        let details = &preflight["details"];
        assert_eq!(details["location"]["pointer"], Value::Null);
        assert!(details["location"]["line"].is_u64());
        assert!(details["location"]["column"].is_u64());
        assert!(
            details["repair"]
                .as_str()
                .unwrap()
                .contains("duplicate fields")
        );
        assert_eq!(
            fs::read_to_string(f.root.join("work.yaml")).unwrap(),
            source
        );
        assert_eq!(f.revision(), revision);
        assert_eq!(f.claim_status(), "active");
        fs::write(
            f.root.join("work.yaml"),
            source.replace("status: in_progress", "status: completed"),
        )
        .unwrap();
        f.ok(&["source", "reindex"]);
        let inspected = f.ok(&["intake", "inspect", "--source-sha", SHA]);
        assert_eq!(inspected["organization"]["verified_completed"], 0);
        let gap = inspected["organization"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|gap| gap["target"] == "W" && gap["code"] == "completion_verification_failed")
            .unwrap();
        let detail = gap["detail"].as_str().unwrap();
        assert!(
            detail.contains(&format!(
                "Source: {}:{}:{}",
                evidence.locator, details["location"]["line"], details["location"]["column"]
            )),
            "{gap}"
        );
        assert!(
            detail.contains(details["repair"].as_str().unwrap()),
            "{gap}"
        );
        assert!(!detail.contains("Field:"), "{gap}");
        assert!(!detail.contains("synthetic-private"), "{gap}");
    }
}

#[test]
fn opaque_report_metadata_preserves_completion_and_truthful_fallback_diagnostics() {
    let f = Fixture::new();
    let mut report = f.report();
    let check = report["checks"][0].as_object_mut().unwrap();
    check.remove("details");
    check.insert("result".into(), json!(""));
    let mut bytes = serde_json::to_string(&report).unwrap();
    bytes.pop();
    bytes.push_str(",\"producer\":1e400}");
    assert!(serde_json::from_str::<CompletionReport>(&bytes).is_ok());
    let evidence = f.evidence("E", &report, |draft| {
        draft.sha256 = Some(
            awr_source::fingerprint(bytes.as_bytes())
                .trim_start_matches("sha256:")
                .into(),
        );
    });
    fs::write(f.root.join(&evidence.locator), &bytes).unwrap();
    let source = fs::read(f.root.join("work.yaml")).unwrap();
    let revision = f.revision();
    let preflight = f.run(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        &evidence.locator,
        "--evidence-key",
        "E",
        "--source-sha",
        SHA,
    ]);
    Fixture::error(&preflight, "EvidenceMissing");
    let completion = f.complete(&f.input());
    Fixture::error(&completion, "EvidenceMissing");
    let preflight: Value = serde_json::from_slice(&preflight.stderr).unwrap();
    let completion: Value = serde_json::from_slice(&completion.stderr).unwrap();
    assert_eq!(preflight["details"], completion["details"]);
    assert_eq!(preflight["details"]["location"]["pointer"], Value::Null);
    assert_eq!(
        preflight["details"]["location"]["locator"],
        evidence.locator
    );
    assert_eq!(
        preflight["details"]["rule"],
        "completion.report.check.details"
    );
    assert!(!preflight["details"]["repair"].as_str().unwrap().is_empty());
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), source);
    assert_eq!(f.revision(), revision);
    assert_eq!(f.claim_status(), "active");
    report["checks"][0]["result"] = json!("Checked the analysis and delivery receipt");
    let corrected = bytes.replace(
        "\"result\":\"\"",
        "\"result\":\"Checked the analysis and delivery receipt\"",
    );
    let evidence = f.evidence("corrected", &report, |draft| {
        draft.sha256 = Some(
            awr_source::fingerprint(corrected.as_bytes())
                .trim_start_matches("sha256:")
                .into(),
        );
    });
    fs::write(f.root.join(&evidence.locator), corrected).unwrap();
    let prepared = f.ok(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        &evidence.locator,
        "--evidence-key",
        "corrected",
        "--source-sha",
        SHA,
    ]);
    Fixture::success(&f.complete(&prepared["completion"]));
    assert_eq!(f.work()["status"], "completed");
    assert_eq!(f.claim_status(), "released");
}

#[test]
fn completed_source_revalidation_uses_the_shared_report_explanation() {
    let f = Fixture::new();
    f.organize();
    let mut report = f.report();
    let mut second = report["checks"][0].clone();
    second["name"] = json!("delivery review");
    second.as_object_mut().unwrap().remove("passed");
    report["checks"].as_array_mut().unwrap().push(second);
    let evidence = f.evidence("E", &report, |_| {});
    let preflight = f.run(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        &evidence.locator,
        "--evidence-key",
        "E",
        "--source-sha",
        SHA,
    ]);
    Fixture::error(&preflight, "InvalidInput");
    let preflight: Value = serde_json::from_slice(&preflight.stderr).unwrap();
    assert_eq!(
        preflight["details"]["location"]["pointer"],
        "/checks/1/passed"
    );
    let source = fs::read_to_string(f.root.join("work.yaml")).unwrap();
    fs::write(
        f.root.join("work.yaml"),
        source.replace("status: in_progress", "status: completed"),
    )
    .unwrap();
    f.ok(&["source", "reindex"]);
    let inspected = f.ok(&["intake", "inspect", "--source-sha", SHA]);
    assert_eq!(inspected["organization"]["verified_completed"], 0);
    let gap = inspected["organization"]["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gap| gap["target"] == "W" && gap["code"] == "completion_verification_failed")
        .unwrap();
    let detail = gap["detail"].as_str().unwrap();
    assert!(detail.contains("field passed is required"), "{gap}");
    let text = Command::new(env!("CARGO_BIN_EXE_awr"))
        .arg("--project")
        .arg(&f.root)
        .args(["intake", "inspect", "--source-sha", SHA])
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8(text.stdout).unwrap();
    for expected in [
        format!("Source: {}", evidence.locator),
        "Field: /checks/1/passed".into(),
        format!("Rule: {}", preflight["details"]["rule"].as_str().unwrap()),
        format!(
            "Repair: {}",
            preflight["details"]["repair"].as_str().unwrap()
        ),
    ] {
        assert!(detail.contains(&expected), "{gap}");
        assert!(text.contains(&expected), "{text}");
    }
}

#[test]
fn completion_preserves_metadata_binds_all_proof_and_releases_only_own_claim() {
    let f = Fixture::new();
    let e = f.evidence("E", &f.report(), |d| d.level = EvidenceLevel::Released);
    let extra = f.evidence("support", &f.report(), |_| {});
    let mut input = f.input();
    input["required_evidence"] = json!(["support"]);
    let done = Fixture::success(&f.complete(&input));
    assert_eq!(done["event"]["event_type"], "work.completed");
    assert_eq!(done["proposal"]["status"], "applied");
    assert_eq!(done["source_write_performed"], true);
    assert_eq!(
        done["apply_attempt"]["resolved_event_id"],
        done["event"]["id"]
    );
    assert_eq!(
        done["event"]["payload"]["released_claim_ids"],
        json!([f.claim])
    );
    assert_eq!(f.work()["status"], "completed");
    assert_eq!(f.work()["evidence_level"], "locally_verified");
    assert_eq!(f.work()["owner"], "business-coordinator");
    assert_eq!(f.claim_status(), "released");
    let source = fs::read_to_string(f.root.join("work.yaml")).unwrap();
    for expected in [
        "old-report.json",
        "reviewed_by",
        "coordinator",
        &e.locator,
        &extra.locator,
    ] {
        assert!(source.contains(expected));
    }
    let binding = &done["event"]["payload"]["work_action"]["completion"];
    assert_eq!(binding["evidence"].as_array().unwrap().len(), 2);
    assert_eq!(binding["acceptance"].as_array().unwrap().len(), 2);
    Fixture::error(&f.complete(&input), "InvalidTransition");
    Fixture::error(
        &f.proposal_action(
            "recover",
            done["proposal"]["id"].as_str().unwrap().parse().unwrap(),
        ),
        "InvalidTransition",
    );
}

#[test]
fn completion_requires_exact_nonempty_acceptance_and_registered_proof_without_mutation() {
    let f = Fixture::new();
    f.evidence("E", &f.report(), |_| {});
    let original = fs::read(f.root.join("work.yaml")).unwrap();
    let revision = f.revision();
    for acceptance in [
        json!([]),
        json!([{"criterion":C1,"evidence":["E"]}]),
        json!([{"criterion":C1,"evidence":["E"]},{"criterion":C1,"evidence":["E"]}]),
        json!([{"criterion":C1,"evidence":[]},{"criterion":C2,"evidence":["E"]}]),
    ] {
        let mut input = f.input();
        input["acceptance"] = acceptance;
        Fixture::error(&f.complete(&input), "EvidenceMissing");
    }
    let mut input = f.input();
    input["required_evidence"] = json!(["absent"]);
    Fixture::error(&f.complete(&input), "NotFound");
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), original);
    assert_eq!(f.revision(), revision);
    assert_eq!(f.claim_status(), "active");
}

#[test]
fn completion_checks_report_results_work_sha_scope_command_time_coverage_and_level() {
    for case in [
        "version",
        "failed",
        "wrong_work",
        "wrong_sha",
        "wrong_scope",
        "wrong_command",
        "future",
        "no_checks",
        "no_details",
        "no_coverage",
        "unknown_criteria",
        "wrong_evidence_work",
        "stale_evidence_sha",
        "unknown_level",
        "low_level",
        "higher_minimum",
    ] {
        let f = Fixture::new();
        let mut report = f.report();
        let mut input = f.input();
        match case {
            "version" => report["version"] = json!(2),
            "failed" => report["checks"][0]["passed"] = json!(false),
            "wrong_work" => report["work_item"] = json!("OTHER"),
            "wrong_sha" => report["source_sha"] = json!("b".repeat(40)),
            "wrong_scope" => report["scope"] = json!(["OTHER"]),
            "wrong_command" => report["command"] = json!("different command"),
            "future" => report["verified_at"] = json!(now_millis().unwrap() + 86400000),
            "no_checks" => report["checks"] = json!([]),
            "no_details" => report["checks"][0]["details"] = json!(""),
            "no_coverage" => report["checks"][0]["criteria"] = json!([C1]),
            "unknown_criteria" => report["checks"][0]["criteria"] = json!([C1, C2, "invented"]),
            "higher_minimum" => input["minimum_level"] = json!("real_environment_validated"),
            _ => (),
        }
        f.evidence("E", &report, |d| match case {
            "wrong_evidence_work" => d.work_item_key = Some("OTHER".into()),
            "stale_evidence_sha" => d.source_sha = Some("b".repeat(40)),
            "unknown_level" => d.level = EvidenceLevel::Unknown,
            "low_level" => d.level = EvidenceLevel::Implemented,
            _ => (),
        });
        let before = fs::read(f.root.join("work.yaml")).unwrap();
        let revision = f.revision();
        Fixture::error(&f.complete(&input), "EvidenceMissing");
        assert_eq!(
            fs::read(f.root.join("work.yaml")).unwrap(),
            before,
            "{case}"
        );
        assert_eq!(f.revision(), revision, "{case}");
    }
}

#[test]
fn completion_reads_report_files_and_rejects_drift_missing_and_oversized_bytes() {
    for case in ["drift", "missing", "oversize"] {
        let f = Fixture::new();
        let e = f.evidence("E", &f.report(), |_| {});
        match case {
            "drift" => fs::write(f.root.join(&e.locator), "changed report").unwrap(),
            "missing" => fs::remove_file(f.root.join(&e.locator)).unwrap(),
            _ => fs::write(f.root.join(&e.locator), vec![b'x'; 1024 * 1024 + 1]).unwrap(),
        }
        let before = fs::read(f.root.join("work.yaml")).unwrap();
        let revision = f.revision();
        let r = f.complete(&f.input());
        assert!(!r.status.success(), "{case}");
        assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), before);
        assert_eq!(f.revision(), revision);
    }
}

#[test]
fn completion_orders_revision_dependencies_acceptance_evidence_and_blocker_gates() {
    let f = Fixture::new();
    Fixture::error(
        &f.run(&[
            "work",
            "complete",
            "W",
            "--session",
            &f.session,
            "--reason",
            "Review",
            "--input",
            "absent.json",
            "--expected-revision",
            "0",
        ]),
        "RevisionConflict",
    );
    fs::write(
        f.root.join("deps.yaml"),
        "work_items:\n- id: D\n  status: blocked\n",
    )
    .unwrap();
    let mut input = f.input();
    input["acceptance"] = json!([]);
    Fixture::error(&f.complete(&input), "SourceConflict");
    f.ok(&["source", "reindex"]);
    Fixture::error(&f.complete(&input), "DependencyBlocked");
    fs::write(
        f.root.join("deps.yaml"),
        "work_items:\n- id: D\n  status: completed\n",
    )
    .unwrap();
    fs::write(
        f.root.join("work.yaml"),
        WORK.replacen(
            "status: in_progress",
            "status: in_progress\n  blocker: Waiting for signoff",
            1,
        ),
    )
    .unwrap();
    f.ok(&["source", "reindex"]);
    Fixture::error(&f.complete(&input), "EvidenceMissing");
    Fixture::error(&f.complete(&f.input()), "NotFound");
    f.evidence("E", &f.report(), |_| {});
    Fixture::error(&f.complete(&f.input()), "DependencyBlocked");
    assert_eq!(f.work()["status"], "in_progress");
}

#[test]
fn completion_requires_current_work_bound_claim_and_does_not_rewrite_business_owner() {
    let f = Fixture::new();
    f.evidence("E", &f.report(), |_| {});
    f.ok(&[
        "work",
        "release",
        "W",
        "--session",
        &f.session,
        "--claim",
        &f.claim,
        "--expected-revision",
        &f.revision(),
    ]);
    let before = fs::read(f.root.join("work.yaml")).unwrap();
    Fixture::error(&f.complete(&f.input()), "ClaimConflict");
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), before);
}

#[test]
fn proposal_apply_rechecks_actual_proof_and_refuses_source_metadata_forgery() {
    for forged in [false, true] {
        let f = Fixture::new();
        let e = f.evidence("E", &f.report(), |_| {});
        let draft=f.draft(e.clone(),forged.then(||json!({"status":"completed","evidence":[e.locator],"verification":{"evidence_level":"released"}})));
        let id = f.approved(draft);
        let before = fs::read(f.root.join("work.yaml")).unwrap();
        if !forged {
            fs::write(f.root.join(&e.locator), "altered after approval").unwrap();
        }
        Fixture::error(
            &f.proposal_action("apply", id),
            if forged {
                "InvalidInput"
            } else {
                "SourceConflict"
            },
        );
        assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), before);
        assert_eq!(f.work()["status"], "in_progress");
        assert_eq!(f.claim_status(), "active");
    }
}

#[test]
fn completion_receipt_failure_keeps_recoverable_proof_and_claim_until_atomic_finalization() {
    let f = Fixture::new();
    let e = f.evidence("E", &f.report(), |_| {});
    let bytes = fs::read(f.root.join(&e.locator)).unwrap();
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_completion BEFORE INSERT ON events WHEN NEW.event_type='work.completed' BEGIN SELECT RAISE(ABORT,'injected completion receipt failure'); END;").unwrap();
    let r = f.complete(&f.input());
    Fixture::error(&r, "MutationIncomplete");
    let failed: Value = serde_json::from_slice(&r.stdout).unwrap();
    let id: Id = failed["proposal"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(failed["proposal"]["status"], "approved");
    assert_eq!(failed["source_write_performed"], true);
    assert_eq!(f.work()["status"], "completed");
    assert_eq!(f.claim_status(), "active");
    db.execute_batch("DROP TRIGGER reject_completion").unwrap();
    fs::write(f.root.join(&e.locator), "changed during interruption").unwrap();
    Fixture::error(&f.proposal_action("recover", id), "MutationIncomplete");
    assert_eq!(f.claim_status(), "active");
    fs::write(f.root.join(&e.locator), bytes).unwrap();
    let r = Fixture::success(&f.proposal_action("recover", id));
    assert_eq!(r["event"]["event_type"], "work.completed");
    assert_eq!(r["source_write_performed"], false);
    assert_eq!(r["apply_attempt"]["resolved_event_id"], r["event"]["id"]);
    assert_eq!(f.claim_status(), "released");
    assert_eq!(
        f.ok(&["work", "history", "W", "--event-type", "work.completed"])["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn completion_namespace_collisions_and_replaced_records_fail_before_source_writing() {
    for collision in [false, true] {
        let f = Fixture::new();
        let e = f.evidence("E", &f.report(), |_| {});
        if collision {
            f.evidence(&format!("W/evidence/{}", e.locator), &f.report(), |_| {});
            Fixture::error(&f.complete(&f.input()), "SourceConflict");
        } else {
            let mut draft = f.draft(e, None);
            draft
                .patch
                .work_action
                .as_mut()
                .unwrap()
                .completion
                .as_mut()
                .unwrap()
                .evidence[0]
                .summary = "forged bound metadata".into();
            let (mut s, p) = f.store();
            let r = s.project(p).unwrap().project_revision;
            assert!(matches!(
                s.create_proposal(p, r, draft),
                Err(Error::EvidenceMissing(_))
            ));
        }
        assert_eq!(f.work()["status"], "in_progress");
        assert_eq!(f.claim_status(), "active");
    }
}

#[test]
fn completing_reopened_work_requires_a_new_revision_and_keeps_prior_receipt() {
    let f = Fixture::new();
    f.evidence("E", &f.report(), |_| {});
    let first = Fixture::success(&f.complete(&f.input()));
    f.ok(&[
        "work",
        "reopen",
        "W",
        "--session",
        &f.session,
        "--reason",
        "Additional review is needed",
        "--next-action",
        "Review new comments",
        "--expected-revision",
        &f.revision(),
    ]);
    Fixture::error(&f.complete(&f.input()), "InvalidTransition");
    f.ok(&[
        "work",
        "claim",
        "W",
        "--session",
        &f.session,
        "--expected-revision",
        &f.revision(),
    ]);
    f.ok(&[
        "work",
        "progress",
        "W",
        "--session",
        &f.session,
        "--reason",
        "Recheck the delivery",
        "--next-action",
        "Close after review",
        "--expected-revision",
        &f.revision(),
    ]);
    let second = Fixture::success(&f.complete(&f.input()));
    assert_ne!(first["event"]["id"], second["event"]["id"]);
    assert_eq!(
        f.ok(&["work", "history", "W", "--event-type", "work.completed"])["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn immutable_evidence_ids_survive_external_key_collisions_and_accept_producer_metadata() {
    let f = Fixture::new();
    let mut report = f.report();
    let details = report["checks"][0]
        .as_object_mut()
        .unwrap()
        .remove("details")
        .unwrap();
    report["checks"][0]["result"] = details;
    report["producer_receipt"] =
        json!({"kind":"local verification","independent_acceptance":false});
    let evidence = f.evidence("E", &report, |_| {});
    f.evidence(&evidence.id.to_string(), &f.report(), |_| {});
    let r = Fixture::success(&f.complete(&f.input()));
    assert_eq!(
        r["event"]["payload"]["work_action"]["completion"]["evidence"][0]["id"],
        json!(evidence.id)
    );
}

#[test]
fn completion_recovery_retains_newer_source_without_confirming_completion() {
    let f = Fixture::new();
    f.evidence("E", &f.report(), |_| {});
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_completion BEFORE INSERT ON events WHEN NEW.event_type='work.completed' BEGIN SELECT RAISE(ABORT,'injected completion receipt failure'); END;").unwrap();
    let r = f.complete(&f.input());
    Fixture::error(&r, "MutationIncomplete");
    let failed: Value = serde_json::from_slice(&r.stdout).unwrap();
    let id = failed["proposal"]["id"].as_str().unwrap().parse().unwrap();
    db.execute_batch("DROP TRIGGER reject_completion").unwrap();
    let newer = fs::read_to_string(f.root.join("work.yaml")).unwrap() + "\n# New external edit\n";
    fs::write(f.root.join("work.yaml"), &newer).unwrap();
    Fixture::error(&f.proposal_action("recover", id), "SourceConflict");
    assert_eq!(fs::read_to_string(f.root.join("work.yaml")).unwrap(), newer);
    assert_eq!(
        f.ok(&["proposal", "show", &id.to_string()])["proposal"]["status"],
        "conflict"
    );
    assert_eq!(f.claim_status(), "active");
    assert_eq!(
        f.ok(&["work", "history", "W", "--event-type", "work.completed"])["events"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn completion_report_byte_mismatch_names_the_sha256_and_newline_cause() {
    let f = Fixture::new();
    f.organize();
    let selected = f.evidence("E", &f.report(), |_| {});
    // One trailing newline changes the bytes without changing the parsed JSON.
    let path = f.root.join(&selected.locator);
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(&path, &bytes).unwrap();
    let result = f.complete(&f.input());
    Fixture::error(&result, "SourceConflict");
    let message = String::from_utf8_lossy(&result.stderr);
    assert!(
        message.contains("SHA256 bound at registration"),
        "{message}"
    );
    assert!(message.contains("trailing newlines"), "{message}");
    assert!(
        message.contains(selected.sha256.as_deref().unwrap()),
        "{message}"
    );
}

#[test]
fn completion_report_criterion_mismatch_names_the_field_without_the_submitted_text() {
    let f = Fixture::new();
    f.organize();
    let mut report = f.report();
    // A criterion that does not exist in the ledger, shaped like private
    // content; the diagnostic names the JSON pointer but must not echo it.
    let submitted = "reviewed privately by 9f8e7d6c5b4a";
    report["checks"][0]["criteria"] = json!([C1, submitted]);
    let selected = f.evidence("E", &report, |_| {});
    assert!(selected.sha256.is_some());
    let result = f.complete(&f.input());
    Fixture::error(&result, "EvidenceMissing");
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(
        error["details"]["location"]["pointer"],
        "/checks/0/criteria/1"
    );
    assert_eq!(error["details"]["location"]["locator"], selected.locator);
    assert_eq!(error["details"]["rule"], "completion.report.check.criteria");
    assert!(
        error["details"]["repair"]
            .as_str()
            .unwrap()
            .contains("exact criterion"),
        "{error}"
    );
    assert!(!error.to_string().contains(submitted), "{error}");
}

#[test]
fn completion_schema_error_does_not_echo_submitted_values() {
    let f = Fixture::new();
    // Passes the public-content filter (no credential keywords) but is still a
    // private submitted value; the serde echo must not surface it verbatim.
    let value = "private-hostname-9f8e7d6c5b4a.internal";
    let mut input = f.input();
    input["version"] = json!(value);
    let result = f.complete(&input);
    Fixture::error(&result, "InvalidInput");
    let message = String::from_utf8_lossy(&result.stderr);
    assert!(message.contains("does not match its schema"), "{message}");
    assert!(!message.contains(value), "{message}");
    assert!(message.contains('…'), "{message}");
}

#[test]
fn completion_binding_errors_name_fields_without_submitted_values() {
    let f = Fixture::new();
    f.organize();
    // The report carries secret-shaped submitted values in every field the
    // binding checks; each diagnostic names the JSON pointer and the static
    // rule, never the submitted content.
    let mut report = f.report();
    report["work_item"] = json!("secret-project-9f8e7d6c5b4a");
    report["command"] = json!("curl -H private-9f8e7d6c5b4a");
    report["scope"] = json!(["secret-scope-9f8e7d6c5b4a"]);
    let selected = f.evidence("E", &report, |_| {});
    assert!(selected.sha256.is_some());
    let result = f.complete(&f.input());
    Fixture::error(&result, "EvidenceMissing");
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(error["details"]["location"]["pointer"], "/work_item");
    assert_eq!(error["details"]["location"]["locator"], selected.locator);
    assert_eq!(error["details"]["rule"], "completion.report.work_item");
    assert!(!error["details"]["repair"].as_str().unwrap().is_empty());
    let message = String::from_utf8_lossy(&result.stderr);
    assert!(message.contains("/work_item"), "{message}");
    assert!(
        !message.contains("secret-project-9f8e7d6c5b4a"),
        "{message}"
    );
    assert!(!message.contains("private-9f8e7d6c5b4a"), "{message}");
    assert!(!message.contains("secret-scope-9f8e7d6c5b4a"), "{message}");
}

#[test]
fn work_prepare_completion_warns_on_non_canonical_reports_without_touching_the_file() {
    let f = Fixture::new();
    f.organize();
    // The report is authored without the final newline that lint would add —
    // the form json.dumps, JSON.stringify and serde_json::to_writer produce.
    let report = f.report();
    let locator = "report-prepare.json";
    let bytes = serde_json::to_vec_pretty(&report).unwrap();
    assert!(!bytes.ends_with(b"\n"));
    fs::write(f.root.join(locator), &bytes).unwrap();

    // The rule is advisory for this release: the preflight succeeds, keeps
    // the byte-exact digest of the stored report, and returns a warning that
    // names the canonical form and the digest its bytes would have.
    let prepared = f.ok(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        locator,
        "--evidence-key",
        "E-PREP",
        "--source-sha",
        SHA,
    ]);
    let warning = prepared["warning"].as_str().unwrap();
    assert!(warning.contains("no trailing newline"), "{warning}");
    assert!(warning.contains("canonical form"), "{warning}");
    let canonical = [bytes.as_slice(), b"\n"].concat();
    let would = awr_source::fingerprint(&canonical)
        .trim_start_matches("sha256:")
        .to_string();
    assert!(warning.contains(&would), "{warning}");
    // The digest binds the exact stored bytes and the caller's file is
    // untouched.
    assert_eq!(
        prepared["report_sha256"].as_str().unwrap(),
        awr_source::fingerprint(&bytes).trim_start_matches("sha256:")
    );
    assert_eq!(fs::read(f.root.join(locator)).unwrap(), bytes.as_slice());

    // More than one trailing newline is equally non-canonical.
    let doubled = [bytes.as_slice(), b"\n\n"].concat();
    fs::write(f.root.join(locator), &doubled).unwrap();
    let prepared = f.ok(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        locator,
        "--evidence-key",
        "E-PREP",
        "--source-sha",
        SHA,
    ]);
    let warning = prepared["warning"].as_str().unwrap();
    assert!(warning.contains("more than one newline"), "{warning}");
    assert!(warning.contains(&would), "{warning}");

    // Windows CRLF endings are non-canonical too: a single CRLF warns with
    // the LF-converted digest, and doubled CRLFs count as multiple endings.
    let crlf = [bytes.as_slice(), b"\r\n"].concat();
    fs::write(f.root.join(locator), &crlf).unwrap();
    let prepared = f.ok(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        locator,
        "--evidence-key",
        "E-PREP",
        "--source-sha",
        SHA,
    ]);
    let warning = prepared["warning"].as_str().unwrap();
    assert!(warning.contains("CRLF"), "{warning}");
    assert!(warning.contains(&would), "{warning}");
    assert_eq!(
        prepared["report_sha256"].as_str().unwrap(),
        awr_source::fingerprint(&crlf).trim_start_matches("sha256:")
    );
    let doubled_crlf = [bytes.as_slice(), b"\r\n\r\n"].concat();
    fs::write(f.root.join(locator), &doubled_crlf).unwrap();
    let prepared = f.ok(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        locator,
        "--evidence-key",
        "E-PREP",
        "--source-sha",
        SHA,
    ]);
    let warning = prepared["warning"].as_str().unwrap();
    assert!(warning.contains("more than one newline"), "{warning}");
    assert!(warning.contains(&would), "{warning}");

    // A bare trailing CR (no LF) is not a line ending: it warns as a missing
    // newline, with the digest the exact stored bytes plus one LF would have
    // (the bare-`\r` case the second-round review asked to cover).
    let bare_cr = [bytes.as_slice(), b"\r"].concat();
    fs::write(f.root.join(locator), &bare_cr).unwrap();
    let prepared = f.ok(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        locator,
        "--evidence-key",
        "E-PREP",
        "--source-sha",
        SHA,
    ]);
    let warning = prepared["warning"].as_str().unwrap();
    assert!(warning.contains("no trailing newline"), "{warning}");
    let bare_cr_canonical = [bare_cr.as_slice(), b"\n"].concat();
    assert!(
        warning.contains(awr_source::fingerprint(&bare_cr_canonical).trim_start_matches("sha256:")),
        "{warning}"
    );

    // The canonical form produces no warning; the digest still binds the
    // exact stored bytes and completion verifies strictly.
    fs::write(f.root.join(locator), &canonical).unwrap();
    let prepared = f.ok(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        locator,
        "--evidence-key",
        "E-PREP",
        "--source-sha",
        SHA,
    ]);
    assert!(prepared["warning"].is_null(), "{}", prepared["warning"]);
    assert_eq!(
        prepared["report_sha256"].as_str().unwrap(),
        awr_source::fingerprint(&canonical).trim_start_matches("sha256:")
    );

    // Registering the prepared digest and completing verifies strictly.
    let selected = f.evidence("E-PREP", &report, |d| {
        d.locator = locator.into();
        d.sha256 = Some(
            prepared["report_sha256"]
                .as_str()
                .unwrap()
                .trim_start_matches("sha256:")
                .into(),
        );
    });
    assert!(selected.sha256.is_some());
    let input = json!({
        "version": 1,
        "source_sha": SHA,
        "acceptance": [
            {"criterion": C1, "evidence": ["E-PREP"]},
            {"criterion": C2, "evidence": ["E-PREP"]}
        ]
    });
    let result = f.complete(&input);
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
