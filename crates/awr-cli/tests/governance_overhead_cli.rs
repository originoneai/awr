//! What governance costs a clear small task, measured with the shipped binary against an equivalent
//! baseline: the same work taken through the same steps without a policy. Only what can be
//! counted is counted: calls of the CLI and the bytes they return. Model tokens and money are not
//! measured here and nothing is inferred about them.
//!
//! The targets are the ones adopted for governed work: at most one extra call, at most 2 KiB of
//! guidance for a clear small task, and a median extra-call ratio of at most 0.2 over the flows.
//! A flow that goes over a target fails this check, which is what blocks adopting governance for a
//! project until the path is simplified or the overhead is accepted by the user. Run it with
//! `--nocapture` to read the report.
use awr_core::Id;
use serde_json::{Value, json};
use sha2::Digest;
use std::{fs, path::PathBuf, process::Command};

const TARGET_EXTRA_CALLS: usize = 1;
const TARGET_GUIDANCE_BYTES: usize = 2048;
const TARGET_MEDIAN_RATIO: f64 = 0.2;

const ACC: &str = "Saving failure keeps the user's input";
const OUTCOME: &str = "Users keep their input when saving fails";

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What one call of the CLI returned.
struct Call {
    name: &'static str,
    bytes: usize,
}

struct Flow {
    calls: Vec<Call>,
}

impl Flow {
    fn bytes(&self) -> usize {
        self.calls.iter().map(|c| c.bytes).sum()
    }
}

fn definition(purpose: &str, pin: &str) -> Value {
    let mut definition = json!({
        "version": 1, "purpose": purpose,
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": pin}],
        "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge", "by": "agent-a"},
        "deliverable": "Local save with a failure fallback"
    });
    if purpose == "implementation" {
        definition["tier"] = json!("required");
        definition["responsibility"] = json!({"name": "storage-module", "kind": "delivery"});
        definition["evidence_method"] =
            json!("Targeted tests compared before and after the change");
        definition["failure_recovery"] = json!("A failed save keeps the previous data");
        definition["examples"] = json!([
            {"criterion": ACC, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
            {"criterion": ACC, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"}
        ]);
        definition["discrimination"] = json!({"method": "fail_before_pass_after"});
    }
    definition
}

impl Project {
    fn new(purpose: &str, governed: bool) -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-overhead-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        let policy = if governed {
            "[sources.options.admission_policy]\nversion=1\nmode='governed_enforce'\nscope='all'\n"
        } else {
            ""
        };
        fs::write(
            p.0.join("mapping.toml"),
            format!("[project]\nname='Overhead fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n{policy}[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"),
        )
        .unwrap();
        fs::write(
            p.0.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        let pin = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(fs::read(p.0.join("decisions/scope.md")).unwrap())
        );
        let ledger = json!({
            "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
            "work_items": [{"id": "W", "title": "Save safely", "status": "planned", "goal": "G",
                "acceptance": [ACC], "paths": ["crates/storage/src"], "tags": [], "next_action": "Do the work",
                "definition": definition(purpose, &pin)}]
        });
        fs::write(
            p.0.join("work.yaml"),
            serde_json::to_vec_pretty(&ledger).unwrap(),
        )
        .unwrap();
        p.raw(&["init", "--manifest", "mapping.toml", "--accept"]);
        p
    }

    fn raw(&self, args: &[&str]) -> (Value, usize) {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (
            serde_json::from_slice(&output.stdout).unwrap(),
            output.stdout.len(),
        )
    }

    /// The revision every response is computed at. A harness reads it for bookkeeping; an agent
    /// gets it from the response before, so it is not counted as a call.
    fn revision(&self) -> String {
        self.raw(&["status"]).0["project_revision"].to_string()
    }

    fn call(&self, flow: &mut Flow, name: &'static str, args: &[&str]) -> Value {
        let (value, bytes) = self.raw(args);
        flow.calls.push(Call { name, bytes });
        value
    }
}

/// Takes a clear small task from reading its context to the end of its session, with the one
/// step governance adds.
fn run(purpose: &str, governed: bool) -> (Flow, usize) {
    let p = Project::new(purpose, governed);
    let mut flow = Flow { calls: vec![] };
    let prepared = p.call(
        &mut flow,
        "work prepare",
        &[
            "work",
            "prepare",
            "W",
            "--budget",
            "12000",
            "--response-view",
            "action",
        ],
    );
    let guidance_bytes = serde_json::to_vec(&prepared["guidance"]).unwrap().len();
    if governed {
        let request = json!({"work": "W", "agent": "claude-run", "expected_revision": p.revision().parse::<u64>().unwrap(),
            "request_key": "admit-1", "decision": "approved",
            "contract_fingerprint": prepared["management"]["contract_fingerprint"],
            "judgment": {"contribution": "Advances the saving outcome", "necessity": "The draft is lost today",
                         "alternatives": "Reuse of the existing buffer was checked", "deliverable": "A fallback path",
                         "exit": "Stop when the fallback is verified", "responsibility": "storage-module"}});
        let path = p.0.join("admit.json");
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        p.call(
            &mut flow,
            "work admit",
            &["work", "admit", "--input", path.to_str().unwrap()],
        );
    }
    let revision = p.revision();
    let started = p.call(
        &mut flow,
        "session start",
        &[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "claude-run",
            "--provider",
            "test",
            "--model",
            "m",
            "--claim",
            "--expected-revision",
            &revision,
        ],
    );
    let session = started["session"]["id"].as_str().unwrap().to_string();
    let revision = p.revision();
    p.call(
        &mut flow,
        "work progress",
        &[
            "work",
            "progress",
            "W",
            "--session",
            &session,
            "--reason",
            "Starting the work",
            "--next-action",
            "Write the fallback",
            "--expected-revision",
            &revision,
        ],
    );
    // The report the work produced; writing it is the work, not a call.
    let sha = "a".repeat(40);
    let verified_at = awr_core::now_millis().unwrap();
    let report = p.0.join("report.json");
    fs::write(
        &report,
        serde_json::to_vec(&json!({"version": 1, "work_item": "W", "source_sha": sha, "command": "cargo test",
            "scope": ["W"], "verified_at": verified_at,
            "checks": [{"name": "saving", "passed": true, "criteria": [ACC], "details": "The tests pass"}]}))
        .unwrap(),
    )
    .unwrap();
    let evidence = json!({"work_item_key": "W", "external_key": "unit-1", "evidence_type": "unit_test",
        "level": "locally_verified", "summary": "The tests pass", "locator": report.to_str().unwrap(),
        "sha256": format!("{:x}", sha2::Sha256::digest(fs::read(&report).unwrap())),
        "source_sha": sha, "command": "cargo test", "scope": ["W"], "verified_at": verified_at});
    let path = p.0.join("evidence.json");
    fs::write(&path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    let revision = p.revision();
    p.call(
        &mut flow,
        "evidence add",
        &[
            "evidence",
            "add",
            "--input",
            path.to_str().unwrap(),
            "--expected-revision",
            &revision,
        ],
    );
    let completion = json!({"version": 1, "source_sha": sha, "minimum_level": "locally_verified",
        "required_evidence": ["unit-1"], "acceptance": [{"criterion": ACC, "evidence": ["unit-1"]}]});
    let path = p.0.join("completion.json");
    fs::write(&path, serde_json::to_vec(&completion).unwrap()).unwrap();
    let revision = p.revision();
    p.call(
        &mut flow,
        "work complete",
        &[
            "work",
            "complete",
            "W",
            "--session",
            &session,
            "--reason",
            "The tests pass",
            "--input",
            path.to_str().unwrap(),
            "--expected-revision",
            &revision,
        ],
    );
    let status = p.raw(&["work", "show", "W"]).0["work"]["status"].clone();
    assert_eq!(
        status, "completed",
        "the task was delivered, not only measured"
    );
    let revision = p.revision();
    p.call(
        &mut flow,
        "session end",
        &[
            "session",
            "end",
            "--session",
            &session,
            "--expected-revision",
            &revision,
        ],
    );
    (flow, guidance_bytes)
}

/// One flow measured against its baseline.
struct Measured {
    purpose: String,
    baseline_calls: usize,
    governed_calls: usize,
    extra_calls: usize,
    ratio: f64,
    extra_bytes: i64,
    guidance_bytes: usize,
    extra_steps: Vec<&'static str>,
}

fn measure(purpose: &str) -> Measured {
    let (baseline, _) = run(purpose, false);
    let (governed, guidance_bytes) = run(purpose, true);
    let baseline_names: Vec<&str> = baseline.calls.iter().map(|c| c.name).collect();
    let extra_steps: Vec<&'static str> = governed
        .calls
        .iter()
        .map(|c| c.name)
        .filter(|name| !baseline_names.contains(name))
        .collect();
    let extra_calls = governed.calls.len() - baseline.calls.len();
    Measured {
        purpose: purpose.to_string(),
        baseline_calls: baseline.calls.len(),
        governed_calls: governed.calls.len(),
        extra_calls,
        ratio: extra_calls as f64 / baseline.calls.len() as f64,
        extra_bytes: governed.bytes() as i64 - baseline.bytes() as i64,
        guidance_bytes,
        extra_steps,
    }
}

/// Reports every target a set of flows goes over; nothing when all are met.
fn over_target(flows: &[Measured]) -> Vec<String> {
    let mut over = Vec::new();
    for flow in flows {
        if flow.extra_calls > TARGET_EXTRA_CALLS {
            over.push(format!(
                "{}: {} extra calls against a target of {TARGET_EXTRA_CALLS} ({})",
                flow.purpose,
                flow.extra_calls,
                flow.extra_steps.join(", ")
            ));
        }
        if flow.guidance_bytes > TARGET_GUIDANCE_BYTES {
            over.push(format!(
                "{}: {} bytes of guidance against a target of {TARGET_GUIDANCE_BYTES}",
                flow.purpose, flow.guidance_bytes
            ));
        }
    }
    let mut ratios: Vec<f64> = flows.iter().map(|f| f.ratio).collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = ratios[ratios.len() / 2];
    if median > TARGET_MEDIAN_RATIO {
        over.push(format!(
            "median extra-call ratio {median:.3} against a target of {TARGET_MEDIAN_RATIO}"
        ));
    }
    over
}

#[test]
fn a_clear_small_task_costs_one_extra_call_and_a_bounded_instruction_on_every_purpose() {
    let flows: Vec<Measured> = ["implementation", "research", "document"]
        .into_iter()
        .map(measure)
        .collect();
    let report: Vec<Value> = flows
        .iter()
        .map(|f| {
            json!({"purpose": f.purpose, "baseline_calls": f.baseline_calls, "governed_calls": f.governed_calls,
                   "extra_calls": f.extra_calls, "extra_call_ratio": (f.ratio * 1000.0).round() / 1000.0,
                   "extra_bytes": f.extra_bytes, "guidance_bytes": f.guidance_bytes, "extra_steps": f.extra_steps})
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "measured": "calls of the CLI and the bytes they return, governed against the same steps without a policy",
            "not_measured": ["model tokens", "cost", "wall-clock time"],
            "targets": {"extra_calls_max": TARGET_EXTRA_CALLS, "guidance_bytes_max": TARGET_GUIDANCE_BYTES,
                        "median_extra_call_ratio_max": TARGET_MEDIAN_RATIO},
            "flows": report,
        }))
        .unwrap()
    );
    assert_eq!(over_target(&flows), Vec::<String>::new());
    for flow in &flows {
        // The one extra call is the admission, and it is the only step governance adds.
        assert_eq!(flow.extra_steps, ["work admit"], "{}", flow.purpose);
        assert!(
            flow.baseline_calls >= 5,
            "the baseline is a real flow: {}",
            flow.purpose
        );
    }
}

fn fake(purpose: &str, baseline: usize, extra: usize, guidance: usize) -> Measured {
    Measured {
        purpose: purpose.into(),
        baseline_calls: baseline,
        governed_calls: baseline + extra,
        extra_calls: extra,
        ratio: extra as f64 / baseline as f64,
        extra_bytes: 0,
        guidance_bytes: guidance,
        extra_steps: vec!["work define", "work review", "work admit"][..extra.min(3)].to_vec(),
    }
}

#[test]
fn a_task_that_needs_two_extra_calls_for_definition_review_and_admission_fails_a_one_call_target() {
    // Within the targets.
    assert!(
        over_target(&[
            fake("a", 6, 1, 900),
            fake("b", 6, 1, 900),
            fake("c", 6, 1, 900)
        ])
        .is_empty()
    );
    // Two extra calls fail the one-call target and the median ratio, and say which steps they were.
    let over = over_target(&[
        fake("small", 6, 2, 900),
        fake("b", 6, 1, 900),
        fake("c", 6, 2, 900),
    ]);
    assert!(
        over.iter()
            .any(|o| o.contains("2 extra calls against a target of 1")),
        "{over:?}"
    );
    assert!(
        over.iter().any(|o| o.contains("median extra-call ratio")),
        "{over:?}"
    );
    assert!(over.iter().any(|o| o.contains("work define, work review")));
    // Guidance over 2 KiB fails on its own.
    let over = over_target(&[
        fake("a", 6, 1, 4096),
        fake("b", 6, 1, 900),
        fake("c", 6, 1, 900),
    ]);
    assert_eq!(over.len(), 1, "{over:?}");
    assert!(over[0].contains("4096 bytes of guidance"));
    // One flow over the median ratio is not enough to fail it: the median is the target.
    assert!(
        over_target(&[
            fake("a", 2, 1, 900),
            fake("b", 8, 1, 900),
            fake("c", 8, 1, 900)
        ])
        .is_empty()
    );
    // Two flows over it are: now the median itself is over.
    let over = over_target(&[
        fake("a", 2, 1, 900),
        fake("b", 3, 1, 900),
        fake("c", 8, 1, 900),
    ]);
    assert_eq!(over.len(), 1, "{over:?}");
    assert!(over[0].contains("median extra-call ratio"), "{over:?}");
}
