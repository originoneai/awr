//! Native CLI/HTTP boundary tests: these do not substitute for PostgreSQL/MCP parity.
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const TOKEN: &str = "synthetic-team-query-credential";
const STREAM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

struct Project(PathBuf);
impl Project {
    fn new(endpoint: &str, alias: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "awr-team-query-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join(".awr/team-remotes")).unwrap();
        let profile = awr_team::RemoteProfile {
            name: "test".into(),
            endpoint: endpoint.into(),
            project_key: alias.into(),
            credential_env: "AWR_QUERY_TEST_TOKEN".into(),
            protocol_version: 1,
        };
        fs::write(
            path.join(".awr/team-remotes/test.toml"),
            toml::to_string(&profile).unwrap(),
        )
        .unwrap();
        Self(path)
    }
    fn command(&self, args: &[&str], json: bool) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_awr"));
        cmd.env("AWR_TEAM_ENABLED", "1")
            .env("AWR_QUERY_TEST_TOKEN", TOKEN)
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .env("NO_PROXY", "*")
            .arg("--project")
            .arg(&self.0);
        if json {
            cmd.arg("--json");
        }
        cmd.args(args);
        cmd
    }
    fn run(&self, args: &[&str], json: bool) -> Output {
        self.command(args, json).output().unwrap()
    }
    fn assert_no_personal_state(&self) {
        assert!(!self.0.join(".awr/state.db").exists());
        let mut entries: Vec<_> = fs::read_dir(self.0.join(".awr"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        entries.sort();
        assert_eq!(entries, vec![std::ffi::OsString::from("team-remotes")]);
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Request {
    target: String,
    headers: String,
    body: Value,
}
struct Server {
    listener: TcpListener,
    worker: JoinHandle<Request>,
}
impl Server {
    fn raw(response: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let copy = listener.try_clone().unwrap();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut socket = loop {
                match copy.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("mock service did not receive a query: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let (headers, start, length) = loop {
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    break (headers, end + 4, length);
                }
            };
            while bytes.len() < start + length {
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let request = Request {
                target: headers.lines().next().unwrap().to_owned(),
                body: serde_json::from_slice(&bytes[start..start + length]).unwrap(),
                headers,
            };
            // A bounded client may close the connection before an oversized reply is sent.
            let _ = socket.write_all(&response);
            request
        });
        Self { listener, worker }
    }
    fn json(status: u16, value: &Value) -> Self {
        Self::raw(reply(status, value.to_string().as_bytes()))
    }
    fn endpoint(&self) -> String {
        format!("http://{}/gateway///", self.listener.local_addr().unwrap())
    }
    fn finish(self) -> Request {
        let request = self.worker.join().unwrap();
        assert_eq!(
            self.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "one invocation must send only one request"
        );
        request
    }
}
fn reply(status: u16, body: &[u8]) -> Vec<u8> {
    let mut result = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
    result.extend_from_slice(body);
    result
}
fn envelope(data: Value) -> Value {
    json!({"protocol_version":1,"workstream_id":STREAM,"authority_version":"4","selection_basis":"explicit",
        "scope_id":"main","source_snapshot_id":"fixture-source","project_revision":"17","coordinator_epoch":"2","data":data})
}
fn response(op: &str) -> Value {
    match op {
        "capabilities" => json!({"protocol":"awr-team-workstream","protocol_version":1,
            "identity":{"actor_id":"reader","client_id":"terminal"},"queries":["capabilities","work.next","session.inspect","claim.inspect","execution.inspect","command.inspect","work.recovery"],"commands":["session.start"],"extension":{"keep":true}}),
        "workstreams.list" => {
            json!({"items":[{"id":STREAM,"title":"Synthetic stream","external_key":"main","state":"active","authority_version":4}],"total":1,"next_cursor":null})
        }
        "work.next" => {
            json!({"protocol_version":1,"project_revision":"17","data":{"identity":{"actor_id":"reader"},
            "resume":[],"resume_truncated":false,"items":[{"work_id":"w","state":null,"responsibility":{"relation":"unassigned"},"next_query":{"op":"work.recovery","work_id":"w"}}],
            "next_cursor":"opaque-next","execution_authorized":false,"next_action":"inspect","recheck_on":["source_change"]}})
        }
        "work.inbox" => {
            json!({"protocol_version":1,"project_revision":"17","source_snapshot_id":"fixture-source","data":{
            "identity":{"actor_id":"reader"},"items":[],"next_cursor":"opaque-empty-page","execution_authorized":false,
            "state_basis":"current_persistent_facts","deduplicate_by":"item_key","refresh_from_first_page_on_change":true,"next_action":"continue paging"}})
        }
        "work.list" | "work.search" => envelope(
            json!({"items":[{"work_id":"w","title":"Synthetic task","contract_hash":"contract-hash"}],"total":1,"next_cursor":null}),
        ),
        "work.prepare" | "work.snapshot" => {
            let mut data = json!({"work_id":"w","context_hash":"context-hash","context_hash_protocol":"awr-team-workstream-context-v1",
                "context_complete":false,"completeness_reasons":["missing_goals"],"execution_admission":"not_evaluated","runtime":null,
                "visible_contract":{"goals":[],"hard_rules":["Keep required facts"]},"required_specs":[{"text":"Required spec text"}],
                "authorized_readable_refs":[],"next_step":"restore context","guidance":{"condition":"missing goals"}});
            if op == "work.snapshot" {
                data["snapshot"] =
                    json!({"consistency":"repeatable_read","queried_at_unix_ms":1000});
                data["observation"] =
                    json!({"runtime":null,"usage":null,"guidance":{"condition":"missing goals"}});
            }
            envelope(data)
        }
        "work.observe" => envelope(
            json!({"work_id":"w","observed_at_unix_ms":1000,"runtime":null,"usage":null,
            "execution_authorized":false,"missing":{"usage":"no_session"},"pr_deliveries":[],"guidance":{"condition":"inspect recorded facts"}}),
        ),
        "session.inspect" | "work.recovery" => envelope(json!({
            "items":[{"session_id":"s","state":"ended","session_version":"9007199254740993",
                "checkpoint_id":"checkpoint","context_hash":"historical-context-hash",
                "contract_hash":"contract-hash","contract_matches_current":true,
                "next_action":"historical action","open_loops":[]}],
            "current_contract_hash":"contract-hash","automatic_resume":false,
            "checkpoint_actions_are_historical":true,"guidance":{"code":"inspect_current_facts"}
        })),
        "claim.inspect" => envelope(
            json!({"claim_id":"claim","session_id":"s","state":"released",
            "fence":"9007199254740993","lease_version":"2","lease_live":false,"execution_authorized":false}),
        ),
        "execution.inspect" => envelope(
            json!({"execution_id":"execution","session_id":"s","state":"failed",
            "execution_version":"9007199254740993","effects_settled":false,"artifact_verified":false,
            "receipt_details_available":false,"latest_receipt":null,"execution_authorized":false,
            "extension":{"authority":"future-authority","optional":null}}),
        ),
        "command.inspect" => envelope(json!({"state":"committed","receipt":{
            "request_id":"original-request","op":"session.start","committed_project_revision":"9007199254740993",
            "data":{"session_id":"s","state":"active"},"extension":null}})),
        _ => panic!("unknown fixture operation"),
    }
}
fn error(out: &Output, exit: i32) -> Value {
    assert_eq!(out.status.code(), Some(exit), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    serde_json::from_slice(&out.stderr).expect("one typed error JSON on stderr")
}

#[test]
fn all_fourteen_native_commands_preserve_service_results_and_send_one_minimal_query() {
    let cases: [(&str, Vec<&str>, Value); 14] = [
        ("capabilities", vec!["capabilities"], json!({})),
        ("workstreams.list", vec!["workstream", "list"], json!({})),
        ("work.next", vec!["work", "next"], json!({})),
        ("work.inbox", vec!["work", "inbox"], json!({})),
        ("work.list", vec!["work", "list"], json!({})),
        (
            "work.search",
            vec!["work", "search", "Observer 页面"],
            json!({"search":"Observer 页面"}),
        ),
        (
            "work.prepare",
            vec!["work", "prepare", "w"],
            json!({"work_id":"w"}),
        ),
        (
            "work.snapshot",
            vec!["work", "snapshot", "--session", "s"],
            json!({"session_id":"s"}),
        ),
        (
            "work.observe",
            vec!["work", "observe", "w", "--session", "s"],
            json!({"work_id":"w","session_id":"s"}),
        ),
        (
            "session.inspect",
            vec!["session", "inspect", "s"],
            json!({"session_id":"s"}),
        ),
        (
            "claim.inspect",
            vec!["claim", "inspect", "claim", "--work", "w"],
            json!({"claim_id":"claim","work_id":"w"}),
        ),
        (
            "execution.inspect",
            vec!["execution", "inspect", "execution", "--session", "s"],
            json!({"execution_id":"execution","session_id":"s"}),
        ),
        (
            "command.inspect",
            vec![
                "command",
                "inspect",
                "original-request",
                "--work",
                "w",
                "--session",
                "s",
            ],
            json!({"request_id":"original-request","work_id":"w","session_id":"s"}),
        ),
        (
            "work.recovery",
            vec!["work", "recovery", "w"],
            json!({"work_id":"w"}),
        ),
    ];
    for (op, arguments, extra) in cases {
        let expected_response = response(op);
        let server = Server::json(200, &expected_response);
        let project = Project::new(&server.endpoint(), "demo");
        let profile_before = fs::read(project.0.join(".awr/team-remotes/test.toml")).unwrap();
        let mut args = vec!["team"];
        args.extend(arguments);
        args.extend(["--remote", "test", "--json"]);
        let out = project.run(&args, false);
        assert!(out.status.success(), "{op}: {out:?}");
        assert!(out.stderr.is_empty(), "{out:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            expected_response
        );
        let request = server.finish();
        assert_eq!(
            request.target,
            "POST /gateway/v1/projects/demo/query HTTP/1.1"
        );
        assert!(
            request
                .headers
                .to_ascii_lowercase()
                .contains(&format!("authorization: bearer {TOKEN}"))
        );
        assert!(
            request
                .headers
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        assert!(!request.headers.to_ascii_lowercase().contains("origin:"));
        let mut expected = json!({"protocol_version":1,"op":op});
        expected
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(request.body, expected, "{op}");
        assert_eq!(
            fs::read(project.0.join(".awr/team-remotes/test.toml")).unwrap(),
            profile_before
        );
        project.assert_no_personal_state();
    }
}

#[test]
fn runtime_inspections_preserve_scope_combinations_and_literal_record_ids() {
    let record = " original 中文 request ";
    for (resource, field) in [
        ("claim", "claim_id"),
        ("execution", "execution_id"),
        ("command", "request_id"),
    ] {
        for (scope, fields) in [
            (vec!["--work", "w"], json!({"work_id":"w"})),
            (vec!["--session", "s"], json!({"session_id":"s"})),
            (
                vec!["--work", "w", "--session", "s"],
                json!({"work_id":"w","session_id":"s"}),
            ),
        ] {
            let op = format!("{resource}.inspect");
            let server = Server::json(200, &response(&op));
            let project = Project::new(&server.endpoint(), "demo");
            let mut args = vec![
                "team",
                resource,
                "inspect",
                record,
                "--remote",
                "test",
                "--workstream",
                STREAM,
            ];
            args.extend(scope);
            assert!(project.run(&args, true).status.success(), "{args:?}");
            let mut expected = json!({"protocol_version":1,"op":op,"workstream_id":STREAM});
            expected[field] = json!(record);
            expected
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            assert_eq!(server.finish().body, expected);
        }
    }
    for (op, args, fields) in [
        (
            "session.inspect",
            vec!["session", "inspect", "s", "--work", "w"],
            json!({"work_id":"w","session_id":"s"}),
        ),
        (
            "work.recovery",
            vec!["work", "recovery", "--session", "s"],
            json!({"session_id":"s"}),
        ),
        (
            "work.recovery",
            vec!["work", "recovery", "w", "--session", "s"],
            json!({"work_id":"w","session_id":"s"}),
        ),
    ] {
        let server = Server::json(200, &response(op));
        let project = Project::new(&server.endpoint(), "demo");
        let mut all = vec!["team"];
        all.extend(args);
        all.extend(["--remote", "test", "--workstream", STREAM]);
        assert!(project.run(&all, true).status.success());
        let mut expected = json!({"protocol_version":1,"op":op,"workstream_id":STREAM});
        expected
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert_eq!(server.finish().body, expected);
    }
}

#[test]
fn runtime_reads_reject_missing_scope_invalid_ids_and_unsupported_options_without_network() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let project = Project::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "demo",
    );
    let long_id = "雪".repeat(43);
    for resource in ["claim", "execution", "command"] {
        for scope in [vec![], vec!["--workstream", STREAM]] {
            let mut args = vec!["team", resource, "inspect", "record", "--remote", "test"];
            args.extend(scope);
            assert_eq!(
                error(&project.run(&args, true), 2)["code"],
                "InvalidInput",
                "{args:?}"
            );
        }
    }
    for base in [
        vec!["team", "session", "inspect", "s"],
        vec!["team", "claim", "inspect", "c", "--work", "w"],
        vec!["team", "execution", "inspect", "e", "--session", "s"],
        vec!["team", "command", "inspect", "r", "--work", "w"],
        vec!["team", "work", "recovery", "w"],
    ] {
        assert_eq!(error(&project.run(&base, true), 2)["code"], "InvalidInput");
        for flag in ["--limit", "--cursor", "--max-context-bytes"] {
            let mut args = base.clone();
            args.extend(["--remote", "test", flag, "1"]);
            assert_eq!(
                error(&project.run(&args, true), 2)["code"],
                "InvalidInput",
                "{args:?}"
            );
        }
        for invalid in ["", &long_id, "record\n"] {
            let mut args = base.clone();
            args[3] = invalid;
            args.extend(["--remote", "test"]);
            assert_eq!(
                error(&project.run(&args, true), 2)["code"],
                "InvalidInput",
                "{args:?}"
            );
        }
        let mut missing_id = base.clone();
        missing_id.remove(3);
        missing_id.extend(["--remote", "test"]);
        assert_eq!(
            error(&project.run(&missing_id, true), 2)["code"],
            "InvalidInput"
        );
    }
    for extra in [
        vec!["--body", "{}"],
        vec!["--offline"],
        vec!["--body", "{}", "--offline"],
    ] {
        let mut args = vec![
            "team", "command", "inspect", "r", "--work", "w", "--remote", "test",
        ];
        args.extend(extra.iter().copied());
        assert_eq!(error(&project.run(&args, true), 2)["code"], "InvalidInput");
        let mut before = vec!["team", "command"];
        before.extend(extra);
        before.extend(["inspect", "r", "--work", "w", "--remote", "test"]);
        assert_eq!(
            error(&project.run(&before, true), 2)["code"],
            "InvalidInput"
        );
    }
    for args in [
        vec![
            "team", "session", "inspect", "s", "--work", "", "--remote", "test",
        ],
        vec![
            "team",
            "claim",
            "inspect",
            "c",
            "--session",
            "s\n",
            "--remote",
            "test",
        ],
        vec![
            "team",
            "execution",
            "inspect",
            "e",
            "--work",
            &long_id,
            "--remote",
            "test",
        ],
        vec![
            "team",
            "command",
            "inspect",
            "r",
            "--work",
            "w",
            "--workstream",
            "main",
            "--remote",
            "test",
        ],
        vec![
            "team",
            "work",
            "recovery",
            "--workstream",
            STREAM,
            "--remote",
            "test",
        ],
        vec!["team", "command"],
    ] {
        assert_eq!(
            error(&project.run(&args, true), 2)["code"],
            "InvalidInput",
            "{args:?}"
        );
    }
    for args in [
        vec!["team", "session", "inspect", "--help"],
        vec!["team", "claim", "inspect", "--help"],
        vec!["team", "execution", "inspect", "--help"],
        vec!["team", "command", "--help"],
        vec!["team", "command", "inspect", "--help"],
        vec!["team", "work", "recovery", "--help"],
    ] {
        let out = project.run(&args, true);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert!(out.stderr.is_empty());
        assert!(String::from_utf8_lossy(&out.stdout).contains("Usage:"));
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    project.assert_no_personal_state();
}

#[test]
fn unknown_command_and_empty_recovery_are_successful_reads_with_safe_human_output() {
    let mut unknown = envelope(json!({"state":"unknown","request_id":"original-request",
        "retry":"conditional","action_guidance":{"because":"No committed receipt is visible; an in-flight command may still commit."},
        "extension":{"literal":"\u{1b}[31m\u{9b}31m","optional":null}}));
    unknown["authority_version"] = json!("9007199254740993");
    let mut empty = response("work.recovery");
    empty["data"]["items"] = json!([]);
    let mut expired = response("claim.inspect");
    expired["data"]["state"] = json!("expired");
    let mut settled = response("execution.inspect");
    settled["data"]["state"] = json!("succeeded");
    settled["data"]["effects_settled"] = json!(true);
    settled["data"]["settlement_basis"] = json!("caller_asserted");
    for (args, value, human_hint) in [
        (
            vec!["session", "inspect", "s"],
            response("session.inspect"),
            "Checkpoint actions are historical",
        ),
        (
            vec!["command", "inspect", "original-request", "--work", "w"],
            unknown,
            "does not prove the original command failed or cannot still commit",
        ),
        (
            vec!["work", "recovery", "w"],
            empty,
            "Checkpoint actions are historical",
        ),
        (
            vec!["claim", "inspect", "claim", "--work", "w"],
            expired,
            "does not grant execution authority",
        ),
        (
            vec!["execution", "inspect", "execution", "--work", "w"],
            settled,
            "does not grant execution authority",
        ),
    ] {
        for json_output in [true, false] {
            let server = Server::json(200, &value);
            let project = Project::new(&server.endpoint(), "demo");
            let mut all = vec!["team"];
            all.extend(args.clone());
            all.extend(["--remote", "test"]);
            let out = project.run(&all, json_output);
            assert!(out.status.success(), "{all:?}: {out:?}");
            assert!(out.stderr.is_empty());
            if json_output {
                assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap(), value);
            } else {
                let text = String::from_utf8(out.stdout).unwrap();
                assert!(text.contains(human_hint), "{text}");
                assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
            }
            server.finish();
            project.assert_no_personal_state();
        }
    }
}

#[test]
fn runtime_reads_reject_unrelated_results_and_malformed_command_variants() {
    for (args, value) in [
        (vec!["session", "inspect", "s"], response("work.list")),
        (
            vec!["claim", "inspect", "c", "--work", "w"],
            response("execution.inspect"),
        ),
        (
            vec!["execution", "inspect", "e", "--work", "w"],
            response("claim.inspect"),
        ),
        (
            vec!["command", "inspect", "r", "--work", "w"],
            envelope(json!({"state":"committed","receipt":null})),
        ),
        (
            vec!["command", "inspect", "r", "--work", "w"],
            envelope(json!({"state":"unknown"})),
        ),
        (vec!["work", "recovery", "w"], response("work.observe")),
    ] {
        let server = Server::json(200, &value);
        let project = Project::new(&server.endpoint(), "demo");
        let mut all = vec!["team"];
        all.extend(args);
        all.extend(["--remote", "test"]);
        assert_eq!(
            error(&project.run(&all, true), 1)["code"],
            "TeamResponseInvalid"
        );
        server.finish();
    }
}

#[test]
fn optional_fields_and_opaque_cursors_map_without_changing_filters() {
    let cursor = "original cursor/+==";
    for (op, args, expected) in [
        (
            "work.search",
            vec![
                "work",
                "search",
                " title %_ ",
                "--workstream",
                STREAM,
                "--limit",
                "100",
                "--cursor",
                cursor,
            ],
            json!({"workstream_id":STREAM,"search":" title %_ ","limit":100,"cursor":cursor}),
        ),
        (
            "work.inbox",
            vec![
                "work",
                "inbox",
                "--limit",
                "1",
                "--cursor",
                cursor,
                "--max-context-bytes",
                "262144",
            ],
            json!({"limit":1,"cursor":cursor,"max_context_bytes":262144}),
        ),
        (
            "work.prepare",
            vec![
                "work",
                "prepare",
                "w",
                "--session",
                "s",
                "--workstream",
                STREAM,
                "--max-context-bytes",
                "131072",
            ],
            json!({"work_id":"w","session_id":"s","workstream_id":STREAM,"max_context_bytes":131072}),
        ),
    ] {
        let server = Server::json(200, &response(op));
        let project = Project::new(&server.endpoint(), "a/b c%2f中文");
        let mut all = vec!["team"];
        all.extend(args);
        all.extend(["--remote", "test"]);
        assert!(project.run(&all, true).status.success());
        let request = server.finish();
        let mut body = json!({"protocol_version":1,"op":op});
        body.as_object_mut()
            .unwrap()
            .extend(expected.as_object().unwrap().clone());
        assert_eq!(request.body, body);
        assert_eq!(
            request.target,
            "POST /gateway/v1/projects/a%2Fb%20c%252f%E4%B8%AD%E6%96%87/query HTTP/1.1"
        );
    }
}

#[test]
fn invalid_usage_never_connects_and_help_stays_successful() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let project = Project::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "demo",
    );
    let long_id = "x".repeat(129);
    let long_search = "雪".repeat(171);
    let long_cursor = "x".repeat(4097);
    for args in [
        vec!["capabilities", "--remote", ""],
        vec!["work", "next"],
        vec!["work", "prepare", "--remote", "test"],
        vec![
            "work",
            "prepare",
            "--workstream",
            STREAM,
            "--remote",
            "test",
        ],
        vec!["work", "next", "--remote", "test", "--workstream", STREAM],
        vec!["work", "inbox", "--remote", "test", "--session", "s"],
        vec![
            "work",
            "list",
            "--remote",
            "test",
            "--max-context-bytes",
            "1",
        ],
        vec!["work", "prepare", "w", "--remote", "test", "--limit", "1"],
        vec!["work", "next", "--remote", "test", "--limit", "0"],
        vec!["work", "next", "--remote", "test", "--limit", "101"],
        vec![
            "work",
            "observe",
            "w",
            "--remote",
            "test",
            "--max-context-bytes",
            "262145",
        ],
        vec![
            "work",
            "observe",
            "w",
            "--remote",
            "test",
            "--max-context-bytes",
            "0",
        ],
        vec!["work", "prepare", &long_id, "--remote", "test"],
        vec!["work", "prepare", "w\n", "--remote", "test"],
        vec!["work", "search", &long_search, "--remote", "test"],
        vec!["work", "search", "", "--remote", "test"],
        vec!["work", "next", "--remote", "test", "--cursor", &long_cursor],
        vec!["work", "list", "--remote", "test", "--workstream", "main"],
        vec!["work", "list", "--remote", "../test"],
        vec!["work", "list", "--remote", ""],
    ] {
        let mut all = vec!["team"];
        all.extend(args);
        assert_eq!(
            error(&project.run(&all, true), 2)["code"],
            "InvalidInput",
            "{all:?}"
        );
    }
    for args in [
        vec!["team", "--help"],
        vec!["team", "work", "--help"],
        vec!["team", "work", "prepare", "--help"],
    ] {
        let out = project.run(&args, true);
        assert!(out.status.success(), "{out:?}");
        assert!(out.stderr.is_empty());
        assert!(String::from_utf8_lossy(&out.stdout).contains("Usage:"));
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    project.assert_no_personal_state();
}

#[test]
fn remote_errors_keep_codes_diagnostics_and_missing_messages() {
    for (status, report) in [
        (
            401,
            json!({"code":"Unauthorized","message":"invalid credential"}),
        ),
        (403, json!({"code":"Forbidden","message":"access denied"})),
        (
            409,
            json!({"code":"ContextIncomplete","message":"raise budget","next_step":"restore refs","diagnostic":{"pointer":"/max_context_bytes"},"extension":{"version":"7"}}),
        ),
        (503, json!({"code":"Busy"})),
        (
            500,
            json!({"code":"FutureCode","message":"new error","action_guidance":{"next_action":"inspect"}}),
        ),
    ] {
        let server = Server::json(status, &report);
        let project = Project::new(&server.endpoint(), "demo");
        let out = project.run(&["team", "work", "prepare", "w", "--remote", "test"], true);
        assert_eq!(error(&out, 1), report);
        server.finish();
        project.assert_no_personal_state();
    }
    let report =
        json!({"code":"Forbidden","message":format!("Bearer {TOKEN}"),"details":{TOKEN:[TOKEN]}});
    let server = Server::json(403, &report);
    let project = Project::new(&server.endpoint(), "demo");
    let out = project.run(&["team", "capabilities", "--remote", "test"], true);
    assert_eq!(error(&out, 1)["code"], "Forbidden");
    assert!(!String::from_utf8_lossy(&out.stderr).contains(TOKEN));
    server.finish();

    let server = Server::json(503, &json!({"code":"Busy"}));
    let project = Project::new(&server.endpoint(), "demo");
    let out = project.run(&["team", "capabilities", "--remote", "test"], false);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Busy: Team query failed (HTTP 503)"));
    server.finish();
}

#[test]
fn malformed_results_and_non_json_errors_do_not_become_success() {
    for (status, body, expected) in [
        (
            200,
            "<html>private proxy detail</html>",
            "TeamResponseInvalid",
        ),
        (200, "[]", "TeamResponseInvalid"),
        (200, "{}", "TeamResponseInvalid"),
        (200, r#"{"code":"Forbidden"}"#, "TeamResponseInvalid"),
        (204, "", "TeamResponseInvalid"),
        (500, "<html>private proxy detail</html>", "TeamHttpError"),
        (404, "", "TeamHttpError"),
        (400, "{}", "TeamResponseInvalid"),
        (
            403,
            r#"{"code":"Forbidden","message":false}"#,
            "TeamResponseInvalid",
        ),
    ] {
        let server = Server::raw(reply(status, body.as_bytes()));
        let project = Project::new(&server.endpoint(), "demo");
        let out = project.run(&["team", "capabilities", "--remote", "test"], true);
        assert_eq!(error(&out, 1)["code"], expected, "{status} {body}");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("private proxy detail"));
        server.finish();
    }
    let mut invalid = response("work.snapshot");
    invalid["protocol_version"] = json!(2);
    let server = Server::json(200, &invalid);
    let project = Project::new(&server.endpoint(), "demo");
    assert_eq!(
        error(
            &project.run(&["team", "work", "snapshot", "w", "--remote", "test"], true),
            1
        )["code"],
        "TeamResponseInvalid"
    );
    server.finish();
}

#[test]
fn response_limits_apply_to_declared_and_streamed_bodies() {
    for raw in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 2097153\r\n\r\n".to_vec(),
        {
            let mut bytes = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            bytes.extend(vec![b' '; 2097153]);
            bytes
        },
    ] {
        let server = Server::raw(raw);
        let project = Project::new(&server.endpoint(), "demo");
        assert_eq!(
            error(
                &project.run(&["team", "capabilities", "--remote", "test"], true),
                1
            )["code"],
            "TeamResponseTooLarge"
        );
        server.finish();
    }
}

#[test]
fn redirects_never_forward_credentials_and_interrupted_bodies_fail() {
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    for status in [301, 302, 303, 307, 308] {
        let server = Server::raw(format!("HTTP/1.1 {status} Redirect\r\nLocation: http://{}/stolen\r\nContent-Length: 0\r\n\r\n",destination.local_addr().unwrap()).into_bytes());
        let project = Project::new(&server.endpoint(), "demo");
        assert_eq!(
            error(
                &project.run(&["team", "capabilities", "--remote", "test"], true),
                1
            )["code"],
            "TeamRedirect"
        );
        server.finish();
    }
    assert_eq!(
        destination.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let server = Server::raw(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{".to_vec());
    let project = Project::new(&server.endpoint(), "demo");
    assert_eq!(
        error(
            &project.run(&["team", "capabilities", "--remote", "test"], true),
            1
        )["code"],
        "TeamTransport"
    );
    server.finish();
}

#[test]
fn local_failures_and_the_team_switch_send_nothing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let project = Project::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "demo",
    );
    let args = ["team", "work", "next", "--remote", "test"];
    for token in [
        None,
        Some(""),
        Some("invalid\ncredential"),
        Some("invalid credential"),
    ] {
        let mut cmd = project.command(&args, true);
        cmd.env_remove("AWR_QUERY_TEST_TOKEN");
        if let Some(token) = token {
            cmd.env("AWR_QUERY_TEST_TOKEN", token);
        }
        assert_eq!(error(&cmd.output().unwrap(), 1)["code"], "InvalidInput");
    }
    let out = project
        .command(&args, true)
        .env("AWR_TEAM_ENABLED", "0")
        .output()
        .unwrap();
    assert_eq!(error(&out, 1)["code"], "Unsupported");
    assert_eq!(
        error(
            &project.run(&["team", "work", "next", "--remote", "missing"], true),
            1
        )["code"],
        "NotFound"
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    project.assert_no_personal_state();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let refused = Project::new(&format!("http://{address}"), "demo");
    assert_eq!(error(&refused.run(&args, true), 1)["code"], "TeamTransport");
}

#[test]
fn tls_handshake_failures_do_not_echo_transport_details() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let project = Project::new(
        &format!("https://{}", listener.local_addr().unwrap()),
        "demo",
    );
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "TLS client did not connect");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert!(socket.read(&mut [0; 4096]).unwrap() > 0);
        let _ = socket.write_all(b"HTTP/1.1 400 private-tls-proxy-detail\r\n\r\n");
    });
    let out = project.run(&["team", "capabilities", "--remote", "test"], true);
    assert_eq!(error(&out, 1)["code"], "TeamTransport");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("private-tls-proxy-detail"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(TOKEN));
    server.join().unwrap();
}

#[test]
fn human_output_keeps_incomplete_context_continuations_and_advisory_boundaries() {
    for op in [
        "work.prepare",
        "work.snapshot",
        "work.observe",
        "work.inbox",
        "work.next",
        "capabilities",
        "workstreams.list",
        "work.list",
        "work.search",
    ] {
        let mut value = response(op);
        if op == "work.next" {
            value["data"]["resume_truncated"] = json!(true);
        }
        if op == "work.snapshot" {
            value["data"].as_object_mut().unwrap().remove("guidance");
        }
        if op == "work.list" {
            value["data"]["items"][0]["title"] = json!("literal\u{1b}[31m\u{9b}31m");
        }
        let server = Server::json(200, &value);
        let project = Project::new(&server.endpoint(), "demo");
        let mut args = vec!["team"];
        if op == "capabilities" {
            args.push("capabilities");
        } else if op == "workstreams.list" {
            args.extend(["workstream", "list"]);
        } else {
            args.extend(["work", op.split_once('.').unwrap().1]);
            if matches!(op, "work.prepare" | "work.snapshot" | "work.observe") {
                args.push("w");
            }
            if op == "work.search" {
                args.push("Synthetic");
            }
        }
        args.extend(["--remote", "test"]);
        let out = project.run(&args, false);
        assert!(out.status.success(), "{op}: {out:?}");
        assert!(out.stderr.is_empty());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
        if matches!(op, "work.prepare" | "work.snapshot") {
            for required in [
                "context is incomplete",
                "missing_goals",
                "Required spec text",
                "Keep required facts",
                "context-hash",
                "not_evaluated",
            ] {
                assert!(text.contains(required), "{op}: {required}");
            }
        }
        if op == "work.inbox" {
            assert!(text.contains("more pages remain"));
        }
        if op == "work.next" {
            assert!(text.contains("Resume list truncated"));
            assert!(text.contains("work.recovery"));
        }
        if op == "work.observe" {
            assert!(text.contains("no_session"));
            assert!(text.contains("does not grant execution authority"));
        }
        if op == "capabilities" {
            for query in [
                "session.inspect",
                "claim.inspect",
                "execution.inspect",
                "command.inspect",
                "work.recovery",
            ] {
                assert!(text.contains(query), "{text}");
            }
        }
        server.finish();
    }
}
