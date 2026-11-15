use awr_core::Id;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture {
    root: PathBuf,
    id: Id,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-usage-cli-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join(".awr/project.toml"), "[project]\nname='Usage example'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        fs::write(
            root.join("work.yaml"),
            "work_items:\n- id: W\n  title: Prepare a report\n  status: ready\n",
        )
        .unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let indexed =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        Self {
            root,
            id: indexed.project_id,
        }
    }

    fn revision(&self) -> u64 {
        Store::open_readonly(&self.root.join(".awr/state.db"))
            .unwrap()
            .project(self.id)
            .unwrap()
            .project_revision
    }

    fn call(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .env("AWR_LOCAL_WEB", "0")
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.call(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn record(&self, key: &str, record: Value) -> Value {
        fs::write(
            self.root.join("usage-input.json"),
            serde_json::to_vec(&json!({
                "expected_revision":self.revision(),"request_key":key,"record":record
            }))
            .unwrap(),
        )
        .unwrap();
        self.ok(&["usage", "record", "--input", "usage-input.json"])
    }

    fn receipt(&self, id: &str, cost: Value) -> Value {
        json!({"kind":"receipt","data":{"receipt_id":id,"project_id":self.id,
            "provider_namespace":"fixture-account","provider":"fixture-provider","call_id":id,
            "model":"fixture-model","session_id":"fixture-session","occurred_at_ms":10,"channel":"model",
            "attribution":{"work_id":"W","execution_id":"E","workstream_id":null},
            "tokens":{"input":100,"output":20,"cached_input":10},"cost":cost}})
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn usage_cli_records_replays_adjustments_and_reports_separate_cost_time_and_coverage() {
    let f = Fixture::new();
    let empty = f.ok(&["usage", "summary"]);
    assert!(empty["data"]["time"].is_null());
    assert!(empty["data"]["coverage"]["expected_calls"].is_null());
    let actual = json!({"actual":{"currency":"USD","micros":100}});
    f.record("actual", f.receipt("actual-call", actual.clone()));
    let bytes = fs::read(f.root.join("usage-input.json")).unwrap();
    let revision = f.revision();
    let replay = f.ok(&["usage", "record", "--input", "usage-input.json"]);
    assert_eq!(replay["receipt"]["replayed"], true);
    assert_eq!(f.revision(), revision);
    assert_eq!(fs::read(f.root.join("usage-input.json")).unwrap(), bytes);
    f.record(
        "estimate",
        f.receipt(
            "estimated-call",
            json!({"api_equivalent_estimate":{
        "amount":{"currency":"EUR","micros":500},"pricing_version":"fixture-price-v1"}}),
        ),
    );
    f.record("unknown", f.receipt("unpriced-call", json!("unknown")));
    f.record(
        "correction",
        json!({"kind":"correction","data":{"correction_id":"adjustment",
        "project_id":f.id,"target_receipt_id":"actual-call","corrected_at_ms":20,
        "reason":"Correct the supplied amount","prior_cost":actual,
        "new_cost":{"actual":{"currency":"USD","micros":80}},"actor":"fixture-owner"}}),
    );
    f.record("allocation", json!({"kind":"allocation","data":{"allocation_id":"split",
        "project_id":f.id,"receipt_id":"actual-call","recorded_at_ms":21,"rule":"Recorded shared effort",
        "shares":[{"workstream_id":null,"micros":80}]}}));
    f.record("counter", json!({"kind":"counter","data":{"scope":{"project_id":f.id,
        "provider_namespace":"fixture-account","provider":"fixture-provider","model":"fixture-model",
        "session_id":"fixture-session","counter_epoch":"first"},"observed_at_ms":20,
        "tokens":{"input":300,"output":60,"cached_input":30}}}));
    for (key, execution, start, end) in [
        ("first-interval", "E", 10, 30),
        ("second-interval", "E2", 20, 40),
    ] {
        f.record(key, json!({"kind":"interval","data":{"execution_id":execution,"interval":{"start_ms":start,"end_ms":end}}}));
    }
    let revision = f.revision();
    let summary = f.ok(&["usage", "summary"]);
    assert_eq!(summary["data"]["costs"]["actual_micros"]["USD"], 80);
    assert_eq!(
        summary["data"]["costs"]["api_equivalent_micros"]["EUR"],
        500
    );
    assert_eq!(summary["data"]["costs"]["unknown_calls"], 1);
    assert_eq!(summary["data"]["costs"]["unique_calls"], 3);
    assert_eq!(summary["data"]["corrections"]["ids"], json!(["adjustment"]));
    assert_eq!(summary["data"]["corrections"]["total"], 1);
    assert_eq!(summary["data"]["time"]["observed_wall_clock_ms"], 30);
    assert_eq!(summary["data"]["time"]["observed_execution_ms"], 40);
    assert_eq!(
        f.ok(&["usage", "summary", "--uncorrected"])["data"]["costs"]["actual_micros"]["USD"],
        100
    );
    let page = f.ok(&["usage", "list", "--limit", "1"]);
    assert_eq!(page["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["data"]["total"], 3);
    assert_eq!(page["data"]["next_offset"], 1);
    for kind in [
        "corrections",
        "allocations",
        "counters",
        "intervals",
        "bindings",
    ] {
        assert!(
            !f.ok(&["usage", "list", "--kind", kind])["data"]["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    fs::write(
        f.root.join("usage-query.json"),
        br#"{"kind":"observation","expected_calls":4,"expected_time_ms":50}"#,
    )
    .unwrap();
    let observed = f.ok(&["usage", "query", "--input", "usage-query.json"]);
    assert_eq!(observed["data"]["coverage"]["call_coverage"], json!([3, 4]));
    assert_eq!(
        observed["data"]["coverage"]["time_coverage"],
        json!([30, 50])
    );
    assert_eq!(f.revision(), revision);
}

#[test]
fn usage_cli_refuses_foreign_records_and_malformed_queries_without_ledger_changes() {
    let f = Fixture::new();
    let mut record = f.receipt("foreign", json!("unknown"));
    record["data"]["project_id"] = json!(Id::new());
    fs::write(
        f.root.join("usage-input.json"),
        serde_json::to_vec(&json!({"expected_revision":f.revision(),
        "request_key":"foreign","record":record}))
        .unwrap(),
    )
    .unwrap();
    let revision = f.revision();
    assert!(
        !f.call(&["usage", "record", "--input", "usage-input.json"])
            .status
            .success()
    );
    for query in [
        json!({"kind":"receipts","limit":0}),
        json!({"kind":"summary","unexpected":true}),
    ] {
        fs::write(
            f.root.join("usage-query.json"),
            serde_json::to_vec(&query).unwrap(),
        )
        .unwrap();
        assert!(
            !f.call(&["usage", "query", "--input", "usage-query.json"])
                .status
                .success()
        );
    }
    assert_eq!(f.revision(), revision);
    assert_eq!(
        f.ok(&["usage", "summary"])["data"]["costs"]["unique_calls"],
        0
    );
}
