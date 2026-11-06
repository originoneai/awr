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
    let projects = wait_for_projects(&f, 2);
    assert_eq!(projects["catalog_scope"], "user");
    let entries = projects["projects"].as_array().unwrap();
    assert!(entries.iter().any(|entry| entry["root"]
        == existing.canonicalize().unwrap().to_str().unwrap()
        && entry["state"] == "connected"));
    assert!(entries.iter().any(|entry| entry["root"]
        == repository.canonicalize().unwrap().to_str().unwrap()
        && entry["state"] == "discovered"
        && entry["project_id"].is_null()));
    assert!(!repository.join(".awr").exists());
}

#[test]
fn background_discovery_observes_new_projects_and_keeps_source_files_unchanged() {
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
    let repository = f.root.join("new-repository");
    std::fs::create_dir(&repository).unwrap();
    std::fs::write(
        repository.join("pyproject.toml"),
        "[project]\nname='new-repository'\n",
    )
    .unwrap();
    let observed = wait_for_projects(&f, 2);
    assert_eq!(observed["discovery"]["pending"], 1);
    assert_eq!(std::fs::read(source).unwrap(), before);
    assert!(!repository.join(".awr").exists());
    let instance = initial["service"]["instance"].clone();
    assert_eq!(
        f.run(&["overview", "status", "--json"])["service"]["instance"],
        instance
    );
}

#[test]
fn updated_discovery_settings_take_effect_in_the_running_service() {
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
    std::fs::create_dir_all(f.root.join("added/.git")).unwrap();
    let observed = wait_for_projects(&f, 1);
    assert_eq!(observed["discovery"]["pending"], 1);
    let current = f.run(&["overview", "status", "--json"])["service"].clone();
    assert_eq!(current["instance"], started["instance"]);
}

#[test]
fn ordinary_project_use_records_pending_intake_and_initialization_promotes_the_same_card() {
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
    let pending = f.run(&["overview", "projects", "--json"])["projects"][0].clone();
    assert_eq!(pending["state"], "discovered");
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
