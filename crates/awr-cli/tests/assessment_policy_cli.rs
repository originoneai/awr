//! DEC-040 through the real binary: the policy file is a source, changed only by a previewed and confirmed write, and
//! nothing the runtime records ever changes it.
use awr_core::Id;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::SystemTime,
};

const WORK: &str = "work_items:\n- id: W\n  title: Prepare customer analysis\n  status: ready\n  next_action: Draft the analysis\n  depends_on: [D]\n  acceptance: [Deliver the reviewed analysis]\n- id: D\n  title: Required input\n  status: completed\n";
const RULE: &str = "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve all hard source facts exactly.\n";
const SOURCES: &str = "[project]\nname='DEC040 fixture'\nexternal_key='dec040'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n";
const POLICY: &str = ".awr/assessment-policy.toml";
const QUIET: &str = "format = 1\npolicy_id = \"quiet-advice\"\n\n[[version]]\nversion = 1\nnote = \"No nudge to record observations; the diagnostics stay\"\n\n[version.advice]\nrecord_management_observations = false\n";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-dec040-cli-{}", Id::new()));
        fs::create_dir_all(root.join("decisions")).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(root.join("rules.md"), RULE).unwrap();
        fs::write(
            root.join("goal.md"),
            "# Deliver useful analysis\n\nPersist work and deliver reviewed customer results.\n",
        )
        .unwrap();
        fs::write(root.join("sources.toml"), SOURCES).unwrap();
        let f = Self(fs::canonicalize(&root).unwrap());
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.0)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert!(
            r.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn refused(&self, args: &[&str]) -> (Option<i32>, String) {
        let r = self.run(args);
        assert!(!r.status.success(), "{args:?} should have been refused");
        (
            r.status.code(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&r.stdout),
                String::from_utf8_lossy(&r.stderr)
            ),
        )
    }
    fn path(&self) -> PathBuf {
        self.0.join(POLICY)
    }
    fn write_candidate(&self, name: &str, text: &str) -> String {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }
    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }
    /// What an explanation says: the advice codes, the policy notice and the facts the policy must not touch.
    fn explained(&self) -> (Vec<String>, Value, Value) {
        let prepared = self.ok(&["work", "prepare", "W", "--explain"]);
        let view = &prepared[awr_runtime::ASSESSMENT_EXPLAIN_FIELD];
        let advice = view["envelope"]["advisory_actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["code"].as_str().unwrap().to_string())
            .collect();
        let facts = json!({"ready": prepared["ready"], "mode": prepared["management"]["decision"]["mode"],
            "required_actions": prepared["management"]["decision"]["required_actions"],
            "hard_gate": view["envelope"]["hard_gate"]});
        (advice, view["policy"].clone(), facts)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_capability_is_advertised_and_show_reads_without_writing() {
    let f = Fixture::new();
    let caps = f.ok(&["capabilities", "--require", "assessment.policy"]);
    let entry = caps["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "assessment.policy")
        .expect("assessment.policy is listed");
    assert_eq!(entry["available"], true);
    let before = f.revision();
    let shown = f.ok(&["assessment", "policy", "show"]);
    assert_eq!(shown["ok"], true);
    assert_eq!(shown["policy"]["in_force"]["source"], "builtin");
    assert!(shown["policy"]["versions"].as_array().unwrap().is_empty());
    assert!(shown["policy"]["file_fingerprint"].is_null());
    assert!(!f.path().exists(), "show must not create the file");
    assert_eq!(
        f.revision(),
        before,
        "show must not move the project revision"
    );
    // The as_of can be given, and then the project database is not even needed to answer.
    let given = f.ok(&["assessment", "policy", "show", "--as-of", "12"]);
    assert_eq!(given["policy"]["as_of"], 12);
}

#[test]
fn a_previewed_change_confirmed_with_its_fingerprints_changes_what_an_explanation_says_and_nothing_else()
 {
    let f = Fixture::new();
    let (advice, notice, facts) = f.explained();
    assert_eq!(advice, vec!["record_management_observations"]);
    assert!(notice.is_null(), "no file, nothing to say about a policy");
    let candidate = f.write_candidate("candidate.toml", QUIET);

    let preview = f.ok(&["assessment", "policy", "preview", "--input", &candidate]);
    let preview = &preview["preview"];
    assert_eq!(preview["ok"], true);
    assert_eq!(preview["writes"], false);
    assert!(preview["current_fingerprint"].is_null());
    let fields: Vec<&str> = preview["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["field"].as_str().unwrap())
        .collect();
    assert_eq!(
        fields,
        vec!["policy", "advice.record_management_observations"]
    );
    assert!(!f.path().exists(), "a preview must not create the file");
    let candidate_fingerprint = preview["candidate_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();

    let applied = f.ok(&[
        "assessment",
        "policy",
        "apply",
        "--input",
        &candidate,
        "--expected-current",
        "absent",
        "--expected-candidate",
        &candidate_fingerprint,
    ]);
    assert_eq!(applied["applied"]["wrote"], true);
    assert_eq!(fs::read_to_string(f.path()).unwrap(), QUIET);

    let (advice, notice, after_facts) = f.explained();
    assert_eq!(advice, vec!["no_advisory"]);
    assert_eq!(notice["source"], "project");
    assert_eq!(notice["policy_id"], "quiet-advice");
    assert_eq!(notice["file_fingerprint"], candidate_fingerprint);
    assert!(
        notice["policy_hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    // What the policy is not allowed to touch is exactly as it was.
    assert_eq!(after_facts, facts);
    // `work assess` reads the same policy.
    let assessed = f.ok(&["work", "assess", "W", "--explain"]);
    let view = &assessed[awr_runtime::ASSESSMENT_EXPLAIN_FIELD];
    assert_eq!(view["policy"]["policy_id"], "quiet-advice");
    assert_eq!(
        view["envelope"]["advisory_actions"][0]["code"],
        "no_advisory"
    );
    let shown = f.ok(&["assessment", "policy", "show"]);
    assert_eq!(shown["policy"]["in_force"]["version"], 1);
    assert_eq!(
        shown["policy"]["in_force"]["disabled_advice"],
        json!(["record_management_observations"])
    );
}

#[test]
fn a_confirmation_that_does_not_match_or_a_candidate_that_is_not_valid_writes_nothing() {
    let f = Fixture::new();
    let candidate = f.write_candidate("candidate.toml", QUIET);
    let preview = f.ok(&["assessment", "policy", "preview", "--input", &candidate]);
    let fingerprint = preview["preview"]["candidate_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    // The file is not what was read (something is expected, nothing is there).
    let (_, message) = f.refused(&[
        "assessment",
        "policy",
        "apply",
        "--input",
        &candidate,
        "--expected-current",
        "sha256:00",
        "--expected-candidate",
        &fingerprint,
    ]);
    assert!(message.contains("SourceConflict"), "{message}");
    // The candidate is not the one previewed.
    let (_, message) = f.refused(&[
        "assessment",
        "policy",
        "apply",
        "--input",
        &candidate,
        "--expected-current",
        "absent",
        "--expected-candidate",
        "sha256:11",
    ]);
    assert!(message.contains("SourceConflict"), "{message}");
    assert!(!f.path().exists());
    // An invalid candidate: the preview says so with exit status 1 and the codes, and apply refuses.
    let bad = f.write_candidate(
        "bad.toml",
        "format = 1\npolicy_id = \"p\"\n[[version]]\nversion = 1\n[version.advice]\nquery_original_operation_result = false\n[version.limits]\nmax_advisory = 99\n",
    );
    let (code, message) = f.refused(&["assessment", "policy", "preview", "--input", &bad]);
    assert_eq!(code, Some(1));
    for needle in [
        "protected_rule",
        "limit_out_of_range",
        "query_original_operation_result",
    ] {
        assert!(message.contains(needle), "{needle}: {message}");
    }
    let bad_fingerprint = format!("sha256:{:x}", {
        use sha2::{Digest, Sha256};
        Sha256::digest(fs::read(&bad).unwrap())
    });
    let (_, message) = f.refused(&[
        "assessment",
        "policy",
        "apply",
        "--input",
        &bad,
        "--expected-current",
        "absent",
        "--expected-candidate",
        &bad_fingerprint,
    ]);
    assert!(message.contains("protected_rule"), "{message}");
    assert!(!f.path().exists());
    // A file that is not a policy at all is told by place, without its content.
    let junk = f.write_candidate(
        "junk.toml",
        "format = 1\npolicy_id = \"p\"\nrm_minus_rf = \"yes\"\n",
    );
    let (_, message) = f.refused(&["assessment", "policy", "preview", "--input", &junk]);
    assert!(
        message.contains("at line") && !message.contains("rm_minus_rf"),
        "{message}"
    );
}

#[test]
fn a_second_writer_with_a_stale_view_is_refused_and_the_versions_in_use_cannot_be_rewritten() {
    let f = Fixture::new();
    let first = f.write_candidate("one.toml", QUIET);
    let fingerprint = |text: &str| {
        use sha2::{Digest, Sha256};
        format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
    };
    f.ok(&[
        "assessment",
        "policy",
        "apply",
        "--input",
        &first,
        "--expected-current",
        "absent",
        "--expected-candidate",
        &fingerprint(QUIET),
    ]);
    let two =
        format!("{QUIET}\n[[version]]\nversion = 2\neffective_from = 500\nextends = \"builtin\"\n");
    let rival = format!(
        "{QUIET}\n[[version]]\nversion = 2\neffective_from = 900\n[version.advice]\nrefresh_sources = false\n"
    );
    let path_two = f.write_candidate("two.toml", &two);
    let path_rival = f.write_candidate("rival.toml", &rival);
    f.ok(&[
        "assessment",
        "policy",
        "apply",
        "--input",
        &path_two,
        "--expected-current",
        &fingerprint(QUIET),
        "--expected-candidate",
        &fingerprint(&two),
    ]);
    // The rival read the file before version 2 existed.
    let (_, message) = f.refused(&[
        "assessment",
        "policy",
        "apply",
        "--input",
        &path_rival,
        "--expected-current",
        &fingerprint(QUIET),
        "--expected-candidate",
        &fingerprint(&rival),
    ]);
    assert!(message.contains("SourceConflict"), "{message}");
    assert_eq!(fs::read_to_string(f.path()).unwrap(), two);
    // Editing version 1 in place is not a change; a new version is.
    let rewritten = two.replace(
        "record_management_observations = false",
        "record_management_observations = true",
    );
    let path_rewritten = f.write_candidate("rewritten.toml", &rewritten);
    let (code, message) = f.refused(&[
        "assessment",
        "policy",
        "preview",
        "--input",
        &path_rewritten,
    ]);
    assert_eq!(code, Some(1));
    assert!(message.contains("history_changed"), "{message}");
    // Version 2 only takes effect from its own as_of.
    let at_100 = f.ok(&["assessment", "policy", "show", "--as-of", "100"]);
    let at_600 = f.ok(&["assessment", "policy", "show", "--as-of", "600"]);
    assert_eq!(at_100["policy"]["in_force"]["version"], 1);
    assert_eq!(at_600["policy"]["in_force"]["version"], 2);
    assert_eq!(at_600["policy"]["in_force"]["disabled_advice"], json!([]));
}

#[test]
fn a_policy_file_the_program_cannot_use_never_shapes_an_explanation() {
    let f = Fixture::new();
    for (text, diagnostic) in [
        (
            "format = 1\npolicy_id = \"p\"\n[[version]]\nversion = 1\n[version.advice]\nno_advisory = false\nrecord_management_observations = false\n",
            "project_policy_refused:protected_rule",
        ),
        (
            "format = 9\npolicy_id = \"p\"\n",
            "project_policy_newer_format",
        ),
        ("this is not toml {", "project_policy_invalid"),
    ] {
        fs::write(f.path(), text).unwrap();
        let (advice, notice, _) = f.explained();
        assert_eq!(
            advice,
            vec!["record_management_observations"],
            "{diagnostic}"
        );
        assert_eq!(notice["diagnostics"], json!([diagnostic]));
        assert_eq!(notice["source"], "builtin");
    }
}

#[test]
fn nothing_the_runtime_records_ever_changes_the_policy_file() {
    let f = Fixture::new();
    let candidate = f.write_candidate("candidate.toml", QUIET);
    let fingerprint = {
        use sha2::{Digest, Sha256};
        format!("sha256:{:x}", Sha256::digest(QUIET.as_bytes()))
    };
    f.ok(&[
        "assessment",
        "policy",
        "apply",
        "--input",
        &candidate,
        "--expected-current",
        "absent",
        "--expected-candidate",
        &fingerprint,
    ]);
    let stamp = |p: &PathBuf| -> (Vec<u8>, SystemTime) {
        (
            fs::read(p).unwrap(),
            fs::metadata(p).unwrap().modified().unwrap(),
        )
    };
    let before = stamp(&f.path());
    // Sessions, claims, progress, supervised commands, reindexing, reads with explanations, health checks: the records of a day.
    let session = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "observer",
        "--provider",
        "fixture",
        "--model",
        "m",
        "--claim",
        "--expected-revision",
        &f.revision(),
    ])["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    f.ok(&[
        "work",
        "progress",
        "W",
        "--session",
        &session,
        "--reason",
        "Started",
        "--next-action",
        "Keep going",
        "--expected-revision",
        &f.revision(),
    ]);
    f.ok(&[
        "execution",
        "run",
        "--session",
        &session,
        "--key",
        "noisy",
        "--purpose",
        "A noisy command",
        "--",
        "echo",
        "advice.record_management_observations = true",
    ]);
    f.ok(&["source", "reindex"]);
    let _ = f.explained();
    let _ = f.run(&["doctor"]);
    f.ok(&[
        "session",
        "end",
        "--session",
        &session,
        "--outcome",
        "ended",
        "--expected-revision",
        &f.revision(),
    ]);
    let after = stamp(&f.path());
    assert_eq!(after.0, before.0, "the policy file's bytes changed");
    assert_eq!(after.1, before.1, "the policy file was rewritten");
    let leftovers: Vec<String> = fs::read_dir(f.0.join(".awr"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("assessment-policy") && n != "assessment-policy.toml")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
