#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
use fixture::*;
use serde_json::{Value, json};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

struct Fixture {
    dir: PathBuf,
    child: Option<Child>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn url(db: &str, app: bool) -> String {
    let raw = common::test_database_url_raw();
    if raw.starts_with("postgres://") || raw.starts_with("postgresql://") {
        let mut u = reqwest::Url::parse(&raw).unwrap();
        u.set_path(&format!("/{db}"));
        // Explicitly replace potentially overriding db/user/password query fields.
        let pairs = u
            .query_pairs()
            .filter(|(k, _)| k != "dbname" && !(app && matches!(k.as_ref(), "user" | "password")))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        u.set_query(None);
        if !pairs.is_empty() {
            u.query_pairs_mut().extend_pairs(pairs);
        }
        if app {
            u.set_username("awr_app").unwrap();
            u.set_password(Some("app-test")).unwrap();
        }
        u.to_string()
    } else {
        // Generated database identifiers contain only ASCII letters/digits/_; the
        // original libpq/Unix/hostaddr options remain intact.
        format!(
            "{raw} dbname={db}{}",
            if app {
                " user=awr_app password=app-test"
            } else {
                ""
            }
        )
    }
}
fn cli(connection: &str, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_awr-server"))
        .env("AWR_TEAM_DATABASE_URL", connection)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_operator_cli_provisions_and_revokes_a_client_using_the_native_service() {
    let (_g, _admin, db, _store) = setup().await;
    let dir = std::env::temp_dir().join(format!("awr-access-pg-{}", common::nonce(0)));
    std::fs::create_dir(&dir).unwrap();
    let mut fixture = Fixture {
        dir: dir.clone(),
        child: None,
    };
    let owner = url(&db, false);
    let app = url(&db, true);
    let token_path = dir.join("credential");
    let token = cli(
        &owner,
        &[
            "access",
            "token",
            "--credential-id",
            "native-client",
            "--output",
            token_path.to_str().unwrap(),
        ],
    );
    let bearer = std::fs::read_to_string(&token_path).unwrap();
    let bearer = bearer.trim();
    let mut plan = json!({"protocol_version":1,"tenant_id":TENANT,"project_id":PROJECT,
        "actor":{"id":"native","kind":"human","display_name":"Native member"},"client_id":"native-cli","role":"worker",
        "grants":[{"workstream_id":awr_core::Id::from(1),"authority_version":"1","read":true,"write":true,"manage":false,"attest_execution":false,"reconcile_execution":false}],
        "credential":{"id":"native-client","secret_hash":token["secret_hash"],"expires_at_unix_ms":null},"revoke_credentials":[]});
    let input = dir.join("plan.json");
    std::fs::write(&input, serde_json::to_vec(&plan).unwrap()).unwrap();
    let preview = cli(
        &owner,
        &["access", "preview", "--input", input.to_str().unwrap()],
    );
    let apply_args = [
        "access",
        "apply",
        "--input",
        input.to_str().unwrap(),
        "--request-id",
        "register-native",
        "--expected-state",
        preview["state_digest"].as_str().unwrap(),
        "--expected-plan",
        preview["plan_digest"].as_str().unwrap(),
    ];
    let registered = cli(&owner, &apply_args);
    let replay = cli(&owner, &apply_args);
    assert_eq!(registered["receipt"], replay["receipt"]);
    assert_eq!(replay["replayed"], true);
    let outcome = cli(
        &owner,
        &[
            "access",
            "outcome",
            "--tenant-id",
            TENANT,
            "--project-id",
            PROJECT,
            "--request-id",
            "register-native",
        ],
    );
    assert_eq!(outcome["receipt"], registered["receipt"]);
    let denied = Command::new(env!("CARGO_BIN_EXE_awr-server"))
        .env("AWR_TEAM_DATABASE_URL", &app)
        .args(["access", "preview", "--input", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(denied.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&denied.stderr).unwrap()["code"],
        "Forbidden"
    );
    let config = dir.join("service.toml");
    std::fs::write(&config,format!("version=1\nlisten='127.0.0.1:0'\nallowed_hosts=[]\n[[projects]]\nkey='one'\ntenant_id='{TENANT}'\nproject_id='{PROJECT}'\n")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_awr-server"))
        .env("AWR_TEAM_DATABASE_URL", &app)
        .arg("serve")
        .arg("--config")
        .arg(&config)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    fixture.child = Some(child);
    let announcement = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            std::io::BufReader::new(stdout)
                .read_line(&mut line)
                .unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let endpoint = format!(
        "http://{}/v1/projects/one/query",
        announcement["listen"].as_str().unwrap()
    );
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    // Static member grants do not constitute an Agent execution delegation.
    let agent_token_path = dir.join("agent-credential");
    let agent_token = cli(
        &owner,
        &[
            "access",
            "token",
            "--credential-id",
            "native-agent",
            "--output",
            agent_token_path.to_str().unwrap(),
        ],
    );
    let agent_bearer = std::fs::read_to_string(&agent_token_path).unwrap();
    let mut agent_plan = plan.clone();
    agent_plan["actor"] = json!({"id":"native-agent","kind":"agent","display_name":"Native Agent"});
    agent_plan["client_id"] = json!("native-agent-cli");
    agent_plan["credential"] = json!({
        "id":"native-agent",
        "secret_hash":agent_token["secret_hash"],
        "expires_at_unix_ms":null
    });
    let agent_input = dir.join("agent-plan.json");
    std::fs::write(&agent_input, serde_json::to_vec(&agent_plan).unwrap()).unwrap();
    let agent_preview = cli(
        &owner,
        &[
            "access",
            "preview",
            "--input",
            agent_input.to_str().unwrap(),
        ],
    );
    let agent_registered = cli(
        &owner,
        &[
            "access",
            "apply",
            "--input",
            agent_input.to_str().unwrap(),
            "--request-id",
            "register-native-agent",
            "--expected-state",
            agent_preview["state_digest"].as_str().unwrap(),
            "--expected-plan",
            agent_preview["plan_digest"].as_str().unwrap(),
        ],
    );
    assert_eq!(
        http.post(&endpoint)
            .bearer_auth(agent_bearer.trim())
            .json(&json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    assert!(!agent_registered.to_string().contains(agent_bearer.trim()));
    assert!(
        !agent_registered
            .to_string()
            .contains(agent_token["secret_hash"].as_str().unwrap())
    );
    for (work, expected) in [("a", 200), ("b-private", 403)] {
        let response = http
            .post(&endpoint)
            .bearer_auth(bearer)
            .json(&json!({"protocol_version":1,"op":"work.prepare","work_id":work}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
    }
    plan["credential"] = Value::Null;
    plan["grants"] = json!([]);
    plan["revoke_credentials"] = json!(["native-client"]);
    std::fs::write(&input, serde_json::to_vec(&plan).unwrap()).unwrap();
    let preview = cli(
        &owner,
        &["access", "preview", "--input", input.to_str().unwrap()],
    );
    cli(
        &owner,
        &[
            "access",
            "apply",
            "--input",
            input.to_str().unwrap(),
            "--request-id",
            "revoke-native",
            "--expected-state",
            preview["state_digest"].as_str().unwrap(),
            "--expected-plan",
            preview["plan_digest"].as_str().unwrap(),
        ],
    );
    assert_eq!(
        http.post(endpoint)
            .bearer_auth(bearer)
            .json(&json!({"protocol_version":1,"op":"capabilities"}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    assert!(!registered.to_string().contains(bearer));
    assert!(
        !registered
            .to_string()
            .contains(token["secret_hash"].as_str().unwrap())
    );
}
