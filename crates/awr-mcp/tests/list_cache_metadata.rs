//! `tools/list` must carry the cache metadata that protocol 2026-07-28 requires
//! (`ttlMs` and `cacheScope`, SEP-2549). Hosts that validate the result strictly,
//! such as DeepSeek Harness 0.2.2-rc.1, treat a result without them as invalid and
//! report the server as disconnected.
//!
//! These tests drive the real binary over raw JSON-RPC, so they do not depend on
//! how a particular client SDK treats an absent field. The project directory is
//! empty: the server answers list requests without initializing a project.
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn inline_request(id: u64, method: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "list-cache-metadata-test", "version": "0"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }}
    })
}

/// Send each line to a fresh `awr-mcp` and return the responses that carry an id,
/// in the order the requests were sent. A notification (no id) expects no reply.
fn exchange(requests: &[Value]) -> Vec<Value> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("awr-mcp-list-cache-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_awr-mcp"))
        .arg("--project")
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });

    let mut responses = Vec::new();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        if request.get("id").is_none() {
            continue;
        }
        let line = received
            .recv_timeout(Duration::from_secs(20))
            .expect("awr-mcp must answer every request");
        responses.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_dir_all(&root);
    responses
}

fn assert_cache_metadata(result: &Value) {
    assert_eq!(
        result["ttlMs"],
        json!(0),
        "tools/list needs an explicit ttlMs: {result}"
    );
    assert_eq!(
        result["cacheScope"],
        json!("private"),
        "tools/list needs an explicit cacheScope: {result}"
    );
}

#[test]
fn inline_lifecycle_tools_list_carries_the_same_cache_metadata_as_discover() {
    let responses = exchange(&[
        inline_request(1, "server/discover"),
        inline_request(2, "tools/list"),
    ]);
    let discover = &responses[0]["result"];
    let list = &responses[1]["result"];
    assert!(responses[1].get("error").is_none(), "{}", responses[1]);
    assert!(
        list["tools"].as_array().is_some_and(|t| !t.is_empty()),
        "the catalog must be listed: {list}"
    );
    assert_cache_metadata(list);
    assert_eq!(list["ttlMs"], discover["ttlMs"]);
    assert_eq!(list["cacheScope"], discover["cacheScope"]);
}

#[test]
fn initialize_lifecycle_tools_list_is_listed_and_well_formed() {
    let responses = exchange(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "list-cache-metadata-test", "version": "0"}
        }}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ]);
    let list = &responses[1]["result"];
    assert!(responses[1].get("error").is_none(), "{}", responses[1]);
    assert!(
        list["tools"].as_array().is_some_and(|t| !t.is_empty()),
        "legacy sessions must still list the catalog: {list}"
    );
    // Older protocol versions ignore the two fields, so they may be present; when
    // they are, they must be well formed.
    if let Some(ttl) = list.get("ttlMs") {
        assert!(ttl.is_u64(), "{list}");
    }
    if let Some(scope) = list.get("cacheScope") {
        assert!(
            matches!(scope.as_str(), Some("public" | "private")),
            "{list}"
        );
    }
}
