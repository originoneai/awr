//! EVO-020: the offline corpus. Every case is a file under `tests/fixtures/context-kernel/` with the codes it must give;
//! together they must exercise every code the contract has, in both directions (accepted and refused).
use awr_context_kernel::*;
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/context-kernel")
}

struct Case {
    file: String,
    kind: String,
    request: PackRequest,
    result: Option<PackResult>,
    limits: Limits,
    expect: Vec<String>,
}

fn limits_from(overrides: Option<&Value>) -> Limits {
    let mut limits = Limits::default();
    let Some(Value::Object(map)) = overrides else {
        return limits;
    };
    for (name, value) in map {
        let n = value.as_u64().expect("a limit is a number");
        match name.as_str() {
            "max_chunks" => limits.max_chunks = n as usize,
            "max_groups" => limits.max_groups = n as usize,
            "max_group_members" => limits.max_group_members = n as usize,
            "max_entities_per_chunk" => limits.max_entities_per_chunk = n as usize,
            "max_pinned_entities" => limits.max_pinned_entities = n as usize,
            "max_chunk_bytes" => limits.max_chunk_bytes = n as usize,
            "max_total_bytes" => limits.max_total_bytes = n as usize,
            "max_binding_bytes" => limits.max_binding_bytes = n as usize,
            other => panic!("unknown limit {other}"),
        }
    }
    limits
}

fn cases() -> Vec<Case> {
    let mut files: Vec<_> = fs::read_dir(corpus_dir())
        .expect("the corpus directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let file = path.file_name().unwrap().to_string_lossy().to_string();
            let value: Value = serde_json::from_slice(&fs::read(&path).unwrap())
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            let request: PackRequest = serde_json::from_value(value["request"].clone())
                .unwrap_or_else(|e| panic!("{file}: request: {e}"));
            let result = value.get("result").map(|r| {
                let mut r = r.clone();
                if r["request_digest"] == "@request" {
                    r["request_digest"] = Value::String(request_digest(&request));
                }
                serde_json::from_value::<PackResult>(r)
                    .unwrap_or_else(|e| panic!("{file}: result: {e}"))
            });
            Case {
                file,
                kind: value["kind"].as_str().unwrap().into(),
                request,
                result,
                limits: limits_from(value.get("limits")),
                expect: value["expect"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c.as_str().unwrap().to_string())
                    .collect(),
            }
        })
        .collect()
}

fn codes(issues: &[PackIssue]) -> Vec<String> {
    let unique: BTreeSet<&str> = issues.iter().map(|i| i.code.as_str()).collect();
    unique.into_iter().map(String::from).collect()
}

#[test]
fn every_case_gives_exactly_the_codes_it_lists() {
    let cases = cases();
    assert!(cases.len() >= 60, "the corpus shrank: {}", cases.len());
    for case in &cases {
        let issues = match case.kind.as_str() {
            "request" => validate_request(&case.request, &case.limits),
            "result" => check_result(
                &case.request,
                case.result.as_ref().expect("a result case has a result"),
                &case.limits,
            ),
            other => panic!("{}: unknown kind {other}", case.file),
        };
        assert_eq!(codes(&issues), case.expect, "{}: {issues:?}", case.file);
    }
}

#[test]
fn the_corpus_exercises_every_code_and_both_outcomes() {
    let cases = cases();
    let seen: BTreeSet<String> = cases.iter().flat_map(|c| c.expect.clone()).collect();
    for code in PackIssueCode::ALL {
        assert!(
            seen.contains(code.as_str()),
            "no case gives {}",
            code.as_str()
        );
    }
    for name in &seen {
        assert!(
            PackIssueCode::ALL.iter().any(|c| c.as_str() == name),
            "{name} is not a code"
        );
    }
    let accepted = cases.iter().filter(|c| c.expect.is_empty()).count();
    let refused = cases.len() - accepted;
    assert!(
        accepted >= 12 && refused >= 45,
        "accepted {accepted}, refused {refused}"
    );
    // Both directions, for requests and for results.
    for kind in ["request", "result"] {
        assert!(
            cases.iter().any(|c| c.kind == kind && c.expect.is_empty()),
            "no accepted {kind}"
        );
        assert!(
            cases.iter().any(|c| c.kind == kind && !c.expect.is_empty()),
            "no refused {kind}"
        );
    }
}

#[test]
fn the_names_in_the_corpus_are_unique_and_numbered() {
    let cases = cases();
    let mut names = BTreeSet::new();
    for (index, case) in cases.iter().enumerate() {
        assert!(
            case.file.starts_with(&format!("{:03}-", index + 1)),
            "{} is out of order",
            case.file
        );
        assert!(names.insert(case.file.clone()), "{} twice", case.file);
        assert!(case.file.is_ascii(), "{} is not ASCII", case.file);
    }
}

#[test]
fn the_digest_names_the_request_whatever_the_order_of_its_keys() {
    let case = cases().into_iter().next().unwrap();
    let digest = request_digest(&case.request);
    assert!(
        digest.starts_with("sha256:") && digest.len() == 71,
        "{digest}"
    );
    // The same request with its JSON keys written in another order.
    let mut text = serde_json::to_string(&case.request).unwrap();
    let reordered: Value = serde_json::from_str(&text).unwrap();
    let mut fields: Vec<(String, Value)> = reordered
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    fields.reverse();
    text = format!(
        "{{{}}}",
        fields
            .iter()
            .map(|(k, v)| format!("{}:{}", Value::String(k.clone()), v))
            .collect::<Vec<_>>()
            .join(",")
    );
    let again: PackRequest = serde_json::from_str(&text).unwrap();
    assert_eq!(request_digest(&again), digest);
    // A change of anything that matters is another digest.
    let mut changed = case.request.clone();
    changed.required[0].text.push('!');
    assert_ne!(request_digest(&changed), digest);
    let mut regrouped = case.request.clone();
    regrouped.groups.push(AtomicGroup {
        id: "g".into(),
        members: vec![ChunkRef {
            section: "decisions".into(),
            key: "note".into(),
        }],
    });
    assert_ne!(request_digest(&regrouped), digest);
    let mut rebudgeted = case.request.clone();
    rebudgeted.budget.limit += 1;
    assert_ne!(request_digest(&rebudgeted), digest);
    let mut rebound = case.request.clone();
    rebound.request_binding = serde_json::json!({"branch": "other"});
    assert_ne!(request_digest(&rebound), digest);
    let mut nested = case.request.clone();
    nested.request_binding = serde_json::json!({"b": {"y": 1, "x": 2}, "a": [3, 1]});
    let mut nested_again = case.request.clone();
    nested_again.request_binding = serde_json::json!({"a": [3, 1], "b": {"x": 2, "y": 1}});
    assert_eq!(request_digest(&nested), request_digest(&nested_again));
    let mut order_of_a_list = case.request.clone();
    order_of_a_list.request_binding = serde_json::json!({"a": [1, 3], "b": {"x": 2, "y": 1}});
    assert_ne!(
        request_digest(&order_of_a_list),
        request_digest(&nested_again)
    );
}
