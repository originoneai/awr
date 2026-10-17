use awr_core::Id;
use serde_json::{Value, json};
use sha2::Digest;
use std::{fs, path::PathBuf, process::Command};

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Project {
    fn new(items: Value, mut works: Value) -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-spec-check-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(p.0.join("mapping.toml"), "[project]\nname='Explicit specification fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n").unwrap();
        p.spec("accepted", "0.80", items);
        let fingerprint = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(fs::read(p.0.join("decisions/dashboard.md")).unwrap())
        );
        for work in works.as_array_mut().unwrap() {
            if let Some(refs) = work.get_mut("spec_refs").and_then(Value::as_array_mut) {
                for r in refs {
                    if r["specification"] == "dashboard" && r["approved_version"] == "0.80" {
                        r["source_fingerprint"] = json!(fingerprint);
                    }
                }
            }
        }
        fs::write(
            p.0.join("work.yaml"),
            serde_json::to_vec_pretty(&json!({"work_items":works})).unwrap(),
        )
        .unwrap();
        p.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        p
    }
    fn spec(&self, status: &str, version: &str, items: Value) {
        let metadata = json!({"id":"dashboard","status":status,"specification":{"version":version,"items":items}});
        fs::write(
            self.0.join("decisions/dashboard.md"),
            format!("---\n{}\n---\n# Dashboard\n\n## Decision\nUse the explicitly approved requirements.\n", serde_json::to_string_pretty(&metadata).unwrap()),
        )
        .unwrap();
    }
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn logical_state(&self) -> (i64, i64, i64, i64) {
        let db = rusqlite::Connection::open(self.0.join(".awr/state.db")).unwrap();
        db.query_row("SELECT (SELECT project_revision FROM projects LIMIT 1), (SELECT count(*) FROM events), (SELECT count(*) FROM sessions), (SELECT count(*) FROM claims)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap()
    }
    fn document(&self, key: &str, metadata: Value) {
        fs::write(self.0.join(format!("decisions/{key}.md")), format!("---\n{}\n---\n# Synthetic specification\n\n## Decision\nUse the explicit requirements.\n",serde_json::to_string_pretty(&metadata).unwrap())).unwrap();
    }
    fn version(&self, key: &str) -> Value {
        let sources = self.ok(&["source", "list"]);
        let source = sources["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| {
                s["locator"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("/{key}.md"))
            })
            .unwrap();
        json!({"source_id":source["id"],"external_key":key,"source_fingerprint":source["fingerprint"]})
    }
    fn works(&self, works: Value) {
        fs::write(
            self.0.join("work.yaml"),
            serde_json::to_vec_pretty(&json!({"work_items":works})).unwrap(),
        )
        .unwrap();
        self.ok(&["source", "reindex"]);
    }
    fn adopt(&self, old: &str, new: &str) {
        let request = json!({"version":1,"request_key":format!("adopt-{old}-{new}"),"actor":{"host":"fixture","subject":"reviewer","origin":"human"},"reason":"Adopt the explicitly reviewed specification","change":{"kind":"related","changes":[{"operation":"adopt","candidate":self.version(new),"supersedes":self.version(old)}]}});
        fs::write(
            self.0.join("adopt.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let preview = self.ok(&["batch", "change", "--input", "adopt.json"]);
        self.ok(&[
            "batch",
            "change",
            "--input",
            "adopt.json",
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
            "--expected-revision",
            &preview["project_revision"].to_string(),
        ]);
    }
}

fn reference(item: &str, version: &str) -> Value {
    json!({"specification":"dashboard","item":item,"approved_version":version,"source_fingerprint":format!("sha256:{}","a".repeat(64)),"criterion":"Save the reviewed configuration"})
}

#[test]
fn superseded_reference_is_reported_without_rewriting_sources_or_runtime() {
    let p = Project::new(
        json!([
            {"id":"C64","disposition":"superseded","superseded_by":"C90"},
            {"id":"C90","disposition":"required"}
        ]),
        json!([{"id":"W","title":"Save configuration","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[reference("C64","0.74")]}]),
    );
    let source = fs::read(p.0.join("work.yaml")).unwrap();
    let spec = fs::read(p.0.join("decisions/dashboard.md")).unwrap();
    let state = p.logical_state();
    let result = p.ok(&["spec", "check"]);
    assert_eq!(result["status"], "needs_review");
    assert_eq!(result["references"][0]["work"], "W");
    assert_eq!(result["references"][0]["status"], "needs_review");
    assert_eq!(result["references"][0]["replacement"], "C90");
    assert!(
        result["references"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("approved_version_changed"))
    );
    assert_eq!(result["read_only"], true);
    assert_eq!(fs::read(p.0.join("work.yaml")).unwrap(), source);
    assert_eq!(fs::read(p.0.join("decisions/dashboard.md")).unwrap(), spec);
    assert_eq!(p.logical_state(), state);
}

#[test]
fn future_coverage_distinguishes_missing_work_cancelled_and_out_of_scope() {
    let items = (1..=10)
        .map(|i| json!({"id":format!("F{i}"),"disposition":"future"}))
        .chain([
            json!({"id":"CANCELLED","disposition":"cancelled"}),
            json!({"id":"OUT","disposition":"out_of_scope"}),
        ])
        .collect::<Vec<_>>();
    let works=(1..=9).map(|i|json!({"id":format!("W{i}"),"status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[reference(&format!("F{i}"),"0.80")]})).collect::<Vec<_>>();
    let p = Project::new(json!(items), json!(works));
    let result = p.ok(&["spec", "check", "--spec", "dashboard"]);
    let requirements = result["requirements"].as_array().unwrap();
    let missing = requirements
        .iter()
        .filter(|r| r["status"] == "uncovered")
        .collect::<Vec<_>>();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0]["item"], "F10");
    for key in ["CANCELLED", "OUT"] {
        assert_eq!(
            requirements.iter().find(|r| r["item"] == key).unwrap()["status"],
            "not_required"
        );
    }
    assert_eq!(result["counts"]["covered"], 9);
    assert_eq!(result["counts"]["uncovered"], 1);
}

#[test]
fn unregistered_specification_and_unresolved_criterion_stay_unknown_despite_fresh_sources() {
    let mut unknown = reference("C90", "0.80");
    unknown["specification"] = json!("unregistered");
    let mut bad_criterion = reference("C90", "0.80");
    bad_criterion["criterion"] = json!("Not an acceptance criterion");
    let p = Project::new(
        json!([{"id":"C90","disposition":"required"}]),
        json!([
            {"id":"W","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[unknown,bad_criterion,reference("MISSING","0.80")]}
        ]),
    );
    let result = p.ok(&["spec", "check"]);
    assert_eq!(result["status"], "unknown");
    assert_eq!(result["references"].as_array().unwrap().len(), 3);
    assert!(
        result["references"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "unknown")
    );
    let unregistered = p.ok(&["spec", "check", "--spec", "unregistered"]);
    assert_eq!(unregistered["status"], "unknown");
    assert!(
        unregistered["diagnostics"]
            .as_array()
            .unwrap()
            .contains(&json!("specification_not_registered"))
    );
}

#[test]
fn changing_bytes_without_bumping_version_requires_review_and_stale_reads_write_nothing() {
    let p = Project::new(
        json!([{"id":"C90","disposition":"required"}]),
        json!([
            {"id":"W","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[reference("C90","0.80")]}
        ]),
    );
    assert_eq!(p.ok(&["spec", "check"])["status"], "matched");
    let path = p.0.join("decisions/dashboard.md");
    let mut bytes = fs::read_to_string(&path).unwrap();
    bytes.push_str("\nAn explicitly revised boundary.\n");
    fs::write(&path, bytes).unwrap();
    let before = p.logical_state();
    let stale = p.run(&["spec", "check"]);
    assert!(!stale.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&stale.stderr).unwrap()["code"],
        "SourceStale"
    );
    assert_eq!(p.logical_state(), before);
    p.ok(&["source", "reindex"]);
    let current = p.logical_state();
    let r = p.ok(&["spec", "check"]);
    assert_eq!(r["references"][0]["status"], "needs_review");
    assert_eq!(
        r["references"][0]["reasons"],
        json!(["approved_source_changed"])
    );
    assert_eq!(p.logical_state(), current);
}

#[test]
fn proposed_decisions_and_replacement_cycles_never_prove_coverage() {
    let p = Project::new(
        json!([{"id":"C90","disposition":"required"}]),
        json!([
            {"id":"W","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[reference("C90","0.80")]}
        ]),
    );
    p.spec(
        "proposed",
        "0.80",
        json!([{"id":"C90","disposition":"required"}]),
    );
    p.ok(&["source", "reindex"]);
    let r = p.ok(&["spec", "check"]);
    assert_eq!(r["status"], "unknown");
    assert_eq!(r["counts"]["covered"], 0);
    p.spec(
        "accepted",
        "0.80",
        json!([
            {"id":"C90","disposition":"superseded","superseded_by":"C64"},
            {"id":"C64","disposition":"superseded","superseded_by":"C90"}
        ]),
    );
    p.ok(&["source", "reindex"]);
    let r = p.ok(&["spec", "check"]);
    assert_eq!(r["status"], "unknown");
    assert!(
        r["references"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("replacement_cycle"))
    );
    assert!(
        r["requirements"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "unknown")
    );
}

#[test]
fn cancelled_work_does_not_cover_a_future_item_and_limit_never_silently_truncates() {
    let p = Project::new(
        json!([{"id":"C90","disposition":"future"}]),
        json!([
            {"id":"W","status":"cancelled","acceptance":["Save the reviewed configuration"],"spec_refs":[reference("C90","0.80")]}
        ]),
    );
    let r = p.ok(&["spec", "check"]);
    assert_eq!(r["counts"]["covered"], 0);
    assert_eq!(r["requirements"][0]["status"], "needs_review");
    let limited = p.run(&["spec", "check", "--limit", "1"]);
    assert!(!limited.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&limited.stderr).unwrap()["code"],
        "BudgetExceeded"
    );
}

#[test]
fn legacy_ledger_and_unregistered_files_do_not_become_specification_authority() {
    let p = Project::new(
        json!([{"id":"C90","disposition":"future"}]),
        json!([{"id":"W","status":"planned"}]),
    );
    fs::remove_file(p.0.join("decisions/dashboard.md")).unwrap();
    p.ok(&["source", "reindex"]);
    fs::write(
        p.0.join("unregistered.md"),
        "# Approved specification\nC90 is future.\n",
    )
    .unwrap();
    let before = p.logical_state();
    let r = p.ok(&["spec", "check"]);
    assert_eq!(r["status"], "unknown");
    assert_eq!(r["diagnostics"], json!(["no_explicit_specifications"]));
    assert!(r["requirements"].as_array().unwrap().is_empty());
    assert_eq!(p.logical_state(), before);
}

#[test]
fn malformed_contracts_are_rejected_without_promoting_previous_projection() {
    let p = Project::new(
        json!([{"id":"C90","disposition":"future"}]),
        json!([{"id":"W","status":"planned"}]),
    );
    for items in [
        json!([{"id":"C90","disposition":"future"},{"id":"C90","disposition":"cancelled"}]),
        json!([{"id":"C90","disposition":"private-invalid-value"}]),
        json!([{"id":"C90","disposition":"required","private-unknown-field":"private-invalid-value"}]),
    ] {
        p.spec("accepted", "0.80", items);
        let before = p.logical_state();
        let r = p.run(&["spec", "check"]);
        assert!(!r.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stderr).unwrap()["code"],
            "SourceStale"
        );
        assert!(!String::from_utf8_lossy(&r.stderr).contains("private-invalid-value"));
        assert_eq!(p.logical_state(), before);
    }
}

#[test]
fn adapter_upgrade_requires_explicit_reindex_and_restores_declared_fields() {
    let p = Project::new(
        json!([{"id":"C90","disposition":"future"}]),
        json!([{"id":"W","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[reference("C90","0.80")]}]),
    );
    let ledger = fs::read(p.0.join("work.yaml")).unwrap();
    let spec = fs::read(p.0.join("decisions/dashboard.md")).unwrap();
    // Simulate the previous adapters: identical source bytes, fields not projected.
    {
        let db = rusqlite::Connection::open(p.0.join(".awr/state.db")).unwrap();
        db.execute_batch("UPDATE sources SET config_json=json_set(config_json,'$.adapter_version',CASE adapter WHEN 'yaml-ledger-v1' THEN 4 ELSE 3 END); UPDATE work_items SET payload_json=json_remove(payload_json,'$.spec_refs'); UPDATE decisions SET payload_json=json_remove(payload_json,'$.specification');").unwrap();
    }
    let before = p.logical_state();
    let stale = p.run(&["spec", "check"]);
    assert!(!stale.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&stale.stderr).unwrap()["code"],
        "SourceStale"
    );
    assert_eq!(p.logical_state(), before);
    p.ok(&["source", "reindex"]);
    let result = p.ok(&["spec", "check"]);
    assert_eq!(result["status"], "matched");
    assert_eq!(result["counts"]["covered"], 1);
    assert_eq!(fs::read(p.0.join("work.yaml")).unwrap(), ledger);
    assert_eq!(fs::read(p.0.join("decisions/dashboard.md")).unwrap(), spec);
}

#[test]
fn reviewed_reference_edits_use_existing_batch_and_active_claim_guards() {
    let p = Project::new(
        json!([{"id":"C90","disposition":"future"}]),
        json!([{"id":"W","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[reference("C90","0.80")]}]),
    );
    let source = p.ok(&["source", "list"])["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["domain"] == "ledger")
        .unwrap()
        .clone();
    let request = json!({"version":1,"request_key":"remove-reviewed-reference","actor":{"host":"fixture","subject":"editor","origin":"ai_accepted"},"reason":"Remove the explicit association after review","change":{"kind":"ledger","source_id":source["id"],"source_fingerprint":source["fingerprint"],"operations":[{"operation":"fields","target":"W","fields":{"spec_refs":[]}}]}});
    fs::write(
        p.0.join("batch.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    let preview = p.ok(&["batch", "change", "--input", "batch.json"]);
    p.ok(&[
        "batch",
        "change",
        "--input",
        "batch.json",
        "--accept",
        "--expected-preview",
        preview["preview"]["fingerprint"].as_str().unwrap(),
        "--expected-revision",
        &preview["project_revision"].to_string(),
    ]);
    assert_eq!(p.ok(&["spec", "check"])["counts"]["uncovered"], 1);

    let rev = p.ok(&["session", "list"])["project_revision"].to_string();
    p.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--claim",
        "--agent",
        "fixture",
        "--provider",
        "synthetic",
        "--model",
        "fixture",
        "--expected-revision",
        &rev,
    ]);
    let source = p.ok(&["source", "list"])["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["domain"] == "ledger")
        .unwrap()
        .clone();
    let mut request = request;
    request["request_key"] = json!("restore-reviewed-reference");
    request["change"]["source_fingerprint"] = source["fingerprint"].clone();
    let mut restored = reference("C90", "0.80");
    restored["source_fingerprint"] = json!(format!(
        "sha256:{:x}",
        sha2::Sha256::digest(fs::read(p.0.join("decisions/dashboard.md")).unwrap())
    ));
    request["change"]["operations"][0]["fields"]["spec_refs"] = json!([restored]);
    fs::write(
        p.0.join("batch.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    let ledger = fs::read(p.0.join("work.yaml")).unwrap();
    let blocked = p.run(&["batch", "change", "--input", "batch.json"]);
    assert!(!blocked.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&blocked.stderr).unwrap()["code"],
        "ClaimConflict"
    );
    assert_eq!(fs::read(p.0.join("work.yaml")).unwrap(), ledger);
}

#[test]
fn adopted_history_is_traceable_without_current_gaps_but_live_old_refs_need_review() {
    let p = Project::new(json!([{"id":"R","disposition":"required"}]), json!([]));
    let mut old_ref = reference("R", "0.80");
    old_ref["source_fingerprint"] = p.version("dashboard")["source_fingerprint"].clone();
    p.document("new", json!({"id":"new","status":"proposed","specification":{"version":"1","items":[{"id":"R","disposition":"required"}]}}));
    p.ok(&["source", "reindex"]);
    let before_adoption = p.version("new");
    p.adopt("dashboard", "new");
    assert_ne!(
        p.version("new")["source_fingerprint"],
        before_adoption["source_fingerprint"]
    );
    let new_ref = json!({"specification":"new","item":"R","approved_version":"1","source_fingerprint":p.version("new")["source_fingerprint"],"criterion":"Save the reviewed configuration"});
    let mut works = json!([
        {"id":"NEW","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[new_ref]},
        {"id":"CANCELLED","status":"cancelled","acceptance":["Save the reviewed configuration"],"spec_refs":[old_ref]},
        {"id":"ARCHIVED","status":"completed","archived":true,"acceptance":["Save the reviewed configuration"],"spec_refs":[old_ref]}
    ]);
    p.works(works.clone());
    let bytes = fs::read(p.0.join("work.yaml")).unwrap();
    let old = fs::read(p.0.join("decisions/dashboard.md")).unwrap();
    let state = p.logical_state();
    let r = p.ok(&["spec", "check"]);
    assert_eq!(r["status"], "matched", "{r}");
    assert_eq!(r["counts"]["covered"], 1);
    assert_eq!(r["requirements"].as_array().unwrap().len(), 1);
    assert_eq!(
        r["specifications"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["key"] == "dashboard")
            .unwrap()["status"],
        "historical"
    );
    let human = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args([
            "--project",
            p.0.to_str().unwrap(),
            "spec",
            "check",
            "--spec",
            "dashboard",
        ])
        .output()
        .unwrap();
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("historical"));
    assert_eq!(fs::read(p.0.join("work.yaml")).unwrap(), bytes);
    assert_eq!(fs::read(p.0.join("decisions/dashboard.md")).unwrap(), old);
    assert_eq!(p.logical_state(), state);
    works.as_array_mut().unwrap().push(json!({"id":"LIVE","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[old_ref]}));
    p.works(works);
    let r = p.ok(&["spec", "check"]);
    assert_eq!(r["status"], "needs_review");
    let live = r["references"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["work"] == "LIVE")
        .unwrap();
    assert_eq!(live["status"], "needs_review");
    assert!(
        live["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("specification_superseded"))
    );
}

#[test]
fn document_replacement_targets_must_resolve_identity_version_and_approval() {
    for case in [
        "missing",
        "identity",
        "version",
        "approval",
        "declaration",
        "cycle",
        "status_conflict",
    ] {
        let p = Project::new(json!([{"id":"R","disposition":"required"}]), json!([]));
        p.document("new", json!({"id":"new","status":if case=="approval" {"proposed"}else{"accepted"},"specification":{"version":"1","items":[{"id":"R","disposition":"required"}]}}));
        if case == "cycle" {
            p.document("new", json!({"id":"new","status":"superseded","superseded_by":p.version("dashboard"),"specification":{"version":"1","items":[{"id":"R","disposition":"required"}]}}));
        } else if case == "declaration" {
            p.document("new", json!({"id":"new","status":"accepted"}));
        } else if case == "status_conflict" {
            p.document("new", json!({"id":"new","status":"accepted","superseded_by":p.version("dashboard"),"specification":{"version":"1","items":[{"id":"R","disposition":"required"}]}}));
        }
        p.ok(&["source", "reindex"]);
        let mut target = p.version("new");
        match case {
            "missing" => target["external_key"] = json!("missing"),
            "identity" => target["source_id"] = json!(Id::new()),
            "version" => target["source_fingerprint"] = json!(format!("sha256:{}", "b".repeat(64))),
            _ => (),
        }
        p.document("dashboard", json!({"id":"dashboard","status":"superseded","superseded_by":target,"specification":{"version":"0.80","items":[{"id":"R","disposition":"required"}]}}));
        p.ok(&["source", "reindex"]);
        let mut r = reference("R", "0.80");
        r["source_fingerprint"] = p.version("dashboard")["source_fingerprint"].clone();
        p.works(json!([{"id":"W","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[r]}]));
        let state = p.logical_state();
        let result = p.ok(&["spec", "check"]);
        assert_eq!(
            result["references"][0]["status"], "unknown",
            "{case}: {result}"
        );
        assert!(
            result["references"][0]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap().starts_with("document_replacement_")),
            "{case}: {result}"
        );
        assert_eq!(p.logical_state(), state);
    }
}

#[test]
fn long_spec_refs_stay_out_of_queue_briefs_and_remain_in_explicit_details() {
    let criterion = "Reviewed requirement ".to_owned() + &"x".repeat(4000);
    let items = (0..100)
        .map(|i| json!({"id":format!("R{i}"),"disposition":"required"}))
        .collect::<Vec<_>>();
    let refs = (0..100)
        .map(|i| {
            let mut r = reference(&format!("R{i}"), "0.80");
            r["criterion"] = json!(criterion);
            r
        })
        .collect::<Vec<_>>();
    let p = Project::new(
        json!(items),
        json!([{"id":"W","title":"Review configuration","status":"ready","next_action":"Review","acceptance":[criterion],"spec_refs":refs}]),
    );
    let queue = p.ok(&["ready", "--limit", "1"]);
    assert_eq!(queue["ready"][0]["spec_refs_count"], 100);
    assert!(queue["ready"][0].get("spec_refs").is_none());
    assert!(serde_json::to_vec(&queue).unwrap().len() < 16000);
    let status = p.ok(&["status", "--view", "full"]);
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("\"spec_refs\":")
    );
    assert!(serde_json::to_vec(&status).unwrap().len() < 16000);
    let mut ledger: Value =
        serde_json::from_slice(&fs::read(p.0.join("work.yaml")).unwrap()).unwrap();
    ledger["work_items"][0]["status"] = json!("in_progress");
    p.works(ledger["work_items"].clone());
    let status = p.ok(&["status", "--view", "full"]);
    assert_eq!(status["current"][0]["spec_refs_count"], 100);
    assert!(status["current"][0].get("spec_refs").is_none());
    assert!(serde_json::to_vec(&status).unwrap().len() < 16000);
    let detail = p.ok(&["work", "show", "W"]);
    assert_eq!(detail["work"]["spec_refs"].as_array().unwrap().len(), 100);
    assert_eq!(p.ok(&["spec", "check"])["counts"]["covered"], 100);
}

#[test]
fn multi_step_adoption_resolves_history_and_invalidated_receipts_remain_unknown() {
    let p = Project::new(json!([{"id":"R","disposition":"required"}]), json!([]));
    p.document("second", json!({"id":"second","status":"proposed","specification":{"version":"1","items":[{"id":"R","disposition":"required"}]}}));
    p.ok(&["source", "reindex"]);
    p.adopt("dashboard", "second");
    p.document("latest", json!({"id":"latest","status":"proposed","specification":{"version":"2","items":[{"id":"R","disposition":"required"}]}}));
    p.ok(&["source", "reindex"]);
    p.adopt("second", "latest");
    p.works(json!([{"id":"W","status":"planned","acceptance":["Save the reviewed configuration"],"spec_refs":[{"specification":"latest","item":"R","approved_version":"2","source_fingerprint":p.version("latest")["source_fingerprint"]}]}]));
    let state = p.logical_state();
    let r = p.ok(&["spec", "check", "--limit", "5"]);
    assert_eq!(r["status"], "matched", "{r}");
    assert_eq!(r["requirements"].as_array().unwrap().len(), 1);
    assert_eq!(
        r["specifications"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["status"] == "historical")
            .count(),
        2
    );
    assert_eq!(p.logical_state(), state);
    let path = p.0.join("decisions/second.md");
    let mut bytes = fs::read_to_string(&path).unwrap();
    bytes.push_str("\nContent changed after supersession.\n");
    fs::write(path, bytes).unwrap();
    p.ok(&["source", "reindex"]);
    let r = p.ok(&["spec", "check"]);
    assert_eq!(r["status"], "unknown");
    assert_eq!(
        r["specifications"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["key"] == "second")
            .unwrap()["diagnostic"],
        "document_replacement_adoption_unverified"
    );
    assert_eq!(
        p.ok(&["spec", "check", "--spec", "latest"])["status"],
        "matched"
    );
}

#[test]
fn extraction_warnings_preserve_valid_history_but_invalid_adoptions_stay_unknown() {
    for invalidation in ["content", "identity", "receipt_version"] {
        let p = Project::new(json!([{"id":"R","disposition":"required"}]), json!([]));
        p.document("second", json!({"id":"second","status":"proposed","specification":{"version":"1","items":[{"id":"R","disposition":"required"}]}}));
        let path = p.0.join("decisions/second.md");
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("## Decision", "## Requirements");
        fs::write(&path, text).unwrap();
        p.ok(&["source", "reindex"]);
        p.adopt("dashboard", "second");
        p.document("latest", json!({"id":"latest","status":"proposed","specification":{"version":"2","items":[{"id":"R","disposition":"required"}]}}));
        p.ok(&["source", "reindex"]);
        p.adopt("second", "latest");
        p.works(json!([{"id":"W","status":"planned","spec_refs":[{"specification":"latest","item":"R","approved_version":"2","source_fingerprint":p.version("latest")["source_fingerprint"]}]}]));
        let bytes = fs::read(&path).unwrap();
        let ledger = fs::read(p.0.join("work.yaml")).unwrap();
        let state = p.logical_state();
        let r = p.ok(&["spec", "check", "--limit", "5"]);
        assert_eq!(r["status"], "matched", "{invalidation}: {r}");
        assert_eq!(r["counts"]["covered"], 1);
        assert_eq!(r["counts"]["unknown"], 0);
        assert_eq!(
            r["specifications"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["status"] == "historical")
                .count(),
            2
        );
        let db = rusqlite::Connection::open(p.0.join(".awr/state.db")).unwrap();
        let warning: String = db.query_row("SELECT json_extract(payload_json,'$.warnings') FROM events WHERE event_type='source.projected' AND json_extract(payload_json,'$.source_id')=?1 ORDER BY project_revision DESC LIMIT 1", [p.version("second")["source_id"].as_str().unwrap()], |r| r.get(0)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&warning).unwrap(),
            json!(["no decision statement identified; consult the source reference"])
        );
        drop(db);
        assert_eq!(p.logical_state(), state);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read(p.0.join("work.yaml")).unwrap(), ledger);

        if invalidation == "content" {
            // Previous projections have no structured receipt validity; refresh explicitly.
            let db = rusqlite::Connection::open(p.0.join(".awr/state.db")).unwrap();
            db.execute_batch("UPDATE sources SET config_json=json_set(config_json,'$.adapter_version',4) WHERE adapter='markdown-directory-v1'; UPDATE decisions SET payload_json=json_remove(payload_json,'$.adoption_valid');").unwrap();
            drop(db);
            let state = p.logical_state();
            let stale = p.run(&["spec", "check"]);
            assert!(!stale.status.success());
            assert_eq!(
                serde_json::from_slice::<Value>(&stale.stderr).unwrap()["code"],
                "SourceStale"
            );
            assert_eq!(p.logical_state(), state);
            p.ok(&["source", "reindex"]);
            assert_eq!(
                p.ok(&["spec", "check", "--limit", "5"])["status"],
                "matched"
            );
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }

        let text = fs::read_to_string(&path).unwrap();
        let (header, body) = text
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap();
        let mut metadata: Value = serde_yaml_ng::from_str(header).unwrap();
        let body = match invalidation {
            "content" => format!("{body}\nChanged reviewed requirements.\n"),
            "identity" => {
                metadata["adoption"]["candidate"]["source_id"] = json!(Id::new());
                body.to_owned()
            }
            "receipt_version" => {
                metadata["adoption"]["version"] = json!(2);
                body.to_owned()
            }
            _ => unreachable!(),
        };
        fs::write(
            &path,
            format!(
                "---\n{}\n---\n{body}",
                serde_json::to_string_pretty(&metadata).unwrap()
            ),
        )
        .unwrap();
        p.ok(&["source", "reindex"]);
        let state = p.logical_state();
        let r = p.ok(&["spec", "check"]);
        assert_eq!(r["status"], "unknown", "{invalidation}: {r}");
        assert_eq!(
            r["specifications"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["key"] == "second")
                .unwrap()["diagnostic"],
            "document_replacement_adoption_unverified",
            "{invalidation}: {r}"
        );
        assert_eq!(
            p.ok(&["spec", "check", "--spec", "latest"])["status"],
            "matched"
        );
        assert_eq!(p.logical_state(), state);
    }
}
