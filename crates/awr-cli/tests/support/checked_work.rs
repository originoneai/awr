//! Synthetic fixtures shared by the text-budget and JSON governance measurements.
#![allow(dead_code)]

use awr_core::Id;
use awr_store::Store;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

pub const CRITERION: &str = "The draft survives a failed save";

#[derive(Clone, Debug)]
pub struct Call {
    pub name: String,
    pub bytes: usize,
}

#[derive(Clone, Default, Debug)]
pub struct Flow {
    pub calls: Vec<Call>,
}

impl Flow {
    pub fn bytes(&self) -> usize {
        self.calls.iter().map(|call| call.bytes).sum()
    }

    pub fn budget(&self, name: &str, calls: usize, bytes: usize) -> Result<(), String> {
        let steps = self
            .calls
            .iter()
            .map(|call| format!("{}: {} bytes", call.name, call.bytes))
            .collect::<Vec<_>>()
            .join("\n");
        let report = format!(
            "{name}: {} calls (limit {calls}), {} bytes (limit {bytes})\n{steps}",
            self.calls.len(),
            self.bytes()
        );
        println!("{report}");
        if self.calls.len() > calls || self.bytes() > bytes {
            Err(report)
        } else {
            Ok(())
        }
    }
}

pub struct Project {
    pub root: PathBuf,
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Project {
    pub fn new(checked: bool) -> Self {
        Self::with_status(checked, "planned")
    }

    pub fn with_status(checked: bool, status: &str) -> Self {
        let p = Self {
            root: std::env::temp_dir().join(format!("awr-path-{}", Id::new())),
        };
        fs::create_dir_all(p.root.join("decisions")).unwrap();
        let decision = "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nRetain the local draft when saving fails.\n";
        p.write("decisions/scope.md", decision.as_bytes());
        p.write("oracle.txt", b"Draft: retained\nStatus: save failed\n");
        p.write("actual.txt", b"Draft: missing\nStatus: success\n");
        let mut work = json!({"id":"W","title":"Retain the draft","status":status,"goal":"G",
            "acceptance":[CRITERION],"paths":["actual.txt"],"next_action":"Implement draft retention"});
        if checked {
            let examples: Vec<Value> = ["positive", "negative"].into_iter().map(|kind| json!({
                "id":format!("retain-{kind}"),"criterion":CRITERION,"kind":kind,
                "given":"a failed save","when":"the result is inspected",
                "then":if kind == "positive" {"the draft is retained"} else {"success is withheld"},
                "check":{"run":["git","diff","--no-index","--exit-code","--","oracle.txt","actual.txt"],"files":["oracle.txt"],"timeout_seconds":10}
            })).collect();
            work["definition"] = json!({"version":1,"purpose":"implementation","tier":"required","size":"small",
                "intent":[{"role":"user_decision","decision":"scope","source_fingerprint":format!("sha256:{:x}",Sha256::digest(decision))}],
                "contributes":[{"goal":"G","outcome":CRITERION,"claim":"Keep the draft available","by":"author"}],
                "granularity":{"judgment":"independent_result","reason":"One local saving outcome","by":"author"},
                "responsibility":{"name":"draft-storage","kind":"delivery"},"deliverable":"Draft retention",
                "evidence_method":"Run the same checks before and after implementation",
                "failure_recovery":"Keep the draft for another attempt","examples":examples,
                "discrimination":{"method":"fail_before_pass_after"}});
        }
        p.write_json("work.yaml", &json!({
            "goals":[{"id":"G","title":"Retain drafts","summary":CRITERION,"status":"active","success_criteria":[CRITERION]}],
            "work_items":[work]
        }));
        let policy = if checked {
            "[sources.options.admission_policy]\nversion=1\nmode='governed_enforce'\nscope='all'\n"
        } else {
            ""
        };
        let decisions = if checked {
            "[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"
        } else {
            ""
        };
        p.write("mapping.toml", format!("[project]\nname='Path fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n{policy}{decisions}").as_bytes());
        p.write(".gitignore", b".awr/\nactual.txt\n");
        p.git(&["init"]);
        p.git(&["config", "user.email", "paths@example.invalid"]);
        p.git(&["config", "user.name", "Path fixture"]);
        p.git(&["config", "core.autocrlf", "false"]);
        p.json(&["init", "--manifest", "mapping.toml", "--accept"]);
        p.git(&[
            "add",
            "work.yaml",
            "mapping.toml",
            "decisions/scope.md",
            "oracle.txt",
            ".gitignore",
        ]);
        p.git(&["commit", "-m", "Declare the synthetic saving contract"]);
        if checked {
            p.write(
                ".awr/gates.toml",
                b"version=1\n[checks]\nmode='enforce'\nscope='all'\n",
            );
        }
        p
    }

    pub fn write(&self, path: &str, bytes: &[u8]) {
        fs::write(self.root.join(path), bytes).unwrap();
    }

    pub fn write_json(&self, path: &str, value: &Value) {
        self.write(path, &serde_json::to_vec_pretty(value).unwrap());
    }

    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    pub fn raw(&self, args: &[&str], json: bool, input: Option<&Value>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_awr"));
        command
            .current_dir(&self.root)
            .arg("--project")
            .arg(&self.root)
            .env("AWR_LOCAL_WEB", "0")
            .env_remove("AWR_HOOK_GATES")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if json {
            command.arg("--json");
        }
        let mut child = command.args(args).spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(input).unwrap())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        child.wait_with_output().unwrap()
    }

    pub fn json(&self, args: &[&str]) -> Value {
        let output = self.raw(args, true, None);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    pub fn call(&self, flow: &mut Flow, name: &str, args: &[&str], json: bool) -> String {
        let output = self.raw(args, json, None);
        flow.calls.push(Call {
            name: name.into(),
            bytes: output.stdout.len() + output.stderr.len(),
        });
        assert!(
            output.status.success(),
            "{name} {args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    // Bookkeeping reads fixture state directly; there are no uncounted CLI invocations
    // between measured steps. Setup and independent final verification are outside the path.
    pub fn revision(&self) -> String {
        let store = Store::open_readonly(&self.root.join(".awr/state.db")).unwrap();
        store
            .project_by_root(&self.root.canonicalize().unwrap())
            .unwrap()
            .project_revision
            .to_string()
    }

    pub fn assessment(&self) -> Value {
        let store = Store::open_readonly(&self.root.join(".awr/state.db")).unwrap();
        awr_runtime::assess_management(
            &store,
            &self.root.canonicalize().unwrap(),
            &awr_runtime::AssessManagementRequest {
                work: "W".into(),
                branch: None,
            },
        )
        .unwrap()
    }

    pub fn start(&self, flow: &mut Flow, agent: &str, json: bool, claim: bool) -> String {
        let revision = self.revision();
        let mut args = vec![
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            agent,
            "--provider",
            "fixture",
            "--model",
            "local",
            "--expected-revision",
            &revision,
        ];
        if claim {
            args.extend(["--claim", "--ttl-ms", "60000"]);
        }
        let text = self.call(flow, "session start", &args, json);
        if json {
            serde_json::from_str::<Value>(&text).unwrap()["session"]["id"]
                .as_str()
                .unwrap()
                .into()
        } else {
            text.lines()
                .find_map(|line| line.strip_prefix("Session: "))
                .unwrap()
                .into()
        }
    }

    pub fn check(&self, flow: &mut Flow, session: &str, phase: &str, json: bool) {
        let revision = self.revision();
        let mut args = vec![
            "work",
            "check",
            "W",
            "--session",
            session,
            "--phase",
            phase,
            "--require-met",
            "--expected-revision",
            &revision,
        ];
        if phase == "acceptance" {
            args.extend(["--write-input", ".awr/completion.json"]);
        }
        self.call(flow, &format!("work check {phase}"), &args, json);
    }

    pub fn admit(
        &self,
        flow: &mut Flow,
        session: &str,
        decision: &str,
        proposal: Option<&str>,
        json: bool,
    ) -> Value {
        let mut request = json!({"work":"W","session":session,"request_key":format!("{session}-{decision}"),
            "expected_revision":self.revision().parse::<u64>().unwrap(),"decision":decision,
            "contract_fingerprint":self.assessment()["contract_fingerprint"]});
        if let Some(proposal) = proposal {
            request["proposal_event"] = json!(proposal);
        } else {
            request["judgment"] = json!({"contribution":"Retain the draft","necessity":"A failed save loses input",
            "alternatives":"Reuse the local draft buffer","deliverable":"Draft retention","exit":"Both registered checks pass","responsibility":"draft-storage"});
        }
        self.write_json(".awr/admit.json", &request);
        serde_json::from_str(&self.call(
            flow,
            &format!("work admit {decision}"),
            &["work", "admit", "--input", ".awr/admit.json"],
            json,
        ))
        .unwrap()
    }
}

pub fn checked_flow(
    json: bool,
    separate_reviewer: bool,
    preparation: bool,
) -> (Project, Flow, Flow, usize) {
    // P2 starts at admission for existing in-progress work. The governance comparison
    // starts at preparation for planned work, matching its measured unchecked baseline.
    let p = Project::with_status(
        true,
        if preparation {
            "planned"
        } else {
            "in_progress"
        },
    );
    let mut author = Flow::default();
    let mut reviewer = Flow::default();
    let guidance = if preparation {
        let prepared = p.call(
            &mut author,
            "work prepare",
            &["work", "prepare", "W", "--response-view", "action"],
            json,
        );
        serde_json::to_vec(&serde_json::from_str::<Value>(&prepared).unwrap()["guidance"])
            .unwrap()
            .len()
    } else {
        0
    };
    let session = p.start(&mut author, "author", json, false);
    p.check(&mut author, &session, "baseline", json);
    let proposed = p.admit(
        &mut author,
        &session,
        if separate_reviewer {
            "proposed"
        } else {
            "approved"
        },
        None,
        json,
    );
    if separate_reviewer {
        let review_session = p.start(&mut reviewer, "reviewer", json, false);
        p.admit(
            &mut reviewer,
            &review_session,
            "approved",
            proposed["event"]["id"].as_str(),
            json,
        );
    }
    p.call(
        &mut author,
        "work claim",
        &[
            "work",
            "claim",
            "W",
            "--session",
            &session,
            "--ttl-ms",
            "60000",
            "--expected-revision",
            &p.revision(),
        ],
        json,
    );
    if preparation {
        p.call(
            &mut author,
            "work progress",
            &[
                "work",
                "progress",
                "W",
                "--session",
                &session,
                "--reason",
                "Start draft retention",
                "--next-action",
                "Implement the saving fallback",
                "--expected-revision",
                &p.revision(),
            ],
            json,
        );
    }
    fs::copy(p.root.join("oracle.txt"), p.root.join("actual.txt")).unwrap();
    if preparation {
        // Completion checks bind a committed source tree. This Git operation is
        // fixture implementation work, separate from calls of the AWR CLI.
        p.git(&["add", "work.yaml"]);
        p.git(&["commit", "-m", "Record the started saving work"]);
    }
    p.check(&mut author, &session, "acceptance", json);
    let revision = p.revision();
    let mut complete = vec![
        "work",
        "complete",
        "W",
        "--session",
        &session,
        "--reason",
        "The registered saving checks pass",
        "--input",
        ".awr/completion.json",
        "--end-session",
        "--expected-revision",
        &revision,
    ];
    if json {
        complete.extend(["--response-view", "summary"]);
    }
    p.call(
        &mut author,
        "work complete and session end",
        &complete,
        json,
    );
    assert_eq!(
        p.json(&["work", "show", "W"])["work"]["status"],
        "completed"
    );
    let ended = p.json(&["session", "show", &session]);
    assert_eq!(ended["session"]["status"], "ended");
    assert!(
        ended["claims"]
            .as_array()
            .unwrap()
            .iter()
            .all(|claim| claim["status"] == "released")
    );
    (p, author, reviewer, guidance)
}
