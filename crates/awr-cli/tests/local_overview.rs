use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    default_scope: bool,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-hub-cli-{}", awr_core::Id::new()));
        std::fs::create_dir(&root).unwrap();
        Self {
            root,
            default_scope: false,
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_awr"));
        c.env("AWR_LOCAL_WEB", "0");
        if self.default_scope {
            c.env_remove("AWR_LOCAL_HOME")
                .env_remove("XDG_STATE_HOME")
                .env("HOME", &self.root)
                .env("USERPROFILE", &self.root)
                .env("LOCALAPPDATA", self.root.join("AppData"));
        } else {
            c.env("AWR_LOCAL_HOME", self.root.join("user-state"));
        }
        c
    }
    fn run(&self, args: &[&str]) -> Value {
        let output = self.command().args(args).output().unwrap();
        success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn project(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::create_dir(&path).unwrap();
        let output = self
            .command()
            .arg("--project")
            .arg(&path)
            .args([
                "init",
                "--goal",
                "Deliver a synthetic application with verifiable progress",
                "--accept",
                "--json",
            ])
            .output()
            .unwrap();
        success(&output);
        path
    }
    fn register(&self, path: &Path) -> Value {
        let output = self
            .command()
            .args(["overview", "register"])
            .arg(path)
            .arg("--json")
            .output()
            .unwrap();
        success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn get(&self, url: &str) -> Value {
        let mut response = ureq::Agent::config_builder()
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .new_agent()
            .get(url)
            .call()
            .unwrap();
        serde_json::from_str(&response.body_mut().read_to_string().unwrap()).unwrap()
    }
    fn enroll(&self, root: &Path) -> Value {
        let base = self.run(&["overview", "start", "--json"])["service"]["url"]
            .as_str()
            .unwrap()
            .to_owned();
        let body = serde_json::json!({"root":root}).to_string();
        let mut response = ureq::post(format!("{base}/api/projects/intake"))
            .header("content-type", "application/json")
            .header("x-awr-project-hub", "1")
            .send(body.as_bytes())
            .unwrap();
        serde_json::from_str(&response.body_mut().read_to_string().unwrap()).unwrap()
    }
    fn refresh(&self) -> Value {
        let base = self.run(&["overview", "status", "--json"])["service"]["url"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut response = ureq::post(format!("{base}/api/projects/refresh"))
            .header("x-awr-project-hub", "1")
            .send_empty()
            .unwrap();
        serde_json::from_str(&response.body_mut().read_to_string().unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.command().args(["overview", "stop"]).output();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn ordinary_calls_register_projects_and_concurrent_startup_reuses_one_service() {
    let f = Fixture::new();
    let a = f.project("alpha");
    let jobs: Vec<_> = (0..12)
        .map(|_| {
            let mut command = f.command();
            command.env("AWR_LOCAL_WEB", "1").args(["--version"]);
            std::thread::spawn(move || command.output().unwrap())
        })
        .collect();
    for job in jobs {
        let output = job.join().unwrap();
        success(&output);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            format!("awr {}", env!("CARGO_PKG_VERSION"))
        );
    }
    let first = f.run(&["overview", "status", "--json"])["service"].clone();
    assert!(first["pid"].as_u64().is_some());
    let output = f
        .command()
        .env("AWR_LOCAL_WEB", "1")
        .arg("--project")
        .arg(&a)
        .args(["status", "--cached", "--json"])
        .output()
        .unwrap();
    success(&output);
    let entries = f.run(&["overview", "projects", "--json"]);
    assert_eq!(entries["projects"].as_array().unwrap().len(), 1);
    assert_eq!(
        entries["projects"][0]["root"],
        a.canonicalize().unwrap().to_str().unwrap()
    );
    let second = f.run(&["overview", "start", "--json"])["service"].clone();
    assert_eq!(first["pid"], second["pid"]);
    assert_eq!(first["instance"], second["instance"]);
    let duplicate = f.command().args(["overview", "serve"]).output().unwrap();
    assert!(!duplicate.status.success());
    assert_eq!(
        f.run(&["overview", "status", "--json"])["service"]["pid"],
        first["pid"]
    );
}

fn wait_for_projects(f: &Fixture, count: usize) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    loop {
        let value = f.run(&["overview", "projects", "--json"]);
        if value["projects"].as_array().unwrap().len() == count
            && value["discovery"]["state"] == "complete"
        {
            return value;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "project discovery did not converge: {value}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn first_use_discovers_existing_projects_in_the_default_user_scope_without_registration() {
    let mut f = Fixture::new();
    std::fs::write(f.root.join("package.json"), "{}").unwrap();
    let existing = f.project("existing");
    let repository = f.root.join("repository");
    std::fs::create_dir_all(repository.join(".git")).unwrap();
    f.default_scope = true;
    let output = f
        .command()
        .env("AWR_LOCAL_WEB", "1")
        .args(["--version"])
        .output()
        .unwrap();
    success(&output);
    let projects = wait_for_projects(&f, 1);
    assert_eq!(projects["catalog_scope"], "user");
    let entries = projects["projects"].as_array().unwrap();
    assert!(entries.iter().any(|entry| entry["root"]
        == existing.canonicalize().unwrap().to_str().unwrap()
        && entry["state"] == "connected"));
    assert!(
        !entries
            .iter()
            .any(|entry| entry["root"] == repository.canonicalize().unwrap().to_str().unwrap())
    );
    assert!(!repository.join(".awr").exists());
}

#[test]
fn startup_discovery_stays_idle_until_a_manual_refresh_and_preserves_source_files() {
    let f = Fixture::new();
    let existing = f.project("existing");
    let source = existing.join(".awr/project.toml");
    let before = std::fs::read(&source).unwrap();
    let initial = f.run(&[
        "overview",
        "discover",
        "--root",
        f.root.to_str().unwrap(),
        "--interval-seconds",
        "1",
        "--json",
    ]);
    assert_eq!(initial["discovery"]["connected"], 1);
    let base = initial["service"]["url"].as_str().unwrap();
    let startup = f.get(&format!("{base}/api/projects"));
    let repository = f.project("new-repository");
    std::thread::sleep(Duration::from_millis(1300));
    let idle = f.run(&["overview", "projects", "--json"]);
    assert_eq!(idle["projects"].as_array().unwrap().len(), 1);
    assert_eq!(
        idle["discovery"]["completed_at"],
        startup["data"]["discovery"]["completed_at"]
    );
    assert_eq!(f.refresh()["data"]["projects"].as_array().unwrap().len(), 2);
    let observed = wait_for_projects(&f, 2);
    assert_eq!(observed["discovery"]["connected"], 2);
    assert_eq!(std::fs::read(source).unwrap(), before);
    assert!(repository.join(".awr/state.db").exists());
    let instance = initial["service"]["instance"].clone();
    assert_eq!(
        f.run(&["overview", "status", "--json"])["service"]["instance"],
        instance
    );
}

#[test]
fn ordinary_calls_and_explicit_intake_do_not_enroll_temporary_session_projects() {
    let f = Fixture::new();
    let parent = f.root.join("Codex/2026-09-29/chat/work");
    std::fs::create_dir_all(&parent).unwrap();
    let temporary = f.project("Codex/2026-09-29/chat/work/project");
    let output = f
        .command()
        .env("AWR_LOCAL_WEB", "1")
        .arg("--project")
        .arg(&temporary)
        .args(["status", "--cached", "--json"])
        .output()
        .unwrap();
    success(&output);
    let catalog = f.run(&["overview", "projects", "--json"]);
    assert!(catalog["projects"].as_array().unwrap().is_empty());
    let discovered = f.run(&[
        "overview",
        "discover",
        "--root",
        f.root.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(discovered["discovery"]["temporary_directories_skipped"], 1);
    let base = discovered["service"]["url"].as_str().unwrap();
    let body = serde_json::json!({"root":temporary}).to_string();
    let result = ureq::post(format!("{base}/api/projects/intake"))
        .header("content-type", "application/json")
        .header("x-awr-project-hub", "1")
        .send(body.as_bytes());
    assert!(matches!(result, Err(ureq::Error::StatusCode(400))));
    assert!(
        f.refresh()["data"]["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(temporary.join(".awr/state.db").is_file());
}

#[test]
fn manual_refresh_adopts_updated_scan_roots_without_restarting_the_service() {
    let f = Fixture::new();
    let started = f.run(&["overview", "start", "--json"])["service"].clone();
    f.run(&[
        "overview",
        "discover",
        "--root",
        f.root.to_str().unwrap(),
        "--interval-seconds",
        "1",
        "--json",
    ]);
    f.get(&format!(
        "{}/api/projects",
        started["url"].as_str().unwrap()
    ));
    f.project("added");
    f.refresh();
    let observed = wait_for_projects(&f, 1);
    assert_eq!(observed["discovery"]["connected"], 1);
    let current = f.run(&["overview", "status", "--json"])["service"].clone();
    assert_eq!(current["instance"], started["instance"]);
}

#[test]
fn explicit_enrollment_creates_a_pending_card_and_initialization_promotes_the_same_identity() {
    let f = Fixture::new();
    let root = f.root.join("application");
    std::fs::create_dir(&root).unwrap();
    let output = f
        .command()
        .env("AWR_LOCAL_WEB", "1")
        .arg("--project")
        .arg(&root)
        .args(["status", "--cached", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        f.run(&["overview", "projects", "--json"])["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let pending = f.enroll(&root)["data"]["project"].clone();
    assert_eq!(pending["state"], "discovered");
    assert_eq!(pending["intake_requested"], true);
    assert!(!root.join(".awr").exists());
    let intake = f.run(&[
        "--project",
        root.to_str().unwrap(),
        "overview",
        "inspect",
        "--json",
    ]);
    assert_eq!(intake["project"]["state"], "discovered");
    assert_eq!(intake["guidance"]["basis"]["goal_inferred"], false);
    assert_eq!(intake["guidance"]["next_action"]["argv"][3], "init");
    let base = f.run(&["overview", "status", "--json"])["service"]["url"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!(
        "{base}/api/projects/{}/intake",
        pending["key"].as_str().unwrap()
    );
    assert_eq!(f.get(&url)["data"]["project"]["key"], pending["key"]);
    assert!(!root.join(".awr").exists());
    let output = f
        .command()
        .env("AWR_LOCAL_WEB", "1")
        .arg("--project")
        .arg(&root)
        .args([
            "init",
            "--goal",
            "Deliver a documented application with current acceptance evidence",
            "--accept",
            "--json",
        ])
        .output()
        .unwrap();
    success(&output);
    let entries = f.run(&["overview", "projects", "--json"]);
    assert_eq!(entries["projects"].as_array().unwrap().len(), 1);
    assert_eq!(entries["projects"][0]["key"], pending["key"]);
    assert_eq!(entries["projects"][0]["state"], "connected");
    assert!(entries["projects"][0]["project_id"].as_str().is_some());
    let connected = f.get(&url);
    assert_eq!(connected["data"]["project"]["key"], pending["key"]);
    assert_eq!(connected["data"]["project"]["state"], "connected");
    assert_eq!(
        connected["data"]["guidance"]["next_action"]["argv"][3],
        "status"
    );
}

#[test]
fn hidden_cards_stay_hidden_after_use_rescan_and_restart_until_explicitly_enrolled() {
    let f = Fixture::new();
    let root = f.project("managed");
    let registered = f.register(&root);
    f.run(&[
        "overview",
        "discover",
        "--root",
        f.root.to_str().unwrap(),
        "--json",
    ]);
    let key = registered["projects"][0]["key"].as_str().unwrap();
    let base = registered["service"]["url"].as_str().unwrap();
    let guarded = ureq::delete(format!("{base}/api/projects/{key}"))
        .header("x-awr-project-hub", "1")
        .call()
        .unwrap();
    assert_eq!(guarded.status(), 200);
    assert!(
        f.refresh()["data"]["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.register(&root);
    assert!(
        f.run(&["overview", "projects", "--json"])["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.run(&["overview", "stop", "--json"]);
    let restarted = f.run(&["overview", "start", "--json"]);
    let observed = f.get(&format!(
        "{}/api/projects",
        restarted["service"]["url"].as_str().unwrap()
    ));
    assert!(observed["data"]["projects"].as_array().unwrap().is_empty());
    let restored = f.enroll(&root);
    assert_eq!(restored["data"]["project"]["key"], key);
    assert_eq!(
        f.run(&["overview", "projects", "--json"])["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn deletion_and_manual_discovery_require_local_request_guards() {
    let f = Fixture::new();
    let root = f.project("managed");
    let registered = f.register(&root);
    let base = registered["service"]["url"].as_str().unwrap();
    let key = registered["projects"][0]["key"].as_str().unwrap();
    for guarded in [false, true] {
        let mut remove = ureq::delete(format!("{base}/api/projects/{key}"));
        let mut refresh = ureq::post(format!("{base}/api/projects/refresh"));
        if guarded {
            remove = remove
                .header("x-awr-project-hub", "1")
                .header("origin", "https://untrusted.example");
            refresh = refresh
                .header("x-awr-project-hub", "1")
                .header("origin", "https://untrusted.example");
        }
        assert!(matches!(remove.call(), Err(ureq::Error::StatusCode(403))));
        assert!(matches!(
            refresh.send_empty(),
            Err(ureq::Error::StatusCode(403))
        ));
    }
    assert_eq!(
        f.get(&format!("{base}/api/projects"))["data"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn each_project_snapshot_reads_newly_committed_awr_state() {
    let f = Fixture::new();
    let root = f.project("managed");
    let registered = f.register(&root);
    let base = registered["service"]["url"].as_str().unwrap();
    let key = registered["projects"][0]["key"].as_str().unwrap();
    let url = format!("{base}/api/projects/{key}/map/snapshot");
    let before = f.get(&url);
    let mut store = awr_store::Store::open(&root.join(".awr/state.db")).unwrap();
    let project = store
        .project_by_root(&root.canonicalize().unwrap())
        .unwrap();
    let (started, _) = store
        .start_session(
            project.id,
            project.project_revision,
            awr_core::SessionDraft {
                work_item_key: None,
                agent_id: "freshness-fixture".into(),
                provider: "fixture".into(),
                model: "fixture".into(),
                branch_id: None,
                claim: false,
                claim_ttl_ms: None,
            },
        )
        .unwrap();
    drop(store);
    let after = f.get(&url);
    assert!(
        after["data"]["project_revision"].as_u64().unwrap()
            > before["data"]["project_revision"].as_u64().unwrap()
    );
    assert!(
        after["data"]["sessions"]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == started.session.id.to_string())
    );
    assert_eq!(after["data"]["basis"]["source_refresh_performed"], false);
}

#[test]
fn intake_requests_require_absolute_paths_and_the_local_request_guard() {
    let f = Fixture::new();
    let root = f.root.join("pending");
    std::fs::create_dir(&root).unwrap();
    let base = f.run(&["overview", "start", "--json"])["service"]["url"]
        .as_str()
        .unwrap()
        .to_owned();
    for (path, guarded, origin, expected) in [
        (root.to_str().unwrap(), false, None, 403),
        (
            root.to_str().unwrap(),
            true,
            Some("https://untrusted.example"),
            403,
        ),
        ("relative/project", true, None, 400),
    ] {
        let mut request = ureq::post(format!("{base}/api/projects/intake"))
            .header("content-type", "application/json");
        if guarded {
            request = request.header("x-awr-project-hub", "1");
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let body = serde_json::json!({"root":path}).to_string();
        assert!(
            matches!(request.send(body.as_bytes()), Err(ureq::Error::StatusCode(code)) if code == expected)
        );
    }
    assert!(
        f.run(&["overview", "projects", "--json"])["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let first = f.enroll(&root);
    let again = f.enroll(&root.join("."));
    assert_eq!(
        first["data"]["project"]["key"],
        again["data"]["project"]["key"]
    );
    assert_eq!(
        f.run(&["overview", "projects", "--json"])["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!root.join(".awr").exists());
}

#[test]
fn intake_rejects_an_identity_replaced_at_the_registered_path() {
    let f = Fixture::new();
    let original = f.project("original");
    let registered = f.register(&original);
    std::fs::rename(&original, f.root.join("previous")).unwrap();
    f.project("original");
    let base = registered["service"]["url"].as_str().unwrap();
    let key = registered["projects"][0]["key"].as_str().unwrap();
    let result = ureq::get(format!("{base}/api/projects/{key}/intake")).call();
    assert!(matches!(result, Err(ureq::Error::StatusCode(409))));
    std::fs::remove_dir_all(original.join(".awr")).unwrap();
    let repair = f.get(&format!("{base}/api/projects/{key}/intake"));
    assert_eq!(repair["data"]["project"]["state"], "attention");
    assert_eq!(
        repair["data"]["guidance"]["next_action"]["argv"][3],
        "doctor"
    );
}

#[test]
fn project_requests_are_isolated_and_static_assets_are_bundled() {
    let f = Fixture::new();
    let a = f.project("alpha");
    let b = f.project("beta");
    let ra = f.register(&a);
    let rb = f.register(&b);
    let base = ra["service"]["url"].as_str().unwrap();
    assert_eq!(ra["service"]["pid"], rb["service"]["pid"]);
    let entries = f.get(&format!("{base}/api/projects"));
    assert_eq!(entries["data"]["projects"].as_array().unwrap().len(), 2);
    assert_eq!(
        entries["data"]["projects"][0]["key"],
        rb["projects"][0]["key"]
    );
    for (selected, other) in [(&ra, &rb), (&rb, &ra)] {
        let key = selected["projects"][0]["key"].as_str().unwrap();
        let graph = f.get(&format!(
            "{base}/api/projects/{key}/map/work-graph?cached=1&limit=100"
        ));
        assert_eq!(graph["ok"], true);
        let nav = f.get(&format!("{base}/api/projects/{key}/map/nav"));
        assert_eq!(nav["ok"], true);
        let json = nav.to_string();
        assert!(json.contains(selected["projects"][0]["project_id"].as_str().unwrap()));
        assert!(!json.contains(other["projects"][0]["project_id"].as_str().unwrap()));
        let snapshot = f.get(&format!("{base}/api/projects/{key}/map/snapshot"));
        assert_eq!(snapshot["ok"], true);
        assert_eq!(snapshot["data"]["basis"]["coherent"], true);
        assert_eq!(snapshot["data"]["basis"]["source_refresh_performed"], false);
        assert_eq!(
            snapshot["data"]["nav"]["project"]["id"],
            selected["projects"][0]["project_id"]
        );
        let unguarded = ureq::post(format!("{base}/api/projects/{key}/map/snapshot")).send_empty();
        assert!(matches!(unguarded, Err(ureq::Error::StatusCode(403))));
    }
    let reads: Vec<_> = [&ra, &rb]
        .into_iter()
        .map(|entry| {
            let url = format!(
                "{base}/api/projects/{}/map/nav",
                entry["projects"][0]["key"].as_str().unwrap()
            );
            let expected = entry["projects"][0]["project_id"]
                .as_str()
                .unwrap()
                .to_owned();
            std::thread::spawn(move || {
                let mut response = ureq::get(&url).call().unwrap();
                let body = response.body_mut().read_to_string().unwrap();
                assert!(body.contains(&expected));
            })
        })
        .collect();
    for read in reads {
        read.join().unwrap();
    }
    for path in [
        "/?project=known",
        "/styles.css",
        "/app.js",
        "/locales/en.js",
        "/project-map/views.js",
    ] {
        let response = ureq::get(format!("{base}{path}")).call().unwrap();
        assert_eq!(response.status(), 200);
    }
    let blocked = ureq::get(format!("{base}/api/projects"))
        .header("Origin", "https://untrusted.example")
        .call();
    assert!(matches!(blocked, Err(ureq::Error::StatusCode(403))));
    let traversal = ureq::get(format!("{base}/api/projects/not-registered/map/work-graph")).call();
    assert!(matches!(traversal, Err(ureq::Error::StatusCode(404))));
}

#[test]
fn killing_the_owned_service_is_recovered_by_the_next_ordinary_call() {
    let _port_holder = std::net::TcpListener::bind("127.0.0.1:7381").ok();
    let f = Fixture::new();
    let before = f.run(&["overview", "start", "--json"])["service"].clone();
    let pid = before["pid"].as_u64().unwrap().to_string();
    assert_eq!(
        before["executable"],
        Path::new(env!("CARGO_BIN_EXE_awr"))
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    #[cfg(unix)]
    let result = Command::new("kill").args(["-KILL", &pid]).output().unwrap();
    #[cfg(windows)]
    let result = Command::new("taskkill")
        .args(["/PID", &pid, "/F"])
        .output()
        .unwrap();
    success(&result);
    for _ in 0..100 {
        if f.run(&["overview", "status", "--json"])["service"].is_null() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = f
        .command()
        .env("AWR_LOCAL_WEB", "1")
        .arg("--version")
        .output()
        .unwrap();
    success(&output);
    let after = f.run(&["overview", "status", "--json"])["service"].clone();
    assert_ne!(before["instance"], after["instance"]);
    assert!(after["pid"].as_u64().is_some());
    assert_eq!(after["service"], "awr-project-hub");
    assert_eq!(
        before["url"], after["url"],
        "recovery retains the prior fallback address"
    );
}

#[test]
fn status_and_stopped_service_leave_the_catalog_intact() {
    let f = Fixture::new();
    assert!(f.run(&["overview", "status", "--json"])["service"].is_null());
    assert!(!f.root.join("user-state").exists());
    let root = f.project("alpha");
    f.register(&root);
    f.run(&["overview", "stop", "--json"]);
    assert!(f.run(&["overview", "status", "--json"])["service"].is_null());
    assert_eq!(
        f.run(&["overview", "projects", "--json"])["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
