//! The `awr workspace` surface, driven the way an operator drives it.
//!
//! Two project roots stand in for two machines: they share a store, they share
//! nothing else. The store is a directory here, so the suite needs no network
//! and no credentials while still exercising the real command, the real
//! config, and the real commit path.
use awr_core::*;
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    base: PathBuf,
    dev: PathBuf,
    mini: PathBuf,
    store: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("awr-workspace-cli-{}", Id::new()));
        let dev = base.join("dev");
        let mini = base.join("mini");
        let store = base.join("store");
        for path in [&dev, &mini, &store] {
            fs::create_dir_all(path).unwrap();
        }
        let fixture = Self {
            base,
            dev,
            mini,
            store,
        };
        fixture.configure(&fixture.dev, "macbook-codex");
        fixture.configure(&fixture.mini, "mac-mini");
        fixture.write(
            &fixture.dev,
            "work-ledger.yaml",
            "work_items:\n  - id: SHARE-001\n    title: Share one project between two machines\n    status: ready\n    priority: P1\n    acceptance:\n      - The peer's files arrive with the same bytes.\n    next_action: Publish this host's tracked files.\n",
        );
        fixture.write(&fixture.dev, "infra/evidence/dump.json", "{\"ok\":true}\n");
        fixture
    }

    fn configure(&self, root: &Path, host: &str) {
        self.configure_track(root, host, &["work-ledger.yaml", "infra"]);
    }

    fn configure_track(&self, root: &Path, host: &str, track: &[&str]) {
        let track = track
            .iter()
            .map(|item| serde_json::to_string(item).unwrap())
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            root.join("remote_workspace.toml"),
            format!(
                "[project]\nkey   = \"poc-infra\"\nroot  = {}\nhost  = {}\ntrack = [{track}]\n\n[store]\npath = {}\n",
                serde_json::to_string(&root.display().to_string()).unwrap(),
                serde_json::to_string(host).unwrap(),
                serde_json::to_string(&self.store.display().to_string()).unwrap(),
            ),
        )
        .unwrap();
    }

    fn write(&self, root: &Path, relpath: &str, body: &str) {
        let path = root.join(relpath);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn read(&self, root: &Path, relpath: &str) -> Option<String> {
        fs::read_to_string(root.join(relpath)).ok()
    }

    fn run(&self, root: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(root)
            .arg("--json")
            .arg("workspace")
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, root: &Path, args: &[&str]) -> Value {
        let out = self.run(root, args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn refused(&self, root: &Path, args: &[&str]) -> (i32, Value) {
        let out = self.run(root, args);
        let code = out.status.code().unwrap();
        assert_ne!(code, 0, "{args:?} was expected to fail");
        let report: Value = serde_json::from_slice(&out.stderr).unwrap();
        (code, report)
    }

    /// A project with a work item to bind a client conversation to.
    fn init(&self, root: &Path) {
        let out = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args([
                "--project",
                root.to_str().unwrap(),
                "--json",
                "init",
                "--goal",
                "Share one project between two machines",
                "--accept",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "init: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The lifecycle event a client delivers before the first turn.
    fn session_start(&self, root: &Path) -> Value {
        let out = self.session_start_output(root);
        assert!(
            out.status.success(),
            "session start: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// The same delivery, without the expectation that AWR accepts the project.
    fn session_start_output(&self, root: &Path) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args([
                "--project",
                root.to_str().unwrap(),
                "--json",
                "client",
                "hook",
                "--client",
                "codex",
                "--work",
                "SHARE-001",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                &serde_json::to_vec(&serde_json::json!({
                    "session_id": "local-codex",
                    "hook_event_name": "SessionStart",
                    "cwd": root,
                    "model": "cli-test",
                }))
                .unwrap(),
            )
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

fn context_of(hook: &Value) -> &str {
    hook["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("no context in {hook}"))
}

fn state_of<'a>(report: &'a Value, path: &str) -> &'a str {
    report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == path)
        .unwrap_or_else(|| panic!("{path} is missing from the status report"))["state"]
        .as_str()
        .unwrap()
}

/// The whole point: one machine publishes, the other one finds it.
#[test]
fn a_published_file_reaches_the_peer_with_the_same_bytes() {
    let fixture = Fixture::new();
    let status = fixture.ok(&fixture.dev, &["status"]);
    assert_eq!(state_of(&status, "work-ledger.yaml"), "local_ahead");
    assert_eq!(status["conflicted"], serde_json::json!([]));

    let published = fixture.ok(&fixture.dev, &["publish"]);
    assert_eq!(published["pushed"].as_array().unwrap().len(), 2);
    assert_eq!(published["commit_mode"], "cas");
    assert!(published["backend_requests"].as_u64().unwrap() > 0);
    // Nothing lost a commit here, and the report says so by leaving the key
    // out: a clean publish looks exactly as it did before a lost commit was
    // reportable at all.
    assert!(published.get("contended").is_none(), "{published}");

    let synced = fixture.ok(&fixture.mini, &["sync"]);
    assert_eq!(synced["pulled"].as_array().unwrap().len(), 2);
    assert_eq!(synced["changed"], true);
    assert_eq!(
        fixture.read(&fixture.mini, "infra/evidence/dump.json"),
        fixture.read(&fixture.dev, "infra/evidence/dump.json")
    );
    assert_eq!(
        state_of(&fixture.ok(&fixture.mini, &["status"]), "work-ledger.yaml"),
        "in_sync"
    );

    // A second sync finds nothing to do and says so.
    let again = fixture.ok(&fixture.mini, &["sync"]);
    assert_eq!(again["changed"], false);
    assert_eq!(again["inbound_count"], 0);
}

/// A file changed on both sides is reported with exit 1, and neither side is
/// rewritten to make the report look better.
#[test]
fn a_conflict_fails_the_command_and_overwrites_nothing() {
    let fixture = Fixture::new();
    fixture.ok(&fixture.dev, &["publish"]);
    fixture.ok(&fixture.mini, &["sync"]);

    fixture.write(&fixture.dev, "work-ledger.yaml", "dev version\n");
    fixture.ok(&fixture.dev, &["publish"]);
    fixture.write(&fixture.mini, "work-ledger.yaml", "mini version\n");

    let (code, report) = fixture.refused(&fixture.mini, &["sync"]);
    assert_eq!(code, 1);
    assert_eq!(report["code"], "WorkspaceConflict");
    assert_eq!(
        fixture.read(&fixture.mini, "work-ledger.yaml").as_deref(),
        Some("mini version\n")
    );
    assert_eq!(
        fixture.read(&fixture.dev, "work-ledger.yaml").as_deref(),
        Some("dev version\n")
    );
}

/// A dry run reports what a publish would do and writes nothing.
#[test]
fn a_dry_run_leaves_the_store_alone() {
    let fixture = Fixture::new();
    let preview = fixture.ok(&fixture.dev, &["publish", "--dry-run"]);
    assert_eq!(preview["dry_run"], true);
    let mut paths: Vec<&str> = preview["pushed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    paths.sort();
    assert_eq!(paths, ["infra/evidence/dump.json", "work-ledger.yaml"]);
    assert!(
        preview["pushed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| file["content_uploaded"] == true)
    );
    assert!(
        !fixture
            .store
            .join("projects/poc-infra/manifest.json")
            .exists()
    );
    let after = fixture.ok(&fixture.dev, &["status"]);
    assert_eq!(after["index_source"], "pointers");
    assert_eq!(state_of(&after, "work-ledger.yaml"), "local_ahead");

    let published = fixture.ok(&fixture.dev, &["publish"]);
    assert!(published.get("dry_run").is_none(), "{published}");
}

/// A dry-run that would conflict still exits 0: it is a preview, not a publish.
#[test]
fn a_dry_run_with_conflicts_does_not_fail() {
    let fixture = Fixture::new();
    fixture.ok(&fixture.dev, &["publish"]);
    fixture.ok(&fixture.mini, &["sync"]);
    fixture.write(&fixture.dev, "work-ledger.yaml", "dev version\n");
    fixture.ok(&fixture.dev, &["publish"]);
    fixture.write(&fixture.mini, "work-ledger.yaml", "mini version\n");
    let preview = fixture.ok(&fixture.mini, &["publish", "--dry-run"]);
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(
        fixture.read(&fixture.mini, "work-ledger.yaml").as_deref(),
        Some("mini version\n")
    );
}

/// The manifest is the index; the per-file mirror is derived, and the two are
/// compared on demand.
#[test]
fn the_index_and_its_mirror_are_compared_on_demand() {
    let fixture = Fixture::new();
    fixture.ok(&fixture.dev, &["publish"]);
    let report = fixture.ok(&fixture.dev, &["verify-index"]);
    assert_eq!(report["manifest_files"], 2);
    assert_eq!(report["pointer_files"], 2);
    assert_eq!(report["drift_count"], 0);

    let pointers = fixture.ok(&fixture.dev, &["status", "--pointers"]);
    assert_eq!(pointers["index_source"], "pointers");
    assert_eq!(state_of(&pointers, "work-ledger.yaml"), "in_sync");
}

/// Handoffs are the request and receipt channel between agents: write-once,
/// idempotent, and never handed back to the host that wrote them.
#[test]
fn a_handoff_travels_one_way_and_replays_idempotently() {
    let fixture = Fixture::new();
    let request = fixture.base.join("request.json");
    fs::write(
        &request,
        "{\"summary\":\"install postgres\",\"to\":\"mac-mini\"}",
    )
    .unwrap();

    let first = fixture.ok(
        &fixture.dev,
        &[
            "handoff",
            "push",
            "--file",
            request.to_str().unwrap(),
            "--name",
            "postgres-up",
        ],
    );
    assert_eq!(first["idempotent_replay"], false);
    let replay = fixture.ok(
        &fixture.dev,
        &[
            "handoff",
            "push",
            "--file",
            request.to_str().unwrap(),
            "--name",
            "postgres-up",
        ],
    );
    assert_eq!(replay["idempotent_replay"], true);

    // The author never reads its own handoff back.
    let mine = fixture.ok(
        &fixture.dev,
        &[
            "handoff",
            "pull",
            "--outdir",
            fixture.dev.join("inbox").to_str().unwrap(),
        ],
    );
    assert_eq!(mine["inbound_count"], 0);

    let outdir = fixture.mini.join("inbox");
    let inbound = fixture.ok(
        &fixture.mini,
        &["handoff", "pull", "--outdir", outdir.to_str().unwrap()],
    );
    assert_eq!(inbound["inbound_count"], 1);
    assert_eq!(inbound["handoffs"][0]["summary"], "install postgres");
    assert_eq!(
        fs::read_to_string(outdir.join("macbook-codex/postgres-up.json")).unwrap(),
        "{\"summary\":\"install postgres\",\"to\":\"mac-mini\"}"
    );
    // Unchanged bytes are not fetched a second time.
    let again = fixture.ok(
        &fixture.mini,
        &["handoff", "pull", "--outdir", outdir.to_str().unwrap()],
    );
    assert_eq!(again["inbound_count"], 0);
}

/// Credentials are stored per machine, with owner-only permissions, and no
/// command ever echoes them.
#[test]
fn credentials_are_stored_off_the_command_line_and_never_echoed() {
    let fixture = Fixture::new();
    let input = fixture.base.join("credentials.json");
    fs::write(
        &input,
        "{\"access_key\":\"AKIAIOSFODNN7EXAMPLE\",\"secret_key\":\"synthetic-secret-value\"}",
    )
    .unwrap();

    let stored = fixture.ok(
        &fixture.dev,
        &["credential", "set", "--input", input.to_str().unwrap()],
    );
    assert_eq!(stored["present"], true);
    assert_eq!(stored["complete"], true);
    assert_eq!(stored["session_token"], false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = fixture.dev.join(".awr/workspace-credentials.json");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let status = fixture.ok(&fixture.dev, &["credential", "status"]);
    // Unix reports owner-only mode; Windows leaves mode unset and relies on directory ACL.
    #[cfg(unix)]
    assert_eq!(status["mode"], "600");
    #[cfg(not(unix))]
    assert!(
        status.get("mode").is_none() || status["mode"].is_null(),
        "{status}"
    );
    let rendered = status.to_string();
    for secret in ["AKIAIOSFODNN7EXAMPLE", "synthetic-secret-value"] {
        assert!(!rendered.contains(secret), "status echoed {secret}");
    }

    let cleared = fixture.ok(&fixture.dev, &["credential", "clear"]);
    assert_eq!(cleared["removed"], true);
    assert_eq!(
        fixture.ok(&fixture.dev, &["credential", "clear"])["removed"],
        false
    );
}

/// A key pasted into a config, or typed as an argument, is refused before it
/// can be used - and the refusal does not repeat the value.
#[test]
fn a_key_in_a_config_or_an_argument_is_refused_without_being_repeated() {
    let fixture = Fixture::new();
    let config = fixture.dev.join("remote_workspace.toml");
    let mut body = fs::read_to_string(&config).unwrap();
    body.push_str("ak = \"AKIAIOSFODNN7EXAMPLE\"\n");
    fs::write(&config, body).unwrap();

    let (_, report) = fixture.refused(&fixture.dev, &["status"]);
    assert_eq!(report["code"], "InvalidInput");
    let message = report["message"].as_str().unwrap();
    assert!(
        message.contains("awr workspace credential set"),
        "{message}"
    );
    assert!(!message.contains("AKIAIOSFODNN7EXAMPLE"), "{message}");

    // The pre-parse guard rejects a secret in argv whatever the subcommand is.
    fs::write(
        &config,
        fs::read_to_string(&config)
            .unwrap()
            .replace("ak = \"AKIAIOSFODNN7EXAMPLE\"\n", ""),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args([
            "workspace",
            "credential",
            "set",
            "--input",
            "AKIAIOSFODNN7EXAMPLE",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(report["code"], "RuleViolation");
}

/// A config is a small, explicit file: a typo is an error rather than a
/// silently different machine.
#[test]
fn a_config_typo_and_an_unknown_backend_are_both_errors() {
    let fixture = Fixture::new();
    let config = fixture.dev.join("remote_workspace.toml");
    let body = fs::read_to_string(&config).unwrap();

    // An object store spelled with a typo, an unknown backend, a misspelt key
    // and a missing section are all refused, each naming what it could not use.
    let with_endpoint = "endpoint = \"oss-cn-beijing.aliyuncs.com\"\nbucket = \"b\"\n";
    for (broken, expected) in [
        (
            format!("{with_endpoint}endpont = \"oss-cn-beijing.aliyuncs.com\"\n{body}"),
            "InvalidInput",
        ),
        (
            body.replace("[store]", "[store]\nbackend = \"gcs\""),
            "InvalidInput",
        ),
        (body.replace("key   =", "keey  ="), "InvalidInput"),
        (body.replace("[store]", "[nothing]"), "InvalidInput"),
    ] {
        fs::write(&config, broken).unwrap();
        let (_, report) = fixture.refused(&fixture.dev, &["status"]);
        assert_eq!(report["code"], expected, "{report}");
    }

    // A missing config is named rather than defaulted away.
    fs::write(&config, body).unwrap();
    let (_, report) = fixture.refused(
        &fixture.dev,
        &[
            "status",
            "--config",
            fixture.dev.join("absent.toml").to_str().unwrap(),
        ],
    );
    assert_eq!(report["code"], "SourceUnavailable");
    assert!(report["message"].as_str().unwrap().contains("absent.toml"));
}

/// The lifecycle hook is where a peer's work is noticed: a session that is
/// starting takes what the other machine published, and says so.
#[test]
fn a_session_start_takes_what_the_peer_published() {
    let fixture = Fixture::new();
    fixture.init(&fixture.dev);
    // The peer takes the ledger, does its work elsewhere, and publishes a new
    // evidence file together with the ledger it updated.
    fixture.ok(&fixture.dev, &["publish"]);
    fixture.ok(&fixture.mini, &["sync"]);
    fixture.write(
        &fixture.mini,
        "infra/evidence/install.json",
        "{\"database\":\"ready\"}\n",
    );
    let ledger = fixture.read(&fixture.mini, "work-ledger.yaml").unwrap();
    fixture.write(
        &fixture.mini,
        "work-ledger.yaml",
        &ledger.replace("Publish this host's tracked files.", "Record the install."),
    );
    fixture.ok(&fixture.mini, &["publish"]);
    assert_eq!(
        fixture.read(&fixture.dev, "infra/evidence/install.json"),
        None
    );

    let hook = fixture.session_start(&fixture.dev);
    let context = context_of(&hook);
    assert!(
        context.contains("The workspace moved 2 file(s)"),
        "{context}"
    );
    assert!(
        context.contains("  infra/evidence/install.json"),
        "{context}"
    );
    assert!(context.contains("  work-ledger.yaml"), "{context}");
    assert_eq!(
        fixture
            .read(&fixture.dev, "infra/evidence/install.json")
            .as_deref(),
        Some("{\"database\":\"ready\"}\n")
    );
    // A pulled file the project also indexes does not spoil the session it
    // arrived in: the ledger reaches this host and the context still renders.
    assert!(
        fixture
            .read(&fixture.dev, "work-ledger.yaml")
            .unwrap()
            .contains("Record the install.")
    );

    // A session that starts with nothing to collect says nothing about the
    // workspace rather than repeating that it is empty.
    let quiet = fixture.session_start(&fixture.dev);
    assert!(!context_of(&quiet).contains("The workspace moved"));
}

/// A conflict found at session start is reported and left alone: a hook that
/// resolved it would be guessing, and one that failed would close the session.
#[test]
fn a_session_start_reports_a_conflict_without_failing() {
    let fixture = Fixture::new();
    fixture.init(&fixture.dev);
    fixture.ok(&fixture.dev, &["publish"]);
    fixture.ok(&fixture.mini, &["sync"]);

    // A file the two machines both edit: not a work source, so the only thing
    // that can refuse it is the exchange plane's own rule.
    fixture.write(&fixture.dev, "infra/evidence/dump.json", "dev version\n");
    fixture.write(&fixture.mini, "infra/evidence/dump.json", "mini version\n");
    fixture.ok(&fixture.mini, &["publish"]);

    let context = fixture.session_start(&fixture.dev);
    let context = context_of(&context).to_string();
    assert!(context.contains("changed on this host"), "{context}");
    assert!(context.contains("infra/evidence/dump.json"), "{context}");
    assert_eq!(
        fixture
            .read(&fixture.dev, "infra/evidence/dump.json")
            .as_deref(),
        Some("dev version\n")
    );
}

/// Without a config the hook is silent: a project that does not use the
/// exchange plane must not pay for it, or be told about it at every start.
#[test]
fn a_session_start_without_a_config_says_nothing_about_a_workspace() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.mini.join("remote_workspace.toml")).unwrap();
    // The same project, on a machine that never configured a store.
    let ledger = fixture.read(&fixture.dev, "work-ledger.yaml").unwrap();
    fixture.write(&fixture.mini, "work-ledger.yaml", &ledger);
    fixture.init(&fixture.mini);
    let context = fixture.session_start(&fixture.mini);
    let context = context_of(&context);
    assert!(context.contains("SHARE-001"), "{context}");
    assert!(!context.contains("workspace"), "{context}");
}

/// A machine that is configured but cannot reach the store still opens its
/// session; the failure is something to read, not a session that will not run.
#[test]
fn a_session_start_survives_a_store_that_cannot_be_used() {
    let fixture = Fixture::new();
    fixture.init(&fixture.dev);
    fs::write(
        fixture.dev.join("remote_workspace.toml"),
        format!(
            "[project]\nkey   = \"poc-infra\"\nroot  = {}\nhost  = \"macbook-codex\"\ntrack = [\"work-ledger.yaml\", \"infra\"]\n\n[store]\nendpoint = \"https://workspace.invalid\"\nbucket   = \"unreachable\"\n",
            serde_json::to_string(&fixture.dev.display().to_string()).unwrap(),
        ),
    )
    .unwrap();

    let context = fixture.session_start(&fixture.dev);
    let context = context_of(&context).to_string();
    assert!(context.contains("did not run"), "{context}");
    assert!(context.contains("credential"), "{context}");
}

/// A handoff an agent wrote on the other machine is part of what a session
/// starts with: it is collected and named, not left for someone to remember.
#[test]
fn a_session_start_collects_an_inbound_handoff() {
    let fixture = Fixture::new();
    fixture.init(&fixture.dev);
    let request = fixture.base.join("handoff.json");
    fs::write(&request, "{\"summary\":\"postgres is up\"}\n").unwrap();
    fixture.ok(
        &fixture.mini,
        &[
            "handoff",
            "push",
            "--file",
            request.to_str().unwrap(),
            "--name",
            "postgres-up",
        ],
    );

    let context = fixture.session_start(&fixture.dev);
    let context = context_of(&context).to_string();
    assert!(context.contains("1 inbound handoff(s)"), "{context}");
    assert!(context.contains("mac-mini/postgres-up.json"), "{context}");
    assert_eq!(
        fixture
            .read(&fixture.dev, "infra/handoffs/mac-mini/postgres-up.json")
            .as_deref(),
        Some("{\"summary\":\"postgres is up\"}\n")
    );
}

/// The loss a store without compare-and-swap cannot prevent is repaired where
/// the session starts, so nobody has to remember to publish again.
#[test]
fn a_session_start_registers_again_an_entry_the_index_dropped() {
    let fixture = Fixture::new();
    fixture.init(&fixture.dev);
    fixture.ok(&fixture.dev, &["publish"]);

    // A peer committed from a stale read and the entry went with it. This is
    // exactly what the guarded commit cannot rule out, only detect.
    let manifest = fixture.store.join("projects/poc-infra/manifest.json");
    let mut index: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    index["files"]
        .as_object_mut()
        .unwrap()
        .remove("infra/evidence/dump.json");
    fs::write(&manifest, serde_json::to_vec(&index).unwrap()).unwrap();

    let context = fixture.session_start(&fixture.dev);
    let context = context_of(&context).to_string();
    assert!(context.contains("dropped out of the index"), "{context}");
    assert!(context.contains("infra/evidence/dump.json"), "{context}");

    let repaired: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert!(
        repaired["files"].get("infra/evidence/dump.json").is_some(),
        "{repaired}"
    );
    assert_eq!(
        state_of(
            &fixture.ok(&fixture.dev, &["status"]),
            "infra/evidence/dump.json"
        ),
        "in_sync"
    );
    // A session that starts healthy says nothing about the index.
    let quiet = fixture.session_start(&fixture.dev);
    assert!(!context_of(&quiet).contains("dropped out of the index"));
}

/// The pull runs before the project's sources are refreshed, so a source the
/// peer broke stops the session at AWR's own guard in the session it arrived
/// in - and a peer that fixes it repairs this host without anyone copying files
/// by hand.
#[test]
fn a_source_the_peer_broke_is_repaired_when_the_peer_fixes_it() {
    let fixture = Fixture::new();
    fixture.init(&fixture.dev);
    fixture.ok(&fixture.dev, &["publish"]);
    fixture.ok(&fixture.mini, &["sync"]);

    let good = fixture.read(&fixture.mini, "work-ledger.yaml").unwrap();
    fixture.write(
        &fixture.mini,
        "work-ledger.yaml",
        &good.replace("    priority: P1\n", "    priority: [\n"),
    );
    fixture.ok(&fixture.mini, &["publish"]);

    let out = fixture.session_start_output(&fixture.dev);
    assert!(
        !out.status.success(),
        "a broken source must stop the session"
    );
    let report: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(report["code"], "SourceStale");
    assert!(
        fixture
            .read(&fixture.dev, "work-ledger.yaml")
            .unwrap()
            .contains("priority: [")
    );

    // The peer publishes a parseable one again; the next session opens on it.
    fixture.write(&fixture.mini, "work-ledger.yaml", &good);
    fixture.ok(&fixture.mini, &["publish"]);
    let context = fixture.session_start(&fixture.dev);
    assert!(context_of(&context).contains("work-ledger.yaml"));
    assert!(
        fixture
            .read(&fixture.dev, "work-ledger.yaml")
            .unwrap()
            .contains("priority: P1")
    );
}

/// Drop refuses a path still covered by track, and the index is unchanged.
#[test]
fn drop_refuses_a_path_still_in_track() {
    let fixture = Fixture::new();
    fixture.ok(&fixture.dev, &["publish"]);
    let (code, report) = fixture.refused(
        &fixture.dev,
        &["drop", "--path", "infra/evidence/dump.json"],
    );
    assert_eq!(code, 1);
    assert_eq!(report["code"], "InvalidInput");
    let message = report["message"].as_str().unwrap_or("");
    assert!(message.contains("project.track"), "{report}");
    let manifest: Value = serde_json::from_slice(
        &fs::read(fixture.store.join("projects/poc-infra/manifest.json")).unwrap(),
    )
    .unwrap();
    assert!(
        manifest["files"].get("infra/evidence/dump.json").is_some(),
        "{manifest}"
    );
}

/// After the path leaves track, drop removes it from the index. The local file
/// stays, a peer sync does not restore it, and the pointer mirror matches.
#[test]
fn drop_after_untrack_does_not_restore_on_the_peer() {
    let fixture = Fixture::new();
    fixture.ok(&fixture.dev, &["publish"]);
    fixture.ok(&fixture.mini, &["sync"]);
    let original = fixture
        .read(&fixture.dev, "infra/evidence/dump.json")
        .unwrap();

    fixture.configure_track(&fixture.dev, "macbook-codex", &["work-ledger.yaml"]);
    let report = fixture.ok(
        &fixture.dev,
        &["drop", "--path", "infra/evidence/dump.json"],
    );
    assert_eq!(
        report["dropped"],
        serde_json::json!(["infra/evidence/dump.json"])
    );
    assert_eq!(
        fixture
            .read(&fixture.dev, "infra/evidence/dump.json")
            .as_deref(),
        Some(original.as_str())
    );

    let verified = fixture.ok(&fixture.dev, &["verify-index"]);
    assert_eq!(verified["drift_count"], 0);
    assert_eq!(verified["manifest_files"], 1);
    assert_eq!(verified["pointer_files"], 1);

    fs::remove_file(fixture.mini.join("infra/evidence/dump.json")).unwrap();
    fixture.ok(&fixture.mini, &["sync"]);
    assert!(
        fixture
            .read(&fixture.mini, "infra/evidence/dump.json")
            .is_none(),
        "a dropped path must not come back from the workspace"
    );
}

#[test]
fn workspace_help_lists_drop() {
    let group = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["help", "environments"])
        .output()
        .unwrap();
    assert!(group.status.success());
    let group = String::from_utf8_lossy(&group.stdout);
    assert!(group.contains("workspace"), "{group}");
    let out = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["workspace", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("drop"), "{help}");
    assert!(help.contains("publish"), "{help}");
    assert!(help.contains("credential"), "{help}");
}

/// A path under `.git` in the index is remote data. Sync must refuse it and
/// leave the local tree untouched.
#[test]
fn a_private_path_in_the_index_is_not_written_by_sync() {
    let fixture = Fixture::new();
    let manifest = fixture.store.join("projects/poc-infra/manifest.json");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(
        &manifest,
        br#"{"schema":1,"project_key":"poc-infra","files":{".git/hooks/post-checkout":{"sha256":"abc","size":1}}}"#,
    )
    .unwrap();

    let hook = fixture.mini.join(".git/hooks/post-checkout");
    let (code, report) = fixture.refused(&fixture.mini, &["sync"]);
    assert_eq!(code, 1);
    assert_eq!(report["code"], "RuleViolation");
    let message = report["message"].as_str().unwrap_or("");
    assert!(message.contains("never leaves a machine"), "{report}");
    assert!(
        !hook.exists(),
        "sync must not write a Git hook from the index"
    );
}

#[cfg(unix)]
#[test]
fn symlink_publish_is_refused_at_cli_and_does_not_leak() {
    let fixture = Fixture::new();
    let outside = fixture.base.join("outside-secret.env");
    fs::write(&outside, b"CLI-SECRET-TOKEN").unwrap();
    let link = fixture.dev.join("linked-secret.txt");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    fixture.configure_track(
        &fixture.dev,
        "macbook-codex",
        &["work-ledger.yaml", "linked-secret.txt"],
    );
    let (code, report) = fixture.refused(&fixture.dev, &["publish"]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["code"], "RuleViolation", "{report}");
    let message = report["message"].as_str().unwrap_or("");
    assert!(message.contains("symbolic link"), "{report}");
    assert!(
        !walk_store_contains(&fixture.store, b"CLI-SECRET-TOKEN"),
        "CLI publish must not upload symlink targets"
    );
}

#[test]
fn handoff_push_rejects_slash_name_at_cli() {
    let fixture = Fixture::new();
    fixture.ok(&fixture.dev, &["publish"]);
    let note = fixture.dev.join("note.json");
    fs::write(&note, br#"{"summary":"x"}"#).unwrap();
    let (code, report) = fixture.refused(
        &fixture.dev,
        &[
            "handoff",
            "push",
            "--file",
            note.to_str().unwrap(),
            "--name",
            "foo/bar",
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(report["code"], "RuleViolation");
    let message = report["message"].as_str().unwrap_or("");
    assert!(
        message.contains("path component") || message.contains("foo/bar"),
        "{report}"
    );
}

#[test]
fn publish_fails_closed_when_content_object_is_forged() {
    let fixture = Fixture::new();
    fixture.configure_track(&fixture.dev, "macbook-codex", &["work-ledger.yaml"]);
    fixture.write(&fixture.dev, "work-ledger.yaml", "v1\n");
    fixture.ok(&fixture.dev, &["publish"]);

    // Forge the content object that currently matches local bytes.
    let mut corrupted = false;
    let files_root = fixture.store.join("projects/poc-infra/files");
    if let Ok(walk) = fs::read_dir(&files_root) {
        'outer: for entry in walk.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(objs) = fs::read_dir(&path) {
                for obj in objs.flatten() {
                    let p = obj.path();
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if name == "current.json" || !p.is_file() {
                        continue;
                    }
                    let local = fixture.read(&fixture.dev, "work-ledger.yaml").unwrap();
                    if fs::read(&p).ok().as_deref() == Some(local.as_bytes()) {
                        fs::write(&p, b"FORGED-CLI-BYTES").unwrap();
                        corrupted = true;
                        break 'outer;
                    }
                }
            }
        }
    }
    assert!(corrupted, "expected to find a content object to forge");

    // Drop index entries so publish tries to re-register local bytes against the forged object.
    let manifest = fixture.store.join("projects/poc-infra/manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["files"].as_object_mut().unwrap().clear();
    fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let (code, report) = fixture.refused(&fixture.dev, &["publish"]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["code"], "SourceUnavailable", "{report}");
    let message = report["message"].as_str().unwrap_or("");
    assert!(
        message.contains("already holds") || message.contains("content object"),
        "{report}"
    );
}

#[test]
fn http_endpoint_outside_loopback_is_refused_at_cli() {
    let fixture = Fixture::new();
    fs::write(
        fixture.dev.join("remote_workspace.toml"),
        format!(
            "[project]\nkey = \"poc-infra\"\nroot = {}\nhost = \"macbook-codex\"\ntrack = [\"work-ledger.yaml\"]\n\n[store]\nendpoint = \"http://oss-cn-beijing.aliyuncs.com\"\nbucket = \"x\"\n",
            serde_json::to_string(&fixture.dev.display().to_string()).unwrap(),
        ),
    )
    .unwrap();
    let (code, report) = fixture.refused(&fixture.dev, &["status"]);
    assert_eq!(code, 1);
    assert_eq!(report["code"], "InvalidInput");
    let message = report["message"].as_str().unwrap_or("");
    assert!(message.contains("https"), "{report}");
}

#[test]
fn malformed_pointer_fails_closed_at_cli_status() {
    let fixture = Fixture::new();
    let pointer = fixture
        .store
        .join("projects/poc-infra/files/work-ledger.yaml/current.json");
    fs::create_dir_all(pointer.parent().unwrap()).unwrap();
    fs::write(&pointer, b"{not-json").unwrap();
    let (code, report) = fixture.refused(&fixture.dev, &["status", "--pointers"]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["code"], "SourceUnavailable", "{report}");
    let message = report["message"].as_str().unwrap_or("");
    assert!(
        message.contains("pointer") || message.contains("JSON") || message.contains("valid"),
        "{report}"
    );
}

#[test]
fn credential_set_roundtrip_never_echoes_secret() {
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args([
            "--project",
            fixture.dev.to_str().unwrap(),
            "--json",
            "workspace",
            "credential",
            "set",
            "--stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        stdin
            .write_all(br#"{"access_key":"AKIA_TEST","secret_key":"super-secret-value"}"#)
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = fixture.ok(&fixture.dev, &["credential", "status"]);
    let rendered = status.to_string();
    assert!(!rendered.contains("super-secret-value"), "{rendered}");
    assert_eq!(status["complete"], true);
}

#[cfg(unix)]
#[test]
fn credential_file_mode_is_600() {
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args([
            "--project",
            fixture.dev.to_str().unwrap(),
            "workspace",
            "credential",
            "set",
            "--stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(br#"{"access_key":"AKIA_TEST","secret_key":"secret"}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    use std::os::unix::fs::PermissionsExt;
    let path = fixture.dev.join(".awr/workspace-credentials.json");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "mode={mode:o}");
}

fn walk_store_contains(root: &Path, needle: &[u8]) -> bool {
    fn walk(path: &Path, needle: &[u8]) -> bool {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return false;
        };
        if meta.file_type().is_symlink() {
            return false;
        }
        if meta.is_file() {
            return fs::read(path)
                .ok()
                .is_some_and(|body| body.windows(needle.len()).any(|w| w == needle));
        }
        if meta.is_dir() {
            if let Ok(rd) = fs::read_dir(path) {
                for entry in rd.flatten() {
                    if walk(&entry.path(), needle) {
                        return true;
                    }
                }
            }
        }
        false
    }
    walk(root, needle)
}
