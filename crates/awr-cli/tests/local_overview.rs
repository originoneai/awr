use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-hub-cli-{}", awr_core::Id::new()));
        std::fs::create_dir(&root).unwrap();
        Self { root }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_awr"));
        c.env("AWR_LOCAL_HOME", self.root.join("user-state"))
            .env("AWR_LOCAL_WEB", "0");
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

#[test]
fn background_startup_releases_captured_output_before_the_service_stops() {
    for args in [vec!["overview", "start", "--json"], vec!["--version"]] {
        let f = Fixture::new();
        let mut command = f.command();
        command.env("AWR_LOCAL_WEB", "1").args(&args);
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let _ = sender.send(command.output());
        });
        // Fixture teardown stops the service even if its inherited pipes prevent EOF.
        let output = receiver
            .recv_timeout(Duration::from_secs(15))
            .expect("background startup retained the calling client's output pipes")
            .unwrap();
        reader.join().unwrap();
        success(&output);
        let status = f.run(&["overview", "status", "--json"])["service"].clone();
        assert!(status["pid"].as_u64().is_some());
        assert_eq!(status["service"], "awr-project-hub");
        assert_eq!(
            f.get(&format!("{}/api/health", status["url"].as_str().unwrap()))["data"]["instance"],
            status["instance"]
        );
    }
}
