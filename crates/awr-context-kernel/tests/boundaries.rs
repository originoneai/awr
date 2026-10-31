//! EVO-020: what the kernel is not. It needs no store, no database, no source reader; calling it initializes nothing; its
//! answers say nothing of authority; and what it takes is plain data that a host can produce without our types.
use awr_context_kernel::*;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::PathBuf, process::Command};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn request() -> Value {
    json!({
        "protocol_version": 1,
        "policy": "awr.whole_chunks.v1",
        "identity": "host:thing@1",
        "budget": {"unit": "bounded_bytes", "limit": 100, "exactness": "exact", "scope": "bytes of the text"},
        "required": [{"key": "a", "section": "s", "text": "required"}]
    })
}

/// The packages the kernel's normal dependencies lead to, found by asking cargo, without building anything.
fn dependency_closure() -> (BTreeSet<String>, BTreeSet<String>) {
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
        ])
        .arg(manifest_dir().join("Cargo.toml"))
        .output()
        .expect("cargo metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let nodes = metadata["resolve"]["nodes"].as_array().unwrap();
    let name_of = |id: &str| -> String {
        metadata["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .map(|p| p["name"].as_str().unwrap().to_string())
            .unwrap()
    };
    let root = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "awr-context-kernel")
        .map(|p| p["id"].as_str().unwrap().to_string())
        .expect("the kernel package");
    let normal = |node: &Value| -> Vec<String> {
        node["deps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|dep| {
                dep["dep_kinds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|k| k["kind"].is_null())
            })
            .map(|dep| dep["pkg"].as_str().unwrap().to_string())
            .collect()
    };
    let by_id = |id: &str| nodes.iter().find(|n| n["id"] == id).unwrap();
    let direct: BTreeSet<String> = normal(by_id(&root)).iter().map(|id| name_of(id)).collect();
    let mut seen = BTreeSet::new();
    let mut queue = vec![root.clone()];
    while let Some(id) = queue.pop() {
        if seen.insert(id.clone()) {
            queue.extend(normal(by_id(&id)));
        }
    }
    (direct, seen.iter().map(|id| name_of(id)).collect())
}

#[test]
fn the_dependency_closure_needs_no_store_no_database_and_no_source_reader() {
    let (direct, closure) = dependency_closure();
    assert_eq!(
        direct,
        ["serde", "serde_json", "sha2"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "the direct dependencies"
    );
    for forbidden in [
        "awr-core",
        "awr-store",
        "awr-source",
        "awr-context",
        "awr-runtime",
        "awr-team",
        "awr-team-pg",
        "awr-server",
        "awr-cli",
        "awr-mcp",
        "awr-workspace",
        "rusqlite",
        "libsqlite3-sys",
        "tokio",
        "tokio-postgres",
        "postgres",
        "ureq",
        "reqwest",
        "hyper",
        "tiktoken-rs",
        "cap-std",
        "cap-fs-ext",
        "yaml-rust2",
        "serde_yaml_ng",
        "pulldown-cmark",
        "toml",
    ] {
        assert!(
            !closure.contains(forbidden),
            "{forbidden} is in the dependency closure of the kernel"
        );
    }
}

#[test]
fn the_source_of_the_kernel_touches_no_file_network_process_environment_or_clock() {
    for entry in fs::read_dir(manifest_dir().join("src")).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        for needle in [
            "std::fs",
            "std::net",
            "std::process",
            "std::env",
            "std::thread",
            "SystemTime",
            "Instant::",
            "std::time",
            "std::io",
            "File::",
            "TcpStream",
            "Command::new",
            "env::var",
            "rusqlite",
            "tokio",
            "unsafe",
        ] {
            assert!(!text.contains(needle), "{} uses {needle}", path.display());
        }
    }
}

#[test]
fn calling_the_kernel_creates_nothing_where_it_runs() {
    let before: BTreeSet<String> = fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let here: BTreeSet<String> = fs::read_dir(manifest_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let parsed: PackRequest = serde_json::from_value(request()).unwrap();
    assert!(validate_request(&parsed, &Limits::default()).is_empty());
    let _ = request_digest(&parsed);
    let after: BTreeSet<String> = fs::read_dir(manifest_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        here, after,
        "calling the kernel changed the directory it ran in"
    );
    assert!(!manifest_dir().join(".awr").exists());
    // The temporary directory is shared with other tests; the point is that no `.awr` or database was made for this call.
    let created: Vec<String> = fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !before.contains(n) && (n.contains("awr") || n.ends_with(".db")))
        .collect();
    assert!(created.iter().all(|n| !n.contains("kernel")), "{created:?}");
}

fn keys(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                out.push(key.to_lowercase());
                keys(inner, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keys(item, out)),
        _ => {}
    }
}

#[test]
fn nothing_the_kernel_gives_says_anything_of_authority_readiness_or_acceptance() {
    let case: Value =
        serde_json::from_slice(&fs::read(corpus_file("001-accepted-minimal.json")).unwrap())
            .unwrap();
    let parsed: PackRequest = serde_json::from_value(case["request"].clone()).unwrap();
    let result = PackResult {
        protocol_version: 1,
        policy: parsed.policy.clone(),
        request_digest: request_digest(&parsed),
        selected: vec![Selection {
            section: "acceptance".into(),
            key: "acceptance".into(),
            required: true,
            group: None,
        }],
        omitted: vec![Omission {
            section: "decisions".into(),
            key: "note".into(),
            reason: OmissionReason::Budget,
            group: None,
        }],
        measurement: Measurement {
            unit: BudgetUnit::BoundedBytes,
            exactness: Exactness::Exact,
            scope: "bytes".into(),
            tokenizer: None,
            limit: 400,
            required_used: 30,
            total_used: 30,
        },
        rendered_hash: format!("sha256:{}", "ab".repeat(32)),
        scope_note: SCOPE_NOTE.into(),
    };
    let mut names = Vec::new();
    keys(&serde_json::to_value(&result).unwrap(), &mut names);
    let issue = PackIssue {
        code: PackIssueCode::MandatoryOverflow,
        section: Some("s".into()),
        key: Some("k".into()),
    };
    keys(&serde_json::to_value(&issue).unwrap(), &mut names);
    for word in [
        "authori",
        "admission",
        "permission",
        "ready",
        "readiness",
        "complete",
        "accepted",
        "acceptance",
        "claim",
        "approved",
        "verified",
        "valid",
    ] {
        assert!(
            names.iter().all(|n| !n.contains(word)),
            "a field says {word}: {names:?}"
        );
    }
    // The note a result carries says that it grants none of this, and is fixed.
    assert!(result.scope_note.contains("grants no authority"));
    let mut forged = result.clone();
    forged.scope_note = "ready".into();
    let codes: Vec<PackIssueCode> = check_result(&parsed, &forged, &Limits::default())
        .iter()
        .map(|i| i.code)
        .collect();
    assert!(codes.contains(&PackIssueCode::ResultScopeNote), "{codes:?}");
    let honest: Vec<PackIssueCode> = check_result(&parsed, &result, &Limits::default())
        .iter()
        .map(|i| i.code)
        .collect();
    assert!(
        !honest.contains(&PackIssueCode::ResultScopeNote),
        "{honest:?}"
    );
}

fn corpus_file(name: &str) -> PathBuf {
    manifest_dir()
        .join("../../tests/fixtures/context-kernel")
        .join(name)
}

#[test]
fn what_the_kernel_takes_is_plain_data_and_every_unknown_field_is_refused() {
    // A host with ids of its own, no database key anywhere: it is accepted as it is.
    let mut host = request();
    host["pinned_entities"] = json!([{"kind": "jira-issue", "id": "ABC-12", "revision": 7}]);
    host["required"][0]["entities"] =
        json!([{"kind": "jira-issue", "id": "ABC-12", "revision": 7}]);
    let parsed: PackRequest = serde_json::from_value(host.clone()).unwrap();
    assert!(validate_request(&parsed, &Limits::default()).is_empty());
    // Nothing is taken that a host would have to write for us: a field we do not know is refused, wherever it stands.
    for (path, extra) in [
        (vec![], ("authorized", json!(true))),
        (vec![], ("store_handle", json!("db"))),
        (vec!["budget"], ("writable", json!(true))),
        (vec!["required", "0"], ("claim", json!("session"))),
        (
            vec!["required", "0", "entities", "0"],
            ("database_id", json!(1)),
        ),
    ] {
        let mut value = host.clone();
        let mut at = &mut value;
        for step in &path {
            at = match step.parse::<usize>() {
                Ok(index) => &mut at[index],
                Err(_) => &mut at[*step],
            };
        }
        at[extra.0] = extra.1.clone();
        if path.last() == Some(&"0") && path.len() == 4 {
            // the entity path above ends at an entity
        }
        assert!(
            serde_json::from_value::<PackRequest>(value).is_err(),
            "{path:?} {}",
            extra.0
        );
    }
}

#[test]
fn a_request_that_is_refused_is_refused_by_name_and_a_good_one_is_not_changed() {
    let parsed: PackRequest = serde_json::from_value(request()).unwrap();
    let before = serde_json::to_value(&parsed).unwrap();
    let issues = validate_request(&parsed, &Limits::default());
    assert!(issues.is_empty());
    assert_eq!(
        serde_json::to_value(&parsed).unwrap(),
        before,
        "validation must not change the request"
    );
    // Asking again gives the same answer.
    let mut over = parsed.clone();
    over.required[0].text = "x".repeat(101);
    assert_eq!(
        validate_request(&over, &Limits::default()),
        validate_request(&over, &Limits::default())
    );
    assert_eq!(
        validate_request(&over, &Limits::default())[0].code,
        PackIssueCode::MandatoryOverflow
    );
    // The error text carries codes and identifiers, never the text of a chunk.
    let shown = validate_request(&over, &Limits::default())
        .iter()
        .map(|i| i.to_string())
        .collect::<String>();
    assert!(!shown.contains("xxxx"), "{shown}");
}

#[test]
fn an_issue_names_a_chunk_by_a_short_plain_identifier_and_never_by_its_text() {
    let mut parsed: PackRequest = serde_json::from_value(request()).unwrap();
    let long_key = format!("{}\u{7}{}", "k".repeat(10), "k".repeat(200));
    parsed.required.push(Chunk {
        key: "a".into(),
        section: "s".into(),
        section_rank: 0,
        encoding: "utf-8".into(),
        media: "text/plain".into(),
        text: "secret body text".into(),
        entities: vec![],
        cost: None,
    });
    parsed.required.push(Chunk {
        key: long_key.clone(),
        section: "s".into(),
        text: "other".into(),
        ..parsed.required[0].clone()
    });
    parsed.required.push(Chunk {
        key: long_key,
        section: "s".into(),
        text: "again".into(),
        ..parsed.required[0].clone()
    });
    let issues = validate_request(
        &parsed,
        &Limits {
            max_name_bytes: 400,
            ..Limits::default()
        },
    );
    let named: Vec<&PackIssue> = issues.iter().filter(|i| i.key.is_some()).collect();
    assert!(!named.is_empty(), "{issues:?}");
    for issue in named {
        let key = issue.key.as_deref().unwrap();
        assert!(key.len() <= 80, "{key}");
        assert!(!key.chars().any(char::is_control), "{key:?}");
        assert!(!issue.to_string().contains("secret body text"));
    }
}
