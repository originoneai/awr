use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("awr-content-review-{}", awr_core::Id::new()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn review(&self, source: &str) -> PathBuf {
        let result = self.ok(&["intake", "review", "--source", source]);
        let mut review = result["review"].clone();
        review["reviewer"] = json!("synthetic-agent");
        review["reviewed_at"] = json!(1000);
        review["decisions"]=json!(review["assessment"]["findings"].as_array().unwrap().iter().map(|f|json!({"finding_id":f["id"],"reason":"Public synthetic marker verified against the source contract."})).collect::<Vec<_>>());
        let path = self.0.join(".review.json");
        fs::write(&path, serde_json::to_vec(&review).unwrap()).unwrap();
        path
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn public_literals_index_from_arbitrary_sources_without_reviews() {
    let p = Project::new();
    fs::create_dir(p.0.join("design")).unwrap();
    let notes = "# Public options {#options status=active}\n\nUse `refreshToken=false`; observe `checkpoint_token=null`、then continue.\n";
    fs::write(p.0.join("design/options.md"), notes).unwrap();
    let ledger = "goals:\n- id: G\n  title: Explain public options\n  status: active\n  success_criteria: [Usable context]\nwork_items:\n- id: W\n  title: Document options\n  status: ready\n  goal: G\n  summary: 'Call(refreshToken=false); checkpoint_token=null、then continue.'\n  acceptance: [Usable context]\n  next_action: Explain the option types\n";
    fs::write(p.0.join("queue.yaml"), ledger).unwrap();
    fs::write(p.0.join("manifest.toml"), "[project]\nname = \"Public literal fixture\"\ncontext_profile = \"minimal\"\n[[sources]]\ndomain = \"ledger\"\nrole = \"primary\"\npath = \"queue.yaml\"\nadapter = \"yaml-ledger-v1\"\n[[sources]]\ndomain = \"plan\"\nrole = \"primary\"\npath = \"design/options.md\"\nadapter = \"markdown-heading-v1\"\n").unwrap();
    let preview = p.ok(&["init", "--manifest", "manifest.toml"]);
    assert_eq!(preview["preview"]["can_apply"], true);
    p.ok(&["init", "--manifest", "manifest.toml", "--accept"]);
    p.ok(&["source", "reindex"]);
    let context = p.ok(&["context", "compile", "--work", "W", "--budget", "6500"]);
    assert_eq!(context["completeness"]["complete"], true);
    let rendered = context["work_context"]["rendered_context"]
        .as_str()
        .unwrap();
    assert!(rendered.contains("refreshToken=false"));
    assert!(rendered.contains("checkpoint_token=null"));
    assert_eq!(
        p.ok(&["intake", "review", "--source", "design/options.md"])["status"],
        "clear"
    );
    assert!(!p.0.join(".awr/content-reviews").exists());
    assert_eq!(fs::read_to_string(p.0.join("queue.yaml")).unwrap(), ledger);
    assert_eq!(
        fs::read_to_string(p.0.join("design/options.md")).unwrap(),
        notes
    );
    fs::write(
        p.0.join("design/options.md"),
        format!("{notes}\nOrdinary update.\n"),
    )
    .unwrap();
    p.ok(&["source", "reindex"]);
    fs::write(
        p.0.join("design/options.md"),
        format!("{notes}\npassword: synthetic-private-value\n"),
    )
    .unwrap();
    let rejected = p.run(&["source", "reindex"]);
    assert!(!rejected.status.success());
    assert!(!String::from_utf8_lossy(&rejected.stdout).contains("synthetic-private-value"));
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("synthetic-private-value"));
}

#[test]
fn reviewed_original_survives_init_reindex_and_source_queries() {
    let p = Project::new();
    let text = "# Public protocol {#protocol status=active}\n\npassword: public-marker\n";
    fs::write(p.0.join("GOALS.md"), text).unwrap();
    let blocked = p.run(&["init"]);
    assert!(!blocked.status.success());
    assert!(!p.0.join(".awr").exists());
    let draft = p.0.join(".diagnostic.json");
    assert!(
        !p.run(&["init", "--write-draft", draft.to_str().unwrap()])
            .status
            .success()
    );
    let diagnostic = fs::read_to_string(draft).unwrap();
    assert!(!diagnostic.contains("public-marker"));
    assert!(diagnostic.contains("content_review_required"));
    let review = p.review("GOALS.md");
    p.ok(&[
        "intake",
        "review",
        "--from-review",
        review.to_str().unwrap(),
    ]);
    assert!(!p.0.join(".awr/state.db").exists());
    p.ok(&["init", "--accept"]);
    p.ok(&["source", "reindex"]);
    assert_eq!(fs::read_to_string(p.0.join("GOALS.md")).unwrap(), text);
    let work = p.ok(&["work", "show", "INTAKE-001"]);
    assert!(work["ok"].as_bool().unwrap_or(true));
    let context = p.ok(&[
        "context",
        "compile",
        "--work",
        "INTAKE-001",
        "--budget",
        "6500",
    ]);
    assert!(
        context["work_context"]["rendered_context"]
            .as_str()
            .unwrap()
            .contains("password: public-marker")
    );
    p.ok(&[
        "context",
        "bootstrap",
        "--work",
        "INTAKE-001",
        "--budget",
        "6500",
    ]);
}
#[test]
fn hard_findings_and_stale_or_cross_project_receipts_never_archive() {
    let p = Project::new();
    fs::write(
        p.0.join("GOALS.md"),
        "# Protocol\npassword: public-marker\n",
    )
    .unwrap();
    let review = p.review("GOALS.md");
    fs::write(
        p.0.join("GOALS.md"),
        "# Protocol\npassword: changed-marker\n",
    )
    .unwrap();
    assert!(
        !p.run(&[
            "intake",
            "review",
            "--from-review",
            review.to_str().unwrap()
        ])
        .status
        .success()
    );
    assert!(!p.0.join(".awr").exists());
    let other = Project::new();
    assert!(
        !other
            .run(&[
                "intake",
                "review",
                "--from-review",
                review.to_str().unwrap()
            ])
            .status
            .success()
    );
    fs::write(
        p.0.join("GOALS.md"),
        "# Protocol\n-----BEGIN PRIVATE KEY-----\nsynthetic material\n",
    )
    .unwrap();
    let review = p.review("GOALS.md");
    assert!(
        !p.run(&[
            "intake",
            "review",
            "--from-review",
            review.to_str().unwrap()
        ])
        .status
        .success()
    );
    assert!(!p.0.join(".awr").exists());
}

#[test]
fn reviewed_task_fields_reach_search_hard_context_and_bootstrap() {
    let p = Project::new();
    let text = "goals:\n- id: G\n  title: Public protocol\n  status: active\n  success_criteria: [Usable]\nwork_items:\n- id: W\n  title: Explain public fields\n  status: ready\n  goals: [G]\n  summary: 'password: public-marker'\n  next_action: 'password: public-next'\n  acceptance: ['password: public-check']\n";
    fs::write(p.0.join("work-ledger.yaml"), text).unwrap();
    let receipt = p.review("work-ledger.yaml");
    let first = p.ok(&[
        "intake",
        "review",
        "--from-review",
        receipt.to_str().unwrap(),
    ]);
    let second = p.ok(&[
        "intake",
        "review",
        "--from-review",
        receipt.to_str().unwrap(),
    ]);
    assert_eq!(first["receipt"], second["receipt"]);
    p.ok(&["init", "--accept"]);
    let work = p.ok(&["work", "show", "W"]);
    assert!(work.to_string().contains("public-next"));
    for command in ["compile", "bootstrap"] {
        let context = p.ok(&["context", command, "--work", "W", "--budget", "6500"]);
        assert!(context.to_string().contains("public-next"));
    }
    let search = p.ok(&["search", "public-marker"]);
    assert!(search.to_string().contains("password: public-marker"));
    // Source changes invalidate the receipt even when only benign text changes.
    fs::write(p.0.join("work-ledger.yaml"), format!("{text}\n# changed\n")).unwrap();
    let stale = p.run(&["source", "reindex"]);
    assert!(!stale.status.success());
    assert!(!String::from_utf8_lossy(&stale.stdout).contains("public-marker"));
}

#[test]
fn reviewed_markdown_task_fields_and_unchanged_receipt_recovery() {
    let p = Project::new();
    let text = "# Tasks\n\n| id | title | status | next_action | acceptance |\n| --- | --- | --- | --- | --- |\n| W | Public example | ready | password: public-next | password: public-check |\n";
    fs::write(p.0.join("tasks.md"), text).unwrap();
    fs::write(p.0.join("manifest.toml"), "[project]\nname = \"Review fixture\"\n[[sources]]\ndomain = \"ledger\"\nrole = \"primary\"\npath = \"tasks.md\"\nadapter = \"markdown-ledger-v1\"\n").unwrap();
    let receipt = p.review("tasks.md");
    p.ok(&[
        "intake",
        "review",
        "--from-review",
        receipt.to_str().unwrap(),
    ]);
    p.ok(&["init", "--manifest", "manifest.toml", "--accept"]);
    let show = p.ok(&["work", "show", "W"]);
    assert_eq!(show["work"]["next_action"], "password: public-next");
    // Models the empty binding table after upgrading an existing database.
    let db = rusqlite::Connection::open(p.0.join(".awr/state.db")).unwrap();
    db.execute("DELETE FROM source_content_reviews", [])
        .unwrap();
    drop(db);
    p.ok(&["source", "reindex"]);
    let show = p.ok(&["work", "show", "W"]);
    assert_eq!(show["acceptance"][0], "password: public-check");
}
