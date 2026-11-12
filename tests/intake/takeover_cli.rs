//! `awr takeover`: an unattended, resumable takeover of an existing project (issue #620).
//!
//! The command drives the CLI's own `init` and `intake inspect` in persisted steps. These tests pin what a
//! caller relies on: the project's own files stay byte-identical, only `.awr/` and the `.gitignore` entries AWR
//! lists itself are written, nothing is committed, a secret source stops the run with its place and never its
//! value, a reviewer cannot raise a goal above `candidate` or leave the ledger without work, and a stopped run
//! resumes after the steps that finished.

use awr_core::Id;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Project {
    root: PathBuf,
    state: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let id = Id::new();
        let base = std::env::temp_dir();
        let p = Self {
            root: base.join(format!("awr-takeover-{name}-{id}")),
            state: base.join(format!("awr-takeover-state-{name}-{id}")),
        };
        fs::create_dir_all(&p.root).unwrap();
        p
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
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
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}\n{}\n{}",
            text(&out.stdout),
            text(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn takeover(&self, extra: &[&str]) -> Output {
        let state = self.state.to_string_lossy().into_owned();
        let mut args = vec!["takeover", "--state-dir", state.as_str()];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// Every file under the project except the given top-level names, by relative path.
    fn snapshot(&self, skip: &[&str]) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, skip: &[&str], out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let relative = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if skip.contains(&relative.as_str()) {
                    continue;
                }
                if path.is_dir() {
                    walk(&path, base, skip, out);
                } else {
                    out.insert(relative, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &self.root, skip, &mut out);
        out
    }

    /// A JSON file in the state directory (outside the project), for example a host's review answer.
    fn state_file(&self, name: &str, value: &Value) -> PathBuf {
        fs::create_dir_all(&self.state).unwrap();
        let file = self.state.join(name);
        fs::write(&file, value.to_string()).unwrap();
        file
    }

    /// A manifest file (outside the project) that lists `paths` as plan sources.
    fn manifest(&self, name: &str, paths: &[&str]) -> PathBuf {
        let mut toml = String::from("[project]\nname = \"p\"\nauthority_mode = \"source_first\"\n");
        for path in paths {
            toml.push_str(&format!(
                "\n[[sources]]\ndomain = \"plan\"\nrole = \"{}\"\npath = \"{path}\"\nadapter = \"markdown-heading-v1\"\n",
                if toml.contains("[[sources]]") { "supporting" } else { "primary" }
            ));
        }
        fs::create_dir_all(&self.state).unwrap();
        let file = self.state.join(format!("{name}.toml"));
        fs::write(&file, toml).unwrap();
        file
    }

    fn ledger_text(&self) -> String {
        fs::read_to_string(self.root.join(".awr/intake/work-ledger.yaml")).unwrap()
    }

    fn receipt(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.join(".awr/takeover/receipt.json")).unwrap())
            .unwrap()
    }

    /// Everything the command left in its state directory, as one string.
    fn state_text(&self) -> String {
        fn walk(dir: &Path, out: &mut String) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push_str(&String::from_utf8_lossy(&fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = String::new();
        walk(&self.state, &mut out);
        out
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.state);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The outcome object the command prints on stdout, completed or not.
fn outcome(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout is not JSON:\n{}\n{}",
            text(&out.stdout),
            text(&out.stderr)
        )
    })
}

const README: &str =
    "# Demo\n\nA small tool that counts words.\n\n## Goals\n\n- Count words in a file\n";

/// A reviewed ledger that raises its goal to `active` and drops the generated intake work.
const REVIEWED_LEDGER: &str = "goals:\n- id: count-words\n  status: active\n  title: Count words in a file\n  summary: The README states one goal.\n  success_criteria: []\nwork_items: []\n";

#[cfg(unix)]
mod reviewer {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// An executable that stores the request it gets on stdin next to itself, then runs `tail`.
    pub fn script(p: &Project, name: &str, tail: &str) -> PathBuf {
        fs::create_dir_all(&p.state).unwrap();
        let path = p.state.join(name);
        let request = format!("{}.request", path.display());
        fs::write(&path, format!("#!/bin/sh\ncat > '{request}'\n{tail}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    pub fn answering(p: &Project, name: &str, answer: &Value) -> PathBuf {
        fs::create_dir_all(&p.state).unwrap();
        let answer_path = p.state.join(format!("{name}.answer.json"));
        fs::write(&answer_path, answer.to_string()).unwrap();
        script(p, name, &format!("cat '{}'", answer_path.display()))
    }

    pub fn request(script: &Path) -> Value {
        serde_json::from_slice(&fs::read(format!("{}.request", script.display())).unwrap()).unwrap()
    }
}

#[test]
fn a_readme_only_project_is_taken_over_without_touching_its_files() {
    let p = Project::new("readme");
    p.write("README.md", README);
    let before = p.snapshot(&[".awr", ".gitignore"]);

    let out = p.takeover(&[]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let first = outcome(&out);
    assert_eq!(first["outcome"], "completed");

    assert_eq!(
        p.snapshot(&[".awr", ".gitignore"]),
        before,
        "the project's own files stay byte-identical"
    );
    assert!(
        p.root.join(".gitignore").is_file(),
        "AWR's own ignore entries are written"
    );
    let receipt = p.receipt();
    assert_eq!(
        receipt["created"]["written_outside_awr"],
        json!([".gitignore"])
    );
    assert_eq!(receipt["git"]["committed"], false);
    assert_eq!(receipt["git"]["pushed"], false);
    assert_eq!(receipt["reviewed"]["by_reviewer"], false);
    assert_eq!(
        receipt["awaiting_confirmation"][0]["key"], "intake-goal",
        "the generated goal waits for its owner"
    );
    assert!(
        p.ok(&["status"])["project_revision"].is_number(),
        "the project is initialized and answers status"
    );

    // Running it again resumes the recorded run; nothing is redone.
    let again = outcome(&p.takeover(&[]));
    assert_eq!(again["outcome"], "completed");
    assert_eq!(again["record"]["id"], first["record"]["id"]);
    let shown = p.ok(&[
        "takeover",
        "--state-dir",
        &p.state.to_string_lossy(),
        "status",
    ]);
    assert_eq!(shown["record"]["status"], "completed");
    assert_eq!(shown["receipt"]["run"], first["record"]["id"]);
}

#[cfg(unix)]
#[test]
fn a_reviewer_cannot_raise_a_goal_or_empty_the_ledger() {
    let p = Project::new("reviewer");
    p.write("README.md", README);
    let answer = json!({
        "generated_files": { ".awr/intake/work-ledger.yaml": REVIEWED_LEDGER },
        "notes": "One goal from the README."
    });
    let script = reviewer::answering(&p, "reviewer.sh", &answer);

    let out = p.takeover(&["--reviewer", &script.to_string_lossy()]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );

    let request = reviewer::request(&script);
    assert_eq!(request["round"], 1);
    assert!(request["draft"]["generated_files"][".awr/intake/work-ledger.yaml"].is_string());
    assert!(request["project_root"].is_string());

    let ledger = p.ledger_text();
    assert!(
        ledger.contains("id: count-words"),
        "the reviewer's goal is kept:\n{ledger}"
    );
    assert!(
        ledger.contains("status: candidate") && !ledger.contains("status: active"),
        "the goal is capped:\n{ledger}"
    );
    assert!(
        ledger.contains("INTAKE-001"),
        "the intake work item is put back:\n{ledger}"
    );
    let receipt = p.receipt();
    assert_eq!(receipt["reviewed"]["by_reviewer"], true);
    let notes = receipt["reviewed"]["notes"].as_str().unwrap();
    assert!(notes.contains("One goal from the README."), "{notes}");
    assert!(
        notes.contains("lowered to candidate") && notes.contains("intake work was kept"),
        "{notes}"
    );
    assert_eq!(receipt["awaiting_confirmation"][0]["key"], "count-words");
}

#[cfg(unix)]
#[test]
fn a_reviewer_settles_an_ambiguous_authority_on_the_next_run() {
    let p = Project::new("ambiguous");
    p.write("README.md", "# P\n");
    for (file, key) in [("docs/a-ledger.md", "T-a1"), ("docs/b-ledger.md", "T-b1")] {
        p.write(
            file,
            &format!("# Tasks\n\n| 工作项 | 状态 |\n| --- | --- |\n| {key} | planned |\n"),
        );
    }
    let before = p.snapshot(&[]);

    let out = p.takeover(&[]);
    assert!(!out.status.success());
    let stopped = outcome(&out);
    assert_eq!(stopped["record"]["stop"]["reason"], "ambiguous_authority");
    assert_eq!(
        p.snapshot(&[]),
        before,
        "a stop before accept leaves the project untouched"
    );

    // The saved draft is what a reviewer works from: this one picks the first candidate as the primary ledger.
    let draft_path = Path::new(stopped["run_dir"].as_str().unwrap()).join("draft.json");
    let draft: Value = serde_json::from_slice(&fs::read(draft_path).unwrap()).unwrap();
    let mut mapping = draft["authority_mapping"].clone();
    mapping["sources"].as_array_mut().unwrap().push(json!({
        "domain": "ledger", "role": "primary", "path": "docs/a-ledger.md",
        "locator": null, "adapter": "markdown-ledger-v1", "options": {}
    }));
    let answer = json!({ "generated_files": draft["generated_files"], "authority_mapping": mapping, "ambiguous_domains": [] });
    let script = reviewer::answering(&p, "chooser.sh", &answer);

    let out = p.takeover(&["--reviewer", &script.to_string_lossy()]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let done = outcome(&out);
    assert_eq!(done["outcome"], "completed");
    assert_eq!(
        done["record"]["id"], stopped["record"]["id"],
        "the same run continued"
    );
    let manifest = fs::read_to_string(p.root.join(".awr/project.toml")).unwrap();
    assert!(
        manifest.contains("docs/a-ledger.md") && !manifest.contains("docs/b-ledger.md"),
        "{manifest}"
    );
    assert_eq!(
        p.snapshot(&[".awr", ".gitignore"]),
        before,
        "the candidates themselves are untouched"
    );
}

#[cfg(unix)]
#[test]
fn reviewed_files_outside_the_intake_directory_are_refused() {
    let p = Project::new("outside");
    p.write("README.md", README);
    let before = p.snapshot(&[]);
    let answer = json!({ "generated_files": { "README.md": "# overwritten\n" } });
    let script = reviewer::answering(&p, "reviewer.sh", &answer);

    let out = p.takeover(&["--reviewer", &script.to_string_lossy()]);
    assert!(!out.status.success());
    let stop = &outcome(&out)["record"]["stop"];
    assert_eq!(
        (stop["step"].as_str(), stop["reason"].as_str()),
        (Some("review"), Some("review_rejected"))
    );
    assert_eq!(
        p.snapshot(&[]),
        before,
        "nothing was written to the project"
    );
}

#[cfg(unix)]
#[test]
fn a_failing_reviewer_stops_the_run_and_a_rerun_resumes_after_the_finished_steps() {
    let p = Project::new("resume");
    p.write("README.md", README);
    let before = p.snapshot(&[]);

    let failing = reviewer::script(&p, "failing.sh", "echo 'model unavailable' >&2\nexit 3");
    let out = p.takeover(&["--reviewer", &failing.to_string_lossy()]);
    assert!(!out.status.success(), "a stopped run exits non-zero");
    let stopped = outcome(&out);
    assert_eq!(stopped["outcome"], "stopped");
    assert_eq!(stopped["record"]["stop"]["step"], "review");
    assert_eq!(stopped["record"]["stop"]["reason"], "review_failed");
    assert!(
        stopped["record"]["stop"]["message"]
            .as_str()
            .unwrap()
            .contains("model unavailable")
    );
    assert_eq!(
        p.snapshot(&[]),
        before,
        "a run that stops before accept leaves the project untouched"
    );
    let preview_done = stopped["record"]["steps"]["preview"]["finished_at"].clone();
    assert!(preview_done.is_number());

    let good = reviewer::answering(
        &p,
        "good.sh",
        &json!({ "generated_files": { ".awr/intake/work-ledger.yaml": REVIEWED_LEDGER } }),
    );
    let out = p.takeover(&["--reviewer", &good.to_string_lossy()]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let finished = outcome(&out);
    assert_eq!(finished["outcome"], "completed");
    assert_eq!(
        finished["record"]["id"], stopped["record"]["id"],
        "the same run continued"
    );
    assert_eq!(
        finished["record"]["steps"]["preview"]["finished_at"], preview_done,
        "a finished step is not repeated"
    );
}

#[test]
fn a_secret_in_a_source_names_the_place_and_never_the_value() {
    let p = Project::new("secret");
    p.write(
        "README.md",
        "# Demo\n\nsetup\npassword: hunter2hunter2hunter2\n",
    );
    let before = p.snapshot(&[]);

    let out = p.takeover(&[]);
    assert!(!out.status.success());
    let stopped = outcome(&out);
    let stop = &stopped["record"]["stop"];
    assert_eq!(stop["reason"], "secret_boundary");
    assert_eq!(stop["step"], "preview");
    assert_eq!(stop["detail"]["path"], "README.md");
    assert_eq!(stop["detail"]["line"], 4);
    for (label, shown) in [
        ("stdout", text(&out.stdout)),
        ("stderr", text(&out.stderr)),
        ("state", p.state_text()),
    ] {
        assert!(
            !shown.contains("hunter2"),
            "the secret value reached {label}"
        );
    }
    assert_eq!(
        p.snapshot(&[]),
        before,
        "the project is byte-identical and has no .awr/"
    );
    assert!(!p.root.join(".awr").exists());

    // The owner redacts the line; the same command continues the run.
    p.write("README.md", "# Demo\n\nsetup\npassword: <redacted>\n");
    let out = p.takeover(&[]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_eq!(outcome(&out)["record"]["id"], stopped["record"]["id"]);
}

#[test]
fn an_initialized_project_needs_adopt_and_an_active_session_always_stops_the_run() {
    let p = Project::new("adopt");
    p.write("README.md", README);
    assert!(p.takeover(&[]).status.success());

    let refused = p.takeover(&["--rerun"]);
    assert!(!refused.status.success());
    assert_eq!(
        outcome(&refused)["record"]["stop"]["reason"],
        "already_initialized"
    );

    let adopted = p.takeover(&["--rerun", "--adopt"]);
    assert!(
        adopted.status.success(),
        "{}\n{}",
        text(&adopted.stdout),
        text(&adopted.stderr)
    );
    let record = &outcome(&adopted)["record"];
    for step in ["preview", "review", "accept"] {
        assert_eq!(record["steps"][step]["status"], "skipped", "{step}");
    }
    assert_eq!(record["steps"]["organize"]["status"], "done");

    let revision = p.ok(&["status"])["project_revision"].to_string();
    p.ok(&[
        "session",
        "start",
        "--work",
        "INTAKE-001",
        "--agent",
        "fixture-agent",
        "--provider",
        "fixture",
        "--model",
        "fixture",
        "--expected-revision",
        &revision,
        "--claim",
    ]);
    let blocked = p.takeover(&["--rerun", "--adopt"]);
    assert!(!blocked.status.success());
    let stop = &outcome(&blocked)["record"]["stop"];
    assert_eq!(stop["reason"], "active_session");
    assert_eq!(stop["detail"]["sessions"].as_array().map(Vec::len), Some(1));
}

#[test]
fn status_without_a_recorded_run_is_an_error() {
    let p = Project::new("status");
    p.write("README.md", README);
    let out = p.run(&[
        "takeover",
        "--state-dir",
        &p.state.to_string_lossy(),
        "status",
    ]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("no takeover run recorded"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn a_manifest_lets_the_takeover_pass_a_project_with_a_credential_file() {
    let p = Project::new("manifest");
    p.write(
        "README.md",
        "# Demo\n\nsetup\npassword: hunter2hunter2hunter2\n",
    );
    p.write("docs/PLAN.md", "# Plan\n\n## Goals\n\n- Ship it\n");
    let before = p.snapshot(&[]);

    // Without a manifest the run stops at the credential and says how to go on.
    let out = p.takeover(&[]);
    assert!(!out.status.success());
    let stopped = outcome(&out);
    assert_eq!(stopped["record"]["stop"]["reason"], "secret_boundary");
    assert!(
        stopped["record"]["stop"]["message"]
            .as_str()
            .unwrap()
            .contains("--manifest"),
        "{stopped}"
    );
    assert_eq!(p.snapshot(&[]), before);

    // With the owner's list of sources the same run continues and completes.
    let manifest = p.manifest("sources", &["docs/PLAN.md"]);
    let out = p.takeover(&["--manifest", &manifest.to_string_lossy()]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let done = outcome(&out);
    assert_eq!(done["outcome"], "completed");
    assert_eq!(
        done["record"]["id"], stopped["record"]["id"],
        "the same run continued"
    );
    let receipt = p.receipt();
    let sources: Vec<&str> = receipt["mapped"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["path"].as_str().unwrap())
        .collect();
    assert!(
        sources.contains(&"docs/PLAN.md") && !sources.contains(&"README.md"),
        "{sources:?}"
    );
    assert!(!p.state_text().contains("hunter2") && !text(&out.stdout).contains("hunter2"));
    for (path, bytes) in &before {
        assert_eq!(
            fs::read(p.root.join(path)).unwrap(),
            *bytes,
            "{path} changed"
        );
    }
    assert!(p.ok(&["status"])["project_revision"].is_number());
}

#[test]
fn a_credential_in_a_listed_source_still_stops_the_takeover_and_the_hint_changes() {
    let p = Project::new("listed");
    p.write(
        "README.md",
        "# Demo\n\nsetup\npassword: hunter2hunter2hunter2\n",
    );
    let manifest = p.manifest("sources", &["README.md"]);
    let out = p.takeover(&["--manifest", &manifest.to_string_lossy()]);
    assert!(!out.status.success());
    let stop = &outcome(&out)["record"]["stop"];
    assert_eq!(
        (stop["reason"].as_str(), stop["detail"]["path"].as_str()),
        (Some("secret_boundary"), Some("README.md"))
    );
    let message = stop["message"].as_str().unwrap();
    assert!(
        message.contains("The manifest lists this file"),
        "{message}"
    );
    assert!(!text(&out.stdout).contains("hunter2") && !p.root.join(".awr").exists());
}

#[cfg(unix)]
#[test]
fn another_manifest_starts_the_run_again_from_the_preview() {
    let p = Project::new("switch");
    p.write("README.md", README);
    p.write("docs/PLAN.md", "# Plan\n\n## Goals\n\n- First\n");
    p.write("docs/OTHER.md", "# Other\n\n## Goals\n\n- Second\n");
    let failing = reviewer::script(&p, "failing.sh", "exit 3");
    let first = p.manifest("first", &["docs/PLAN.md"]);
    let second = p.manifest("second", &["docs/OTHER.md"]);

    let out = p.takeover(&[
        "--manifest",
        &first.to_string_lossy(),
        "--reviewer",
        &failing.to_string_lossy(),
    ]);
    assert!(!out.status.success());
    let one = outcome(&out);
    assert_eq!(one["record"]["stop"]["reason"], "review_failed");
    let sources_of = |outcome: &Value| -> Vec<String> {
        outcome["record"]["steps"]["preview"]["result"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["path"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(sources_of(&one).contains(&"docs/PLAN.md".to_string()));

    // The same manifest resumes after the finished preview; another one redoes it.
    let same = outcome(&p.takeover(&[
        "--manifest",
        &first.to_string_lossy(),
        "--reviewer",
        &failing.to_string_lossy(),
    ]));
    assert_eq!(
        same["record"]["steps"]["preview"]["finished_at"],
        one["record"]["steps"]["preview"]["finished_at"]
    );
    let other = outcome(&p.takeover(&[
        "--manifest",
        &second.to_string_lossy(),
        "--reviewer",
        &failing.to_string_lossy(),
    ]));
    let redone = sources_of(&other);
    assert!(
        redone.contains(&"docs/OTHER.md".to_string())
            && !redone.contains(&"docs/PLAN.md".to_string()),
        "{redone:?}"
    );
    assert_ne!(
        other["record"]["steps"]["preview"]["finished_at"],
        one["record"]["steps"]["preview"]["finished_at"]
    );
    assert_eq!(
        other["record"]["id"], one["record"]["id"],
        "it is still the same run"
    );
}

const LEDGER: &str = ".awr/intake/work-ledger.yaml";

#[test]
fn a_host_reviews_the_draft_itself_and_the_run_never_applies_it_unreviewed() {
    let p = Project::new("host");
    p.write("README.md", README);
    let before = p.snapshot(&[]);

    // The run pauses at the review with the draft and the request; waiting is not a failure.
    let out = p.takeover(&["--await-review"]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let waiting = outcome(&out);
    assert_eq!(waiting["outcome"], "awaiting_review");
    let stop = &waiting["record"]["stop"];
    assert_eq!(
        (stop["step"].as_str(), stop["reason"].as_str()),
        (Some("review"), Some("awaiting_review"))
    );
    let request: Value = serde_json::from_slice(
        &fs::read(stop["detail"]["request_path"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(request["draft"]["generated_files"][LEDGER].is_string());
    assert_eq!(request["round"], 1);
    assert!(request["project_root"].is_string());
    assert!(Path::new(stop["detail"]["draft_path"].as_str().unwrap()).is_file());
    assert_eq!(
        p.snapshot(&[]),
        before,
        "nothing is written while the host reviews"
    );

    // A later command without the flag keeps waiting; it does not apply the draft as generated.
    let again = p.takeover(&[]);
    assert!(again.status.success());
    assert_eq!(outcome(&again)["outcome"], "awaiting_review");
    assert_eq!(p.snapshot(&[]), before);

    // The host's answer goes through the guards of a reviewer's, and the same run finishes.
    let answer = p.state_file(
        "answer.json",
        &json!({ "generated_files": { LEDGER: REVIEWED_LEDGER }, "notes": "Reviewed by the host." }),
    );
    let out = p.takeover(&["--reviewed", &answer.to_string_lossy()]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let done = outcome(&out);
    assert_eq!(done["outcome"], "completed");
    assert_eq!(
        done["record"]["id"], waiting["record"]["id"],
        "the same run continued"
    );
    let ledger = p.ledger_text();
    assert!(
        ledger.contains("id: count-words")
            && ledger.contains("status: candidate")
            && !ledger.contains("status: active"),
        "{ledger}"
    );
    assert!(
        ledger.contains("INTAKE-001"),
        "the intake work item is put back:\n{ledger}"
    );
    let receipt = p.receipt();
    assert_eq!(receipt["reviewed"]["by_reviewer"], true);
    let notes = receipt["reviewed"]["notes"].as_str().unwrap();
    assert!(
        notes.contains("Reviewed by the host.") && notes.contains("lowered to candidate"),
        "{notes}"
    );
}

#[test]
fn a_refused_host_answer_leaves_the_run_waiting_and_the_next_answer_is_taken() {
    let p = Project::new("refused");
    p.write("README.md", README);
    assert_eq!(
        outcome(&p.takeover(&["--await-review"]))["outcome"],
        "awaiting_review"
    );

    // An answer file that does not exist, one that is not JSON, and one that writes outside .awr/intake/.
    let missing = p.state.join("missing.json");
    let not_json = p.state.join("not-json.txt");
    fs::write(&not_json, "no json here").unwrap();
    let outside = p.state_file(
        "outside.json",
        &json!({ "generated_files": { "README.md": "# overwritten\n" } }),
    );
    for (answer, reason) in [
        (&missing, "review_failed"),
        (&not_json, "review_failed"),
        (&outside, "review_rejected"),
    ] {
        let out = p.takeover(&["--reviewed", &answer.to_string_lossy()]);
        assert!(!out.status.success(), "{}", answer.display());
        assert_eq!(
            outcome(&out)["record"]["stop"]["reason"],
            reason,
            "{}",
            answer.display()
        );
        assert!(!p.root.join(".awr").exists(), "nothing was written");
        // The refusal does not turn the run into one that applies the draft unreviewed.
        let waiting = p.takeover(&[]);
        assert!(waiting.status.success());
        assert_eq!(
            outcome(&waiting)["outcome"],
            "awaiting_review",
            "{}",
            answer.display()
        );
        assert!(!p.root.join(".awr").exists());
    }

    let good = p.state_file(
        "good.json",
        &json!({ "generated_files": { LEDGER: REVIEWED_LEDGER } }),
    );
    let out = p.takeover(&["--reviewed", &good.to_string_lossy()]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_eq!(outcome(&out)["outcome"], "completed");
}

#[test]
fn a_reviewed_answer_can_be_given_up_front_and_the_review_flags_exclude_each_other() {
    let p = Project::new("upfront");
    p.write("README.md", README);
    let answer = p.state_file(
        "answer.json",
        &json!({ "generated_files": { LEDGER: REVIEWED_LEDGER } }),
    );
    let a = answer.to_string_lossy().into_owned();

    for args in [
        vec!["takeover", "--await-review", "--reviewed", &a],
        vec!["takeover", "--reviewed", &a, "--reviewer", "/bin/true"],
        vec!["takeover", "--await-review", "--reviewer", "/bin/true"],
    ] {
        let out = p.run(&args);
        assert!(!out.status.success(), "{args:?}");
        assert!(
            text(&out.stderr).contains("cannot be used with"),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    assert!(!p.root.join(".awr").exists());

    let out = p.takeover(&["--reviewed", &a]);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_eq!(outcome(&out)["outcome"], "completed");
    assert!(p.ledger_text().contains("id: count-words"));
    assert_eq!(p.receipt()["reviewed"]["by_reviewer"], true);
}
