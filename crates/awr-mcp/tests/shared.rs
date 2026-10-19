//! Real TCP/HTTP requests to the native MCP process with independent synthetic projects.
use awr_core::*;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    time::timeout,
};

const WRITER: &str = "synthetic-writer-credential-for-http-fixtures";
const READER: &str = "synthetic-reader-credential-for-http-fixtures";
const COLLEAGUE: &str = "synthetic-colleague-credential-for-http-fixtures";

fn diagnostic(result: Value, path: &str, constraint: &str) -> Value {
    error(result.clone(), "InvalidInput");
    let detail = result["structuredContent"].clone();
    assert_eq!(detail["invalid_field"], path, "{result}");
    assert_eq!(detail["constraint"], constraint, "{result}");
    assert!(!result.to_string().contains("private-fixture"));
    assert!(!result.to_string().contains("unexpected-input-name"));
    detail
}

#[tokio::test]
async fn personal_parser_diagnostics_preserve_optional_nulls_and_exact_receipts() {
    for mode in ["flat", "hierarchical"] {
        let a = ProjectFixture::new("Review parser attribution");
        let b = ProjectFixture::new("Separate parser attribution");
        let config = registry(&a, &b);
        let mut server = Server::start_mode(&config, mode).await;
        let source = fs::read(a.root.join("work.yaml")).unwrap();
        let started = ok(server
            .call(
                WRITER,
                "awr_session_start",
                start_args("alpha", "W", "parser", a.revision(), true),
            )
            .await);
        let sid = started["session"]["id"].clone();
        let revision = a.revision();
        diagnostic(
            server
                .raw_call(
                    WRITER,
                    "awr_search",
                    json!({"project":"alpha","kind":"private-fixture-kind","limit":-1}),
                )
                .await,
            "/limit",
            "parse",
        );
        assert_eq!(a.revision(), revision);
        for (name, mut args, path, constraint) in [
            (
                "awr_session_claim",
                json!({"action":"private-fixture-action","claim":"private-fixture-id","request_id":"mixed-claim"}),
                "/claim",
                "parse",
            ),
            (
                "awr_session_checkpoint",
                json!({"agent":null,"context_hash":"a".repeat(64),"digest":false,"next_action":"Review","request_id":"null-agent"}),
                "/digest",
                "type",
            ),
        ] {
            args["project"] = json!("alpha");
            args["session"] = sid.clone();
            args["expected_revision"] = json!(a.revision());
            let rejected = server.raw_call(WRITER, name, args.clone()).await;
            let mut live = diagnostic(rejected, path, constraint);
            let receipt = ok(server
                .raw_call(
                    WRITER,
                    "awr_operation_get",
                    json!({"project":"alpha","request_id":args["request_id"]}),
                )
                .await);
            assert!(receipt["domain_receipts"].as_array().unwrap().is_empty());
            live.as_object_mut().unwrap().remove("operation");
            live.as_object_mut()
                .unwrap()
                .remove("receipt_project_revision");
            assert_eq!(live, receipt["operation"]["result"]);
            let revision = a.revision();
            server.stop().await;
            server = Server::start_mode(&config, mode).await;
            let repeated = server.raw_call(WRITER, name, args).await;
            let mut replay = diagnostic(repeated, path, constraint);
            assert_eq!(replay["operation"]["replayed"], true);
            replay.as_object_mut().unwrap().remove("operation");
            replay
                .as_object_mut()
                .unwrap()
                .remove("receipt_project_revision");
            assert_eq!(live, replay);
            assert_eq!(a.revision(), revision);
        }
        let session = ok(server
            .raw_call(
                WRITER,
                "awr_session_get",
                json!({"project":"alpha","session":sid}),
            )
            .await);
        assert_eq!(session["session"]["status"], "active");
        assert!(
            session["claims"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["status"] == "active")
        );
        let saved = ok(server.call(WRITER, "awr_session_checkpoint", json!({"project":"alpha","session":sid,"expected_revision":a.revision(),"agent":null,"context_hash":"a".repeat(64),"digest":"Review","next_action":"Review"})).await);
        assert_eq!(saved["checkpoint_save"]["actor"]["origin"], "undeclared");
        assert!(saved.get("invalid_field").is_none());
        assert_eq!(source, fs::read(a.root.join("work.yaml")).unwrap());
        server.stop().await;
    }
}

#[tokio::test]
async fn personal_diagnostics_follow_the_actual_admission_rejection() {
    for mode in ["flat", "hierarchical"] {
        let a = ProjectFixture::new("Review admission diagnostics");
        let b = ProjectFixture::new("Separate admission diagnostics");
        let mut server = Server::start_mode(&registry(&a, &b), mode).await;
        let start = ok(server
            .call(
                WRITER,
                "awr_session_start",
                start_args("alpha", "W", "admission", a.revision(), true),
            )
            .await);
        let sid = start["session"]["id"].clone();
        let revision = a.revision();
        let source = fs::read(a.root.join("work.yaml")).unwrap();
        let cases = [
            (
                "awr_search",
                json!({"project":"alpha","limit":0,"text":false}),
                "/text",
                "type",
            ),
            (
                "awr_search",
                json!({"project":"alpha","kind":"private-fixture-kind","text":false}),
                "/text",
                "type",
            ),
            (
                "awr_work_get",
                json!({"project":"alpha","explain":"private-fixture-value"}),
                "/explain",
                "type",
            ),
            (
                "awr_session_end",
                json!({"project":"alpha","session":sid,"expected_revision":revision,"request_id":false}),
                "/request_id",
                "type",
            ),
            (
                "awr_session_end",
                json!({"project":"alpha","session":sid,"expected_revision":revision,"outcome":"ended"}),
                "/request_id",
                "required",
            ),
            (
                "awr_session_end",
                json!({"project":"alpha","session":sid,"expected_revision":revision}),
                "/request_id",
                "required",
            ),
            (
                "awr_session_end",
                json!({"project":"alpha","session":"private-fixture-id","expected_revision":revision,"request_id":"bad-session","outcome":false}),
                "/session",
                "format",
            ),
            (
                "awr_session_end",
                json!({"project":"alpha","conversation":false,"expected_revision":revision,"request_id":"bad-conversation","outcome":false}),
                "/conversation",
                "type",
            ),
        ];
        for (name, args, path, constraint) in cases {
            let rejected = server.raw_call(WRITER, name, args).await;
            diagnostic(rejected, path, constraint);
            assert_eq!(a.revision(), revision);
        }
        let denied = server
            .raw_call(
                READER,
                "awr_session_end",
                json!({"project":"alpha","request_id":false}),
            )
            .await;
        error(denied.clone(), "RuleViolation");
        assert!(denied["structuredContent"].get("invalid_field").is_none());
        let stale = server.raw_call(WRITER, "awr_session_end", json!({"project":"alpha","session":sid,"outcome":false,"expected_revision":0,"request_id":"stale-diagnostic"})).await;
        error(stale.clone(), "RevisionConflict");
        assert!(stale["structuredContent"].get("invalid_field").is_none());
        assert_eq!(a.revision(), revision);
        assert_eq!(source, fs::read(a.root.join("work.yaml")).unwrap());
        server.stop().await;
    }
}

#[tokio::test]
async fn personal_diagnostics_check_payload_type_and_survive_receipt_failure() {
    let a = ProjectFixture::new("Review payload diagnostics");
    let b = ProjectFixture::new("Separate payload diagnostics");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let source = fs::read(a.root.join("work.yaml")).unwrap();
    let args = json!({"project":"alpha","expected_revision":a.revision(),"request_id":"payload-type","event_type":"review.observed","summary":"Review the input","payload":false});
    let rejected = server
        .raw_call(WRITER, "awr_event_append", args.clone())
        .await;
    let mut normalized = diagnostic(rejected, "/payload", "type");
    let receipt = ok(server
        .call(
            WRITER,
            "awr_operation_get",
            json!({"project":"alpha","request_id":"payload-type"}),
        )
        .await);
    normalized.as_object_mut().unwrap().remove("operation");
    normalized
        .as_object_mut()
        .unwrap()
        .remove("receipt_project_revision");
    assert_eq!(normalized, receipt["operation"]["result"]);
    let revision = a.revision();
    server.stop().await;
    let mut server = Server::start(&config).await;
    let repeated = server.raw_call(WRITER, "awr_event_append", args).await;
    let mut replay = repeated["structuredContent"].clone();
    assert_eq!(replay["operation"]["replayed"], true);
    replay.as_object_mut().unwrap().remove("operation");
    replay
        .as_object_mut()
        .unwrap()
        .remove("receipt_project_revision");
    assert_eq!(replay, normalized);
    assert_eq!(a.revision(), revision);

    let conn = rusqlite::Connection::open(a.root.join(".awr/state.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fixture_deny_diagnostic_receipt BEFORE INSERT ON events WHEN new.event_type='mcp.operation_finished' BEGIN SELECT RAISE(ABORT,'synthetic diagnostic receipt failure'); END;").unwrap();
    let bad = json!({"project":"alpha","expected_revision":a.revision(),"request_id":"lost-diagnostic","event_type":"review.observed","summary":"Review the input","payload":false});
    let unknown = server
        .raw_call(WRITER, "awr_event_append", bad.clone())
        .await;
    assert_eq!(unknown["isError"], true);
    assert_eq!(unknown["structuredContent"]["write_outcome"], "unknown");
    assert!(unknown["structuredContent"].get("invalid_field").is_none());
    conn.execute_batch("DROP TRIGGER fixture_deny_diagnostic_receipt;")
        .unwrap();
    drop(conn);
    let revision = a.revision();
    server.stop().await;
    let mut server = Server::start(&config).await;
    let repeated = server.raw_call(WRITER, "awr_event_append", bad).await;
    assert_eq!(repeated["structuredContent"]["write_outcome"], "unknown");
    let recovered = server
        .call(
            WRITER,
            "awr_operation_recover",
            json!({"project":"alpha","request_id":"lost-diagnostic","expected_revision":revision}),
        )
        .await;
    assert_eq!(recovered["structuredContent"]["write_outcome"], "unknown");
    assert_eq!(a.revision(), revision);
    assert_eq!(source, fs::read(a.root.join("work.yaml")).unwrap());
    server.stop().await;
}

#[tokio::test]
async fn legacy_personal_error_receipts_remain_unchanged_after_restart() {
    use sha2::Digest;
    let a = ProjectFixture::new("Review a legacy receipt");
    let b = ProjectFixture::new("Separate legacy receipt");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let started = ok(server
        .call(
            WRITER,
            "awr_session_start",
            start_args("alpha", "W", "legacy", a.revision(), true),
        )
        .await);
    let original = json!({"session":started["session"]["id"],"outcome":false,"expected_revision":a.revision()});
    let fingerprint = format!(
        "{:x}",
        sha2::Sha256::digest(
            serde_json::to_vec(&json!({"tool":"awr_session_end","arguments":original})).unwrap()
        )
    );
    let mut store = Store::open_existing(&a.root.join(".awr/state.db")).unwrap();
    let operation = store
        .begin_mcp_operation(
            a.id,
            a.revision(),
            "writer",
            "legacy-error",
            "awr_session_end",
            &fingerprint,
        )
        .unwrap();
    let (saved, _) = store.finish_mcp_operation(a.id, operation.started_revision, &operation, json!({"code":"InvalidInput","message":"invalid input: tool arguments do not match the tool schema"}), true, false).unwrap();
    let legacy = saved.result.unwrap();
    drop(store);
    let revision = a.revision();
    server.stop().await;
    let mut server = Server::start(&config).await;
    let mut args = original;
    args["project"] = json!("alpha");
    args["request_id"] = json!("legacy-error");
    let repeated = server.raw_call(WRITER, "awr_session_end", args).await;
    error(repeated.clone(), "InvalidInput");
    assert_eq!(repeated["structuredContent"]["operation"]["replayed"], true);
    assert!(repeated["structuredContent"].get("invalid_field").is_none());
    let inspected = ok(server
        .call(
            WRITER,
            "awr_operation_get",
            json!({"project":"alpha","request_id":"legacy-error"}),
        )
        .await);
    assert_eq!(inspected["operation"]["result"], legacy);
    assert_eq!(a.revision(), revision);
    let session = ok(server
        .call(
            WRITER,
            "awr_session_get",
            json!({"project":"alpha","session":started["session"]["id"]}),
        )
        .await);
    assert_eq!(session["session"]["status"], "active");
    server.stop().await;
}

#[tokio::test]
async fn personal_input_diagnostics_preserve_admission_and_durable_error_replay() {
    for mode in ["flat", "hierarchical"] {
        let a = ProjectFixture::new("Review diagnostic guidance");
        let b = ProjectFixture::new("Separate diagnostic guidance");
        let config = registry(&a, &b);
        let mut server = Server::start_mode(&config, mode).await;
        let source = fs::read(a.root.join("work.yaml")).unwrap();
        let start = ok(server
            .call(
                WRITER,
                "awr_session_start",
                start_args("alpha", "W", "diagnostic-window", a.revision(), true),
            )
            .await);
        let sid = start["session"]["id"].clone();

        let revision = a.revision();
        let read = server
            .call(WRITER, "awr_work_get", json!({"project":"alpha","work":17}))
            .await;
        diagnostic(read, "/work", "type");
        assert_eq!(a.revision(), revision);

        let missing = server
            .call(
                WRITER,
                "awr_session_end",
                json!({"project":"alpha","session":sid,"expected_revision":a.revision()}),
            )
            .await;
        diagnostic(missing, "/outcome", "required");

        let checkpoint = server
            .call(
                WRITER,
                "awr_session_checkpoint",
                json!({"project":"alpha","session":sid,"expected_revision":a.revision(),
                    "context_hash":"a".repeat(64),"digest":false,"next_action":"Review the guide"}),
            )
            .await;
        diagnostic(checkpoint, "/digest", "type");

        let evidence = server
            .call(
                WRITER,
                "awr_evidence_record",
                json!({"project":"alpha","expected_revision":a.revision(),"external_key":"guide-check",
                    "evidence_type":"verification_report","level":"unknown","summary":"Review the guide",
                    "locator":"guide-report.json","scope":"guide"}),
            )
            .await;
        diagnostic(evidence, "/scope", "type");

        let bad = json!({"project":"alpha","session":sid,"expected_revision":a.revision(),
            "outcome":"ended","request_id":"diagnostic-end","unexpected-input-name":"private-fixture-content"});
        let rejected = server.call(WRITER, "awr_session_end", bad.clone()).await;
        let detail = diagnostic(rejected, "/", "unexpected_field");
        assert!(
            detail["expected_fields"]
                .as_array()
                .unwrap()
                .contains(&json!("outcome"))
        );

        let revision = a.revision();
        let replay = ok(server
            .call(
                WRITER,
                "awr_operation_get",
                json!({"project":"alpha","request_id":"diagnostic-end"}),
            )
            .await);
        assert_eq!(
            replay["operation"]["result"]["invalid_field"],
            detail["invalid_field"]
        );
        assert_eq!(
            replay["operation"]["result"]["constraint"],
            detail["constraint"]
        );
        let repeated = server.call(WRITER, "awr_session_end", bad).await;
        error(repeated.clone(), "InvalidInput");
        assert_eq!(repeated["structuredContent"]["operation"]["replayed"], true);
        assert_eq!(
            repeated["structuredContent"]["expected_fields"],
            detail["expected_fields"]
        );
        assert_eq!(a.revision(), revision);
        let session = ok(server
            .call(
                WRITER,
                "awr_session_get",
                json!({"project":"alpha","session":sid}),
            )
            .await);
        assert_eq!(session["session"]["status"], "active");
        assert!(
            session["claims"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["status"] == "active")
        );

        let denied = server
            .call(READER, "awr_session_end", json!({"project":"alpha","session":sid,
                "expected_revision":a.revision(),"outcome":"ended","unexpected-input-name":"private-fixture-content"}))
            .await;
        error(denied.clone(), "RuleViolation");
        assert!(denied["structuredContent"].get("invalid_field").is_none());
        assert_eq!(a.revision(), revision);

        let ended = ok(server
            .call(
                WRITER,
                "awr_session_end",
                json!({"project":"alpha","session":sid,
                "expected_revision":a.revision(),"outcome":"ended","request_id":"corrected-end"}),
            )
            .await);
        assert_eq!(ended["session"]["status"], "ended");
        assert!(ended.get("invalid_field").is_none());
        assert_eq!(source, fs::read(a.root.join("work.yaml")).unwrap());
        server.stop().await;
    }
}

#[tokio::test]
async fn compaction_http_isolated_recoverable_and_wait_guidance_has_priority() {
    let a = ProjectFixture::new("Continue the guide");
    let b = ProjectFixture::new("Separate guide");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let start = ok(server
        .call(
            WRITER,
            "awr_session_start",
            start_args("alpha", "W", "compact-window", a.revision(), true),
        )
        .await);
    let sid = start["session"]["id"].clone();
    let observation = json!({"compaction_id":"host-compact-1","sequence":1,"observed_at":now_millis().unwrap(),"trigger":"automatic","model":"fixture-model","source":"fixture.full_request","measurement_scope":"full_request","measurement_basis":"host_reported","after_tokens":180000,"before_tokens":230000,"context_window_tokens":256000});
    let args = json!({"project":"alpha","conversation":"compact-window","expected_revision":a.revision(),"request_id":"compaction-one","observation":observation,"policy":{"post_compaction_threshold_percent":60}});
    error(
        server
            .call(READER, "awr_compaction_observe", args.clone())
            .await,
        "RuleViolation",
    );
    let recorded = ok(server
        .call(WRITER, "awr_compaction_observe", args.clone())
        .await);
    assert_eq!(recorded["state"], "handoff_candidate");
    assert_eq!(recorded["session_switch_performed"], false);
    let revision = a.revision();
    let replay = ok(server.call(WRITER, "awr_compaction_observe", args).await);
    assert_eq!(replay["recorded_event_id"], recorded["recorded_event_id"]);
    assert_eq!(replay["operation"]["replayed"], true);
    assert_eq!(revision, a.revision());
    error(
        server
            .call(
                COLLEAGUE,
                "awr_compaction_get",
                json!({"project":"alpha","session":sid}),
            )
            .await,
        "RuleViolation",
    );
    error(
        server
            .call(
                WRITER,
                "awr_compaction_get",
                json!({"project":"beta","session":sid}),
            )
            .await,
        "NotFound",
    );
    let prep = ok(server
        .call(
            WRITER,
            "awr_work_prepare",
            json!({"project":"alpha","work":"W","session":sid,"response_view":"action"}),
        )
        .await);
    assert!(
        prep["guidance"]["next_action"]
            .as_str()
            .unwrap()
            .contains("ask before opening")
    );
    let wait=ok(server.call(WRITER,"awr_session_wait",json!({"project":"alpha","session":sid,"expected_revision":a.revision(),"request_id":"switch-question","question":"Continue this work in a fresh window?","context_hash":prep["context"]["work_context"]["context_hash"],"digest":"Checkpoint preserves the guide state","next_action":"Await user decision"})).await);
    let waiting = ok(server
        .call(
            WRITER,
            "awr_compaction_get",
            json!({"project":"alpha","conversation":"compact-window"}),
        )
        .await);
    assert!(
        waiting["guidance"]["next_action"]
            .as_str()
            .unwrap()
            .contains("do not repeat")
    );
    ok(server.call(WRITER,"awr_session_reply",json!({"project":"alpha","wait":wait["wait"]["id"],"reply":"Continue here for now","expected_revision":a.revision(),"request_id":"postpone-reply"})).await);
    let deferred=ok(server.call(WRITER,"awr_compaction_defer",json!({"project":"alpha","session":sid,"observation_event_id":recorded["observation_event_id"],"expected_revision":a.revision(),"request_id":"postpone-compaction"})).await);
    assert_eq!(deferred["state"], "deferred");
    let conn = rusqlite::Connection::open(a.root.join(".awr/state.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fixture_lost_compaction_response BEFORE INSERT ON events WHEN new.event_type='mcp.operation_finished' BEGIN SELECT RAISE(ABORT,'synthetic lost compaction response'); END;").unwrap();
    let mut observation = observation;
    observation["compaction_id"] = json!("host-compact-2");
    observation["sequence"] = json!(2);
    let args = json!({"project":"alpha","session":sid,"expected_revision":a.revision(),"request_id":"compaction-lost-response","observation":observation});
    let unknown = server
        .call(WRITER, "awr_compaction_observe", args.clone())
        .await;
    assert_eq!(unknown["structuredContent"]["write_outcome"], "unknown");
    let rev = a.revision();
    server.call(WRITER, "awr_compaction_observe", args).await;
    assert_eq!(rev, a.revision());
    conn.execute_batch("DROP TRIGGER fixture_lost_compaction_response;")
        .unwrap();
    drop(conn);
    ok(server.call(WRITER,"awr_operation_recover",json!({"project":"alpha","request_id":"compaction-lost-response","expected_revision":a.revision()})).await);
    let recovered = ok(server
        .call(
            WRITER,
            "awr_compaction_get",
            json!({"project":"alpha","session":sid,"include_observation":true}),
        )
        .await);
    assert_eq!(recovered["observation"]["sequence"], 2);
    assert_eq!(recovered["state"], "handoff_candidate");
    assert!(recovered["observation"]["usage"].is_null());
    server.stop().await;
}

#[path = "../../../tests/common/compact_indexes.rs"]
mod index_contract;

#[tokio::test]
async fn default_concise_http_keeps_full_details_in_the_original_project() {
    for mode in ["flat", "hierarchical"] {
        let a = ProjectFixture::new("Review concise prepare indexes");
        let b = ProjectFixture::new("Separate project");
        let mut source = fs::read_to_string(a.root.join("work.yaml")).unwrap();
        for n in 0..40 {
            source.push_str(&format!(
                "- id: HISTORY-{n}\n  title: Review support item {n}\n  status: completed\n"
            ));
        }
        fs::write(a.root.join("work.yaml"), source).unwrap();
        a.reindex();
        let mut server = Server::start_mode(&registry(&a, &b), mode).await;
        let args = json!({"project":"alpha","work":"W","branch":"main","goals":["GUIDE"],"source_sha":"a".repeat(40),"budget":100000});
        let initial = ok(server.call(WRITER, "awr_work_prepare", args.clone()).await);
        let pack = &initial["context"]["work_context"];
        let budgets = [
            100000,
            pack["token_estimate"].as_u64().unwrap() - 1,
            pack["required_tokens"].as_u64().unwrap() + 50,
        ];
        let revision = a.revision();
        let mut last_query = Value::Null;
        let mut last_concise = Value::Null;
        for (case, budget) in budgets.into_iter().enumerate() {
            let mut args = args.clone();
            args["budget"] = json!(budget);
            let full = ok(server.call(WRITER, "awr_work_prepare", args.clone()).await);
            let count = full["context"]["work_context"]["omitted_chunks"]
                .as_array()
                .unwrap()
                .len();
            match case {
                0 => assert_eq!(count, 0),
                1 => assert_eq!(count, 1),
                _ => assert!(count > 10),
            }
            for view in ["summary", "action"] {
                let mut args = args.clone();
                args["response_view"] = json!(view);
                let concise = ok(server.call(WRITER, "awr_work_prepare", args).await);
                index_contract::assert_prepared_view(&full, &concise, count);
                let query = &concise["response_view"]["full_result"];
                assert_eq!(query["arguments"]["branch"], "main");
                assert_eq!(query["arguments"]["goals"], json!(["GUIDE"]));
                assert_eq!(query["arguments"]["source_sha"], "a".repeat(40));
                assert_eq!(query["arguments"]["budget"], budget);
                // Project routing is transport metadata; retain the original explicit key.
                let mut query_args = query["arguments"].clone();
                query_args["project"] = json!("alpha");
                let detail = ok(server
                    .call(WRITER, query["tool"].as_str().unwrap(), query_args.clone())
                    .await);
                assert_eq!(
                    detail["context"]["work_context"]["identity"]["project_id"],
                    json!(a.id)
                );
                assert_eq!(
                    detail["context"]["work_context"]["omitted_chunks"],
                    full["context"]["work_context"]["omitted_chunks"]
                );
                assert_eq!(detail["project_revision"], concise["project_revision"]);
                assert_eq!(
                    detail["context"]["work_context"]["context_hash"],
                    concise["context"]["work_context"]["context_hash"]
                );
                last_query = query_args;
                last_concise = concise;
            }
        }
        assert_eq!(a.revision(), revision, "prepare presentation is read-only");
        let baseline = server
            .call(
                WRITER,
                "awr_work_prepare",
                json!({"project":"alpha","work":"W","budget":1}),
            )
            .await;
        assert_eq!(baseline["isError"], true);
        for view in ["summary", "action"] {
            let failed = server
                .call(
                    WRITER,
                    "awr_work_prepare",
                    json!({"project":"alpha","work":"W","budget":1,"response_view":view}),
                )
                .await;
            assert_eq!(failed, baseline);
        }
        fs::write(
            a.root.join("work.yaml"),
            fs::read_to_string(a.root.join("work.yaml"))
                .unwrap()
                .replace("Draft the guide", "Review the guide"),
        )
        .unwrap();
        a.reindex();
        let changed = ok(server.call(WRITER, "awr_work_prepare", last_query).await);
        assert_ne!(
            changed["project_revision"],
            last_concise["project_revision"]
        );
        assert_ne!(
            changed["context"]["work_context"]["context_hash"],
            last_concise["context"]["work_context"]["context_hash"]
        );
        server.stop().await;
    }
}

#[tokio::test]
async fn concise_http_results_keep_client_isolation_and_unknown_outcomes() {
    let a = ProjectFixture::new("Write a concise team guide");
    let b = ProjectFixture::new("Separate project");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let started = ok(server
        .call(
            WRITER,
            "awr_session_start",
            start_args("alpha", "W", "concise", a.revision(), true),
        )
        .await);
    let query = json!({"project":"alpha","work":"W","session":started["session"]["id"]});
    let full = ok(server.call(WRITER, "awr_work_prepare", query.clone()).await);
    let mut query_summary = query;
    query_summary["response_view"] = json!("summary");
    let concise = ok(server.call(WRITER, "awr_work_prepare", query_summary).await);
    assert_eq!(
        full["context"]["work_context"]["rendered_context"],
        concise["context"]["work_context"]["rendered_context"]
    );
    assert_eq!(full["management"], concise["management"]);
    for budget in [1, 100000] {
        let args = json!({"project":"alpha","work":"W","session":started["session"]["id"],"response_view":"action","budget":budget});
        let result = server.call(WRITER, "awr_work_prepare", args).await;
        assert!(
            result["structuredContent"]["response_view"]
                .get("detail_counts")
                .is_none()
        );
        assert!(
            result["structuredContent"]["response_view"]
                .get("detail_note")
                .is_none()
        );
        if budget == 1 {
            assert_eq!(result["isError"], true);
        } else {
            assert_eq!(
                ok(result)["context"]["work_context"]["omitted_chunks"],
                json!([])
            );
        }
    }
    let args = json!({"project":"alpha","work":"W","session":started["session"]["id"],"action":"progress","reason":"Reviewed guide context","next_action":"Review the draft guide","request_id":"concise-write","expected_revision":a.revision(),"response_view":"summary"});
    let conn = rusqlite::Connection::open(a.root.join(".awr/state.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fixture_concise_response BEFORE INSERT ON events WHEN new.event_type='mcp.operation_finished' BEGIN SELECT RAISE(ABORT,'synthetic lost response'); END;").unwrap();
    let unknown = server
        .call(WRITER, "awr_work_transition", args.clone())
        .await;
    assert_eq!(unknown["structuredContent"]["write_outcome"], "unknown");
    assert!(unknown["structuredContent"].get("response_view").is_none());
    let revision = a.revision();
    let mut original = args.clone();
    original["response_view"] = json!("full");
    let replay = server
        .call(WRITER, "awr_work_transition", original.clone())
        .await;
    assert_eq!(replay["structuredContent"], unknown["structuredContent"]);
    assert_eq!(a.revision(), revision);
    error(
        server
            .call(
                COLLEAGUE,
                "awr_operation_get",
                json!({"project":"alpha","request_id":"concise-write"}),
            )
            .await,
        "NotFound",
    );
    conn.execute_batch("DROP TRIGGER fixture_concise_response;")
        .unwrap();
    drop(conn);
    ok(server.call(WRITER,"awr_operation_recover",json!({"project":"alpha","request_id":"concise-write","expected_revision":a.revision()})).await);
    let receipt = ok(server
        .call(
            WRITER,
            "awr_operation_get",
            json!({"project":"alpha","request_id":"concise-write"}),
        )
        .await);
    assert_eq!(receipt["outcome"], "recorded");
    assert!(!receipt["domain_receipts"].as_array().unwrap().is_empty());
    let source = fs::read(a.root.join("work.yaml")).unwrap();
    ok(server.call(WRITER, "awr_work_transition", original).await);
    assert_eq!(source, fs::read(a.root.join("work.yaml")).unwrap());
    server.stop().await;
}

struct ProjectFixture {
    root: PathBuf,
    id: Id,
}
impl ProjectFixture {
    fn workstreams() -> Self {
        Self::workstreams_with("")
    }
    /// The workstream ledger plus extra `work_items` entries (two-space list items) appended at the end.
    fn workstreams_with(extra_items: &str) -> Self {
        let fixture = Self::new("Workstream fixture");
        fs::write(fixture.root.join(".awr/project.toml"), "[project]\nname='Workstream HTTP fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-workstream-ledger-v1'\n").unwrap();
        let ledger = include_str!("../../../tests/fixtures/workstreams/context.yaml");
        fs::write(fixture.root.join("work.yaml"), format!("{ledger}\n  - id: API-2\n    title: Review the interface\n    status: planned\n    workstream: api\n    acceptance: [Review the interface]\n{extra_items}")).unwrap();
        fixture.reindex();
        fixture
    }
    fn reindex(&self) {
        let mut store = Store::open_existing(&self.root.join(".awr/state.db")).unwrap();
        let report = index_project(
            &mut store,
            &self.root,
            &Manifest::load(&self.root).unwrap(),
            false,
        )
        .unwrap();
        assert!(report.ok, "{report:?}");
    }
    fn new(title: &str) -> Self {
        let path = std::env::temp_dir().join(format!("awr-shared-project-{}", Id::new()));
        fs::create_dir_all(path.join(".awr")).unwrap();
        let root = path.canonicalize().unwrap();
        fs::write(root.join(".awr/project.toml"), "[project]\nname='Shared MCP fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        fs::write(root.join("work.yaml"), format!("goals:\n- id: GUIDE\n  title: Deliver useful team guidance\n  status: active\n  summary: Review and deliver the team guide and follow-up\n  success_criteria: [The reviewed guidance is available]\nwork_items:\n- id: W\n  title: {title}\n  status: ready\n  goal: GUIDE\n  next_action: Draft the guide\n  acceptance: [Deliver the reviewed guide]\n- id: NEXT\n  title: Prepare follow-up\n  status: ready\n  goal: GUIDE\n  next_action: Draft the follow-up\n  acceptance: [Deliver the follow-up]\n")).unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let report =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(report.ok);
        Self {
            root,
            id: report.project_id,
        }
    }
    fn revision(&self) -> Revision {
        Store::open_readonly(&self.root.join(".awr/state.db"))
            .unwrap()
            .project(self.id)
            .unwrap()
            .project_revision
    }
}
impl Drop for ProjectFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Server {
    child: Child,
    url: String,
}
impl Server {
    async fn start(registry: &std::path::Path) -> Self {
        Self::start_mode(registry, "hierarchical").await
    }
    async fn start_mode(registry: &std::path::Path, mode: &str) -> Self {
        Self::start_with_team(registry, mode, None).await
    }
    async fn start_with_team(registry: &std::path::Path, mode: &str, team: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_awr-mcp"));
        command.env_remove("AWR_TEAM_ENABLED");
        if let Some(value) = team {
            command.env("AWR_TEAM_ENABLED", value);
        }
        let mut child = command
            .arg("--registry")
            .arg(registry)
            .arg("--listen")
            .arg("127.0.0.1:0")
            .env("AWR_FIXTURE_WRITER", WRITER)
            .env("AWR_FIXTURE_READER", READER)
            .env("AWR_FIXTURE_COLLEAGUE", COLLEAGUE)
            .env("AWR_MCP_TOOL_EXPOSURE_MODE", mode)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let line = timeout(Duration::from_secs(15), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let url = line
            .strip_prefix("AWR MCP listening at ")
            .unwrap_or_else(|| panic!("startup: {line}"))
            .to_owned();
        Self { child, url }
    }
    async fn stop(&mut self) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
    }
    async fn call(&self, credential: &str, name: &str, mut arguments: Value) -> Value {
        if awr_mcp::tools()
            .iter()
            .find(|tool| tool.name == name)
            .is_some_and(|tool| tool.annotations.as_ref().unwrap().read_only_hint == Some(false))
            && name != "awr_operation_recover"
            && arguments.get("request_id").is_none()
        {
            arguments["request_id"] = json!(Id::new().to_string());
        }
        self.raw_call(credential, name, arguments).await
    }
    async fn rpc(
        &self,
        credential: &str,
        method: &str,
        name: Option<&str>,
        mut params: Value,
    ) -> Value {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientCapabilities":{},
            "io.modelcontextprotocol/clientInfo":{"name":"synthetic-http-client","version":"1"}
        });
        let mut request = reqwest::Client::new()
            .post(&self.url)
            .bearer_auth(credential)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method);
        if let Some(name) = name {
            request = request.header("Mcp-Name", name);
        }
        let response = request
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json().await.unwrap()
    }
    async fn tool_names(&self, credential: &str) -> Vec<String> {
        let body = self.rpc(credential, "tools/list", None, json!({})).await;
        body["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{body}"))
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }
    async fn raw_call(&self, credential: &str, name: &str, arguments: Value) -> Value {
        let response = reqwest::Client::new().post(&self.url).bearer_auth(credential)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", name)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":name,"arguments":arguments,"_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities":{},
                    "io.modelcontextprotocol/clientInfo":{"name":"synthetic-http-client","version":"1"}
                }}})).send().await.unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{body}");
        assert!(body.get("error").is_none(), "{body}");
        body["result"].clone()
    }
}
fn ok(result: Value) -> Value {
    assert_ne!(result["isError"], true, "{result}");
    result["structuredContent"].clone()
}
fn error(result: Value, code: &str) {
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["code"], code, "{result}");
}
fn registry(a: &ProjectFixture, b: &ProjectFixture) -> PathBuf {
    let path = a.root.join("service.toml");
    fs::write(&path, format!("version=1\n[[projects]]\nkey='alpha'\nroot={}\nproject_id='{}'\n[[projects]]\nkey='beta'\nroot={}\nproject_id='{}'\n[[clients]]\nid='writer'\ntoken_env='AWR_FIXTURE_WRITER'\nwrite=['alpha','beta']\n[[clients]]\nid='reader'\ntoken_env='AWR_FIXTURE_READER'\nread=['alpha']\n[[clients]]\nid='colleague'\ntoken_env='AWR_FIXTURE_COLLEAGUE'\nwrite=['alpha']\n", json!(a.root), a.id, json!(b.root), b.id)).unwrap();
    path
}

fn start_args(
    project: &str,
    work: &str,
    conversation: &str,
    revision: Revision,
    claim: bool,
) -> Value {
    json!({"project":project,"work":work,"conversation":conversation,"agent":"guide-editor","provider":"synthetic","model":"fixture","expected_revision":revision,"claim":claim,"request_id":Id::new().to_string()})
}

fn attach_local_continuity(
    store: &mut Store,
    project: Id,
    session: Id,
    external: &str,
) -> ClientBinding {
    let revision = store.project(project).unwrap().project_revision;
    let binding = ClientBinding {
        client: "generic".into(),
        external_session: external.into(),
        session_id: session,
        revision: 0,
        digest: "Local continuity fixture".into(),
        next_action: "Inspect the current work".into(),
        open_loops: vec![],
        context_hash: Some("a".repeat(64)),
        context_revision: Some(revision),
        progress_revision: revision,
        last_delivery_key: None,
        last_hook_event: None,
        checkpoint_id: None,
        observed_at: now_millis().unwrap(),
    };
    store
        .save_client_binding(project, revision, binding, false)
        .unwrap()
        .0
}

#[tokio::test]
async fn local_and_mcp_bindings_stay_namespace_scoped_and_bridge_by_session_id() {
    for mode in ["flat", "hierarchical"] {
        let a = ProjectFixture::new("Namespace binding fixture");
        let b = ProjectFixture::new("Separate namespace fixture");
        let config = registry(&a, &b);
        let config_text = fs::read_to_string(&config)
            .unwrap()
            .replace("id='writer'", "id='generic'");
        fs::write(&config, config_text).unwrap();
        let source = fs::read(a.root.join("work.yaml")).unwrap();

        let local_only_session = {
            let mut store = Store::open_existing(&a.root.join(".awr/state.db")).unwrap();
            let revision = store.project(a.id).unwrap().project_revision;
            let (started, _) = store
                .start_session(
                    a.id,
                    revision,
                    SessionDraft {
                        work_item_key: Some("W".into()),
                        agent_id: "local-agent".into(),
                        provider: "fixture".into(),
                        model: "local".into(),
                        branch_id: None,
                        claim: false,
                        claim_ttl_ms: None,
                    },
                )
                .unwrap();
            let binding = attach_local_continuity(
                &mut store,
                a.id,
                started.session.id,
                "namespace-collision",
            );
            assert_eq!(binding.session_id, started.session.id);
            started.session.id
        };

        let mut server = Server::start_mode(&config, mode).await;
        error(
            server
                .call(
                    WRITER,
                    "awr_session_get",
                    json!({"project":"alpha","conversation":"namespace-collision"}),
                )
                .await,
            "NotFound",
        );
        error(
            server
                .call(
                    WRITER,
                    "awr_session_get",
                    json!({"project":"alpha","session":local_only_session}),
                )
                .await,
            "RuleViolation",
        );
        error(
            server
                .call(
                    WRITER,
                    "awr_session_resume",
                    json!({"project":"alpha","session":local_only_session,"conversation":"namespace-collision","agent":"local-agent","provider":"fixture","model":"local","expected_revision":a.revision()}),
                )
                .await,
            "RuleViolation",
        );
        assert!(
            Store::open_readonly(&a.root.join(".awr/state.db"))
                .unwrap()
                .resumed_successor(a.id, local_only_session)
                .unwrap()
                .is_none()
        );
        let listed = ok(server
            .call(
                WRITER,
                "awr_session_list",
                json!({"project":"alpha","limit":100}),
            )
            .await);
        assert!(
            listed["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|session| session["id"] != json!(local_only_session))
        );

        let started = ok(server
            .call(
                WRITER,
                "awr_session_start",
                start_args("alpha", "W", "bridge-conversation", a.revision(), false),
            )
            .await);
        let bridge_session: Id = started["session"]["id"].as_str().unwrap().parse().unwrap();
        {
            let mut store = Store::open_existing(&a.root.join(".awr/state.db")).unwrap();
            let binding =
                attach_local_continuity(&mut store, a.id, bridge_session, "bridge-conversation");
            assert_eq!(
                store
                    .mcp_bound_session(
                        a.id,
                        &McpSessionBinding {
                            client: "generic".into(),
                            conversation: "bridge-conversation".into(),
                        },
                    )
                    .unwrap()
                    .unwrap()
                    .id,
                bridge_session
            );
            assert_eq!(
                store
                    .client_binding(a.id, "generic", "bridge-conversation")
                    .unwrap()
                    .unwrap()
                    .session_id,
                binding.session_id
            );
        }
        let inspected = ok(server
            .call(
                WRITER,
                "awr_session_get",
                json!({"project":"alpha","conversation":"bridge-conversation"}),
            )
            .await);
        assert_eq!(inspected["session"]["id"], json!(bridge_session));
        error(
            server
                .call(
                    COLLEAGUE,
                    "awr_session_get",
                    json!({"project":"alpha","conversation":"bridge-conversation"}),
                )
                .await,
            "NotFound",
        );
        error(
            server
                .call(
                    COLLEAGUE,
                    "awr_session_get",
                    json!({"project":"alpha","session":bridge_session}),
                )
                .await,
            "RuleViolation",
        );

        let resumed = ok(
            server
                .call(
                    WRITER,
                    "awr_session_resume",
                    json!({"project":"alpha","session":bridge_session,"conversation":"bridge-conversation","agent":"successor-agent","provider":"synthetic","model":"fixture","expected_revision":a.revision()}),
                )
                .await,
        );
        let successor: Id = resumed["resumed"]["session"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(successor, bridge_session);
        {
            let store = Store::open_readonly(&a.root.join(".awr/state.db")).unwrap();
            assert_eq!(
                store
                    .mcp_session_binding(a.id, bridge_session)
                    .unwrap()
                    .unwrap()
                    .conversation,
                "bridge-conversation"
            );
            assert_eq!(
                store
                    .mcp_bound_session(
                        a.id,
                        &McpSessionBinding {
                            client: "generic".into(),
                            conversation: "bridge-conversation".into(),
                        },
                    )
                    .unwrap()
                    .unwrap()
                    .id,
                successor
            );
            assert_eq!(
                store
                    .client_binding(a.id, "generic", "bridge-conversation")
                    .unwrap()
                    .unwrap()
                    .session_id,
                bridge_session
            );
            assert_eq!(
                store
                    .client_binding(a.id, "generic", "namespace-collision")
                    .unwrap()
                    .unwrap()
                    .session_id,
                local_only_session
            );
        }
        let current = ok(server
            .call(
                WRITER,
                "awr_session_get",
                json!({"project":"alpha","conversation":"bridge-conversation"}),
            )
            .await);
        assert_eq!(current["session"]["id"], json!(successor));
        error(
            server
                .call(
                    COLLEAGUE,
                    "awr_session_get",
                    json!({"project":"alpha","conversation":"bridge-conversation"}),
                )
                .await,
            "NotFound",
        );
        error(
            server
                .call(
                    COLLEAGUE,
                    "awr_session_get",
                    json!({"project":"alpha","session":successor}),
                )
                .await,
            "RuleViolation",
        );
        assert_eq!(source, fs::read(a.root.join("work.yaml")).unwrap());
        server.stop().await;
    }
}

#[tokio::test]
async fn checkpoint_progress_keeps_source_authority_and_caller_declarations_over_http() {
    let a = ProjectFixture::new("Progress fixture");
    let b = ProjectFixture::new("Separate project");
    let original = fs::read(a.root.join("work.yaml")).unwrap();
    let mut server = Server::start(&registry(&a, &b)).await;
    let started = ok(server
        .call(
            WRITER,
            "awr_session_start",
            start_args("alpha", "W", "progress", a.revision(), true),
        )
        .await);
    let sid = started["session"]["id"].clone();
    let mut args = json!({"project":"alpha","session":sid,"expected_revision":started["session"]["revision"],
        "context_hash":"a".repeat(64),"digest":"Test failed; work remains incomplete","next_action":"Investigate failure","open_loops":["Retest"],"agent":"guide-editor"});
    let stale = server
        .call(WRITER, "awr_session_checkpoint", args.clone())
        .await;
    error(stale.clone(), "RevisionConflict");
    assert_eq!(
        stale["structuredContent"]["details"]["revision_scope"],
        "project"
    );
    args["expected_revision"] = json!(a.revision());
    args["agent"] = json!("other-agent");
    let mismatch = server
        .call(WRITER, "awr_session_checkpoint", args.clone())
        .await;
    error(mismatch.clone(), "RuleViolation");
    assert!(
        mismatch["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("session resume")
    );
    assert!(
        ok(server
            .call(
                WRITER,
                "awr_session_get",
                json!({"project":"alpha","session":sid})
            )
            .await)["checkpoint"]
            .is_null()
    );
    args["agent"] = json!("guide-editor");
    args["expected_revision"] = json!(a.revision());
    args["request_id"] = json!("declared-checkpoint");
    error(
        server
            .call(COLLEAGUE, "awr_session_checkpoint", args.clone())
            .await,
        "RuleViolation",
    );
    let saved = ok(server
        .call(WRITER, "awr_session_checkpoint", args.clone())
        .await);
    assert_eq!(
        saved["checkpoint_save"]["actor"]["agent_id"],
        "guide-editor"
    );
    assert_eq!(
        saved["checkpoint_save"]["actor"]["identity_verified"],
        false
    );
    assert_eq!(saved["context_hash_verified"], false);
    let replay = ok(server.call(WRITER, "awr_session_checkpoint", args).await);
    assert_eq!(replay["checkpoint_save"], saved["checkpoint_save"]);
    assert_eq!(replay["operation"]["replayed"], true);
    let detail = ok(server
        .call(
            WRITER,
            "awr_work_get",
            json!({"project":"alpha","work":"W"}),
        )
        .await);
    assert_eq!(
        detail["progress"]["source_next_action"]["text"],
        "Draft the guide"
    );
    assert_eq!(
        detail["progress"]["latest_checkpoint_next_action"]["text"],
        "Investigate failure"
    );
    for view in ["summary", "action"] {
        let status = ok(server
            .call(
                WRITER,
                "awr_project_status",
                json!({"project":"alpha","work":["W"],"view":view}),
            )
            .await);
        assert_eq!(status["progress"], detail["progress"]);
    }
    let shown = ok(server
        .call(
            WRITER,
            "awr_session_get",
            json!({"project":"alpha","session":sid}),
        )
        .await);
    assert_eq!(
        shown["checkpoint_save"]["actor"],
        saved["checkpoint_save"]["actor"]
    );
    assert_eq!(shown["checkpoint_write_revision"]["scope"], "project");
    let other = ok(server
        .call(WRITER, "awr_work_get", json!({"project":"beta","work":"W"}))
        .await);
    assert!(other["progress"]["latest_checkpoint_next_action"].is_null());
    assert_eq!(fs::read(a.root.join("work.yaml")).unwrap(), original);
    server.stop().await;
}

#[tokio::test]
async fn source_changes_preserve_client_scope_reviewed_versions_and_explicit_draft_activation() {
    let a = ProjectFixture::new("Write the team guide");
    let b = ProjectFixture::new("Write the other guide");
    let beta_before = b.revision();
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let input = json!({"project":"alpha","request_id":"new-reviewed-draft","reason":"Add the reviewed appendix","change":{"kind":"create","title":"Prepare the appendix","fields":{"goal":"GUIDE","acceptance":["The appendix is reviewed"],"next_action":"Draft the appendix"}}});
    let before = a.revision();
    let preview = ok(server
        .call(WRITER, "awr_change_preview", input.clone())
        .await);
    assert_eq!(a.revision(), before);
    let mut apply = input.clone();
    apply["expected_revision"] = preview["project_revision"].clone();
    apply["expected_preview"] = preview["preview"]["fingerprint"].clone();
    let readonly = server.call(READER, "awr_change_apply", apply.clone()).await;
    assert_eq!(readonly["isError"], true);
    error(
        server
            .call(COLLEAGUE, "awr_change_apply", apply.clone())
            .await,
        "SourceConflict",
    );
    let created = ok(server.call(WRITER, "awr_change_apply", apply.clone()).await);
    let key = created["external_key"].as_str().unwrap();
    let revision = a.revision();
    let private_status =
        json!({"project":"alpha","kind":"create","request_id":"new-reviewed-draft"});
    assert_eq!(
        ok(server
            .call(COLLEAGUE, "awr_change_status", private_status.clone())
            .await)["found"],
        false
    );
    let mut beta_status = private_status.clone();
    beta_status["project"] = json!("beta");
    assert_eq!(
        ok(server.call(WRITER, "awr_change_status", beta_status).await)["found"],
        false
    );
    assert_eq!(
        ok(server.call(WRITER, "awr_change_apply", apply).await)["already_recorded"],
        true
    );
    assert_eq!(a.revision(), revision);
    let graph = ok(server
        .call(
            WRITER,
            "awr_work_graph",
            json!({"project":"alpha","roots":[key]}),
        )
        .await);
    let node = &graph["nodes"][0];
    assert_eq!(node["status"], "draft");
    assert_eq!(node["ready"], false);
    let activate = json!({"project":"alpha","request_id":"activate-appendix","reason":"Accept the explicit appendix plan","change":{"kind":"edit","change":{"operation":"activate_draft","work":key,"source_fingerprint":node["source_ref"]["source_fingerprint"]}}});
    let p = ok(server
        .call(WRITER, "awr_change_preview", activate.clone())
        .await);
    // Another client can claim independent work; that still invalidates the reviewed revision.
    ok(server
        .call(
            COLLEAGUE,
            "awr_session_start",
            start_args("alpha", "NEXT", "independent", a.revision(), true),
        )
        .await);
    let mut activation = activate.clone();
    activation["expected_revision"] = p["project_revision"].clone();
    activation["expected_preview"] = p["preview"]["fingerprint"].clone();
    error(
        server
            .call(WRITER, "awr_change_apply", activation.clone())
            .await,
        "RevisionConflict",
    );
    let p = ok(server.call(WRITER, "awr_change_preview", activate).await);
    activation["expected_revision"] = p["project_revision"].clone();
    activation["expected_preview"] = p["preview"]["fingerprint"].clone();
    ok(server.call(WRITER, "awr_change_apply", activation).await);
    let graph = ok(server
        .call(
            WRITER,
            "awr_work_graph",
            json!({"project":"alpha","roots":[key]}),
        )
        .await);
    assert_eq!(graph["nodes"][0]["ready"], true);
    assert_eq!(b.revision(), beta_before);
    server.stop().await;
}

#[tokio::test]
async fn partial_source_batch_is_queryable_after_restart_and_recovers_without_repeating_the_write()
{
    let a = ProjectFixture::new("Write the team guide");
    let b = ProjectFixture::new("Write the other guide");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let store = Store::open_readonly(&a.root.join(".awr/state.db")).unwrap();
    let source = store.work_item(a.id, "W").unwrap().source;
    drop(store);
    let input = json!({"project":"alpha","request_id":"save-dependency-plan","reason":"Apply both reviewed task drafts","change":{"kind":"batch","change":{"kind":"ledger","source_id":source.id,"source_fingerprint":source.fingerprint,"operations":[
        {"operation":"import","external_key":"APPENDIX","title":"Draft appendix","duplicate":"fail","fields":{"depends_on":["INPUT"]}},
        {"operation":"import","external_key":"INPUT","title":"Review input","duplicate":"fail","fields":{}}
    ]}}});
    let p = ok(server
        .call(WRITER, "awr_change_preview", input.clone())
        .await);
    let mut apply = input;
    apply["expected_revision"] = p["project_revision"].clone();
    apply["expected_preview"] = p["preview"]["fingerprint"].clone();
    let db = rusqlite::Connection::open(a.root.join(".awr/state.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_source_index BEFORE UPDATE OF fingerprint ON sources WHEN NEW.fingerprint<>OLD.fingerprint BEGIN SELECT RAISE(ABORT,'synthetic index interruption'); END;").unwrap();
    let failed = server.call(WRITER, "awr_change_apply", apply).await;
    assert_eq!(failed["isError"], true);
    assert_eq!(failed["structuredContent"]["source_write_performed"], true);
    assert_eq!(failed["structuredContent"]["status"], "pending_recovery");
    let after_write = fs::read(a.root.join("work.yaml")).unwrap();
    db.execute_batch("DROP TRIGGER fail_source_index;").unwrap();
    drop(db);
    server.stop().await;
    server = Server::start(&config).await;
    let status = ok(server
        .call(
            WRITER,
            "awr_change_status",
            json!({"project":"alpha","kind":"batch","request_id":"save-dependency-plan"}),
        )
        .await);
    assert_eq!(status["status"], "pending_recovery");
    let recover = json!({"project":"alpha","kind":"batch","request_id":"save-dependency-plan","expected_revision":a.revision()});
    let recovered = ok(server
        .call(WRITER, "awr_change_recover", recover.clone())
        .await);
    assert_eq!(recovered["status"], "completed");
    assert_eq!(recovered["source_write_performed"], false);
    assert_eq!(fs::read(a.root.join("work.yaml")).unwrap(), after_write);
    let revision = a.revision();
    assert_eq!(
        ok(server.call(WRITER, "awr_change_recover", recover).await)["already_recorded"],
        true
    );
    assert_eq!(a.revision(), revision);
    let graph = ok(server
        .call(
            WRITER,
            "awr_work_graph",
            json!({"project":"alpha","roots":["INPUT"]}),
        )
        .await);
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(graph["graph_valid"], true);
    server.stop().await;
}

#[tokio::test]
async fn official_sdk_clients_discover_and_call_the_same_http_endpoint() {
    let a = ProjectFixture::new("Write the team guide");
    let b = ProjectFixture::new("Write the other guide");
    let config = registry(&a, &b);
    // This test covers the Team-enabled catalog; the switch-off catalog is
    // covered by `team_tools_follow_the_runtime_switch`.
    let mut server = Server::start_with_team(&config, "hierarchical", Some("1")).await;
    for (credential, expected_projects) in [(WRITER, 2), (READER, 1)] {
        let transport = StreamableHttpClientTransport::with_client(
            reqwest::Client::new(),
            StreamableHttpClientTransportConfig::with_uri(server.url.clone())
                .auth_header(credential),
        );
        let client = timeout(Duration::from_secs(15), ().serve(transport))
            .await
            .unwrap()
            .unwrap();
        // Default hierarchical exposure advertises project discovery, the
        // workstream tool, confirmed handoff, and review-rule explanation.
        // Flat child names remain callable for integrated hosts.
        let tools = client.list_all_tools().await.unwrap();
        assert_eq!(tools.len(), awr_mcp::domains::DOMAINS.len() + 4);
        assert!(tools.iter().any(|tool| tool.name == "awr_projects_list"));
        assert!(tools.iter().any(|tool| tool.name == "awr_team_handoff"));
        assert!(tools.iter().any(|tool| tool.name == "awr_team_review"));
        assert!(tools.iter().all(|tool| {
            tool.name == "awr_projects_list"
                || tool.name == "awr_workstream"
                || tool.name == "awr_team_handoff"
                || tool.name == "awr_team_review"
                || awr_mcp::domains::is_public_domain(&tool.name)
        }));
        let manifest = client
            .call_tool(
                CallToolRequestParams::new("awr_continuity")
                    .with_arguments(json!({"project":"alpha"}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        let value = manifest.structured_content.unwrap();
        assert_eq!(value["mode"], "manifest");
        assert_eq!(value["child_total"], 5);
        let catalog = client
            .call_tool(CallToolRequestParams::new("awr_projects_list"))
            .await
            .unwrap();
        assert_eq!(
            catalog.structured_content.unwrap()["projects"]
                .as_array()
                .unwrap()
                .len(),
            expected_projects
        );
        let status = client
            .call_tool(
                CallToolRequestParams::new("awr_project_status")
                    .with_arguments(json!({"project":"alpha"}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_ne!(status.is_error, Some(true));
        assert_eq!(
            status.structured_content.unwrap()["project_id"],
            json!(a.id)
        );
        client.cancel().await.unwrap();
    }
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-TERM", &server.child.id().unwrap().to_string()])
            .status()
            .await
            .unwrap();
        assert!(status.success());
        assert!(
            timeout(Duration::from_secs(10), server.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
    #[cfg(not(unix))]
    server.stop().await;
}

#[tokio::test]
async fn lifecycle_binds_conversations_and_preserves_checkpoints_across_connections_and_restart() {
    let a = ProjectFixture::new("Write the team guide");
    let b = ProjectFixture::new("Write the other guide");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let args = start_args("alpha", "W", "guide-discussion", a.revision(), true);
    let started = ok(server.call(WRITER, "awr_session_start", args.clone()).await);
    let sid = started["session"]["id"].clone();
    let revision = a.revision();
    let repeat = ok(server.call(WRITER, "awr_session_start", args).await);
    assert_eq!(repeat["session"]["id"], sid);
    assert_eq!(repeat["operation"]["replayed"], true);
    assert_eq!(a.revision(), revision);
    error(
        server
            .call(
                WRITER,
                "awr_session_start",
                start_args("alpha", "NEXT", "guide-discussion", a.revision(), false),
            )
            .await,
        "SourceConflict",
    );
    let other = ok(server
        .call(
            WRITER,
            "awr_session_start",
            start_args("alpha", "NEXT", "follow-up", a.revision(), true),
        )
        .await);
    let beta = ok(server
        .call(
            WRITER,
            "awr_session_start",
            start_args("beta", "W", "guide-discussion", b.revision(), true),
        )
        .await);
    assert_ne!(sid, beta["session"]["id"]);
    let colleague = ok(server
        .call(
            COLLEAGUE,
            "awr_session_start",
            start_args("alpha", "W", "guide-discussion", a.revision(), false),
        )
        .await);
    assert_ne!(sid, colleague["session"]["id"]);
    error(server.call(COLLEAGUE,"awr_session_end",json!({"project":"alpha","session":sid,"outcome":"ended","expected_revision":a.revision()})).await,"RuleViolation");
    error(server.call(WRITER,"awr_session_get",json!({"project":"alpha","session":other["session"]["id"],"conversation":"guide-discussion"})).await,"RuleViolation");
    let context = ok(server
        .call(
            WRITER,
            "awr_context_compile",
            json!({"project":"alpha","conversation":"guide-discussion"}),
        )
        .await);
    let checkpoint=ok(server.call(WRITER,"awr_session_checkpoint",json!({"project":"alpha","conversation":"guide-discussion","expected_revision":a.revision(),"context_hash":context["work_context"]["context_hash"],"digest":"The introduction is drafted; review examples next.","next_action":"Review the examples","open_loops":["Confirm the example order"]})).await);
    let cp = checkpoint["checkpoint"]["id"].clone();
    let listed = ok(server
        .call(
            WRITER,
            "awr_session_list",
            json!({"project":"alpha","limit":1}),
        )
        .await);
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(listed["sessions"][0]["id"], other["session"]["id"]);
    let older = ok(server
        .call(
            WRITER,
            "awr_session_list",
            json!({"project":"alpha","limit":1,"before_revision":listed["next_before_revision"]}),
        )
        .await);
    assert_eq!(older["sessions"][0]["id"], sid);
    server.stop().await;
    let mut server = Server::start(&config).await;
    let restored = ok(server
        .call(
            WRITER,
            "awr_session_get",
            json!({"project":"alpha","conversation":"guide-discussion"}),
        )
        .await);
    assert_eq!(restored["session"]["id"], sid);
    assert_eq!(restored["session"]["status"], "active");
    assert_eq!(restored["checkpoint"]["id"], cp);
    let resumed=ok(server.call(WRITER,"awr_session_resume",json!({"project":"alpha","session":sid,"conversation":"guide-discussion","agent":"guide-editor","provider":"synthetic","model":"fixture","expected_revision":a.revision()})).await);
    assert_eq!(resumed["checkpoint_id"], cp);
    assert_ne!(resumed["resumed"]["session"]["id"], sid);
    assert!(resumed["resumed"]["claim"].is_object());
    let current = ok(server
        .call(
            WRITER,
            "awr_session_get",
            json!({"project":"alpha","conversation":"guide-discussion"}),
        )
        .await);
    assert_eq!(
        current["session"]["id"],
        resumed["resumed"]["session"]["id"]
    );
    assert_eq!(current["inherited_checkpoint"]["id"], cp);
    // Ending one session releases only its claims; another conversation stays active.
    ok(server.call(WRITER,"awr_session_end",json!({"project":"alpha","conversation":"guide-discussion","expected_revision":a.revision(),"outcome":"ended"})).await);
    let other = ok(server
        .call(
            WRITER,
            "awr_session_get",
            json!({"project":"alpha","conversation":"follow-up"}),
        )
        .await);
    assert_eq!(other["session"]["status"], "active");
    assert_eq!(other["claims"][0]["status"], "active");
    // Runtime inspection/cleanup survive unavailable source files.
    fs::remove_file(a.root.join("work.yaml")).unwrap();
    ok(server
        .call(
            WRITER,
            "awr_session_get",
            json!({"project":"alpha","conversation":"follow-up"}),
        )
        .await);
    ok(server.call(WRITER,"awr_session_end",json!({"project":"alpha","conversation":"follow-up","expected_revision":a.revision(),"outcome":"interrupted"})).await);
    server.stop().await;
}

#[tokio::test]
async fn one_endpoint_routes_independent_clients_projects_and_conflicting_writes() {
    let a = ProjectFixture::new("Write the alpha guide");
    let b = ProjectFixture::new("Write the beta guide");
    let mut server = Server::start(&registry(&a, &b)).await;
    let catalog = ok(server.call(READER, "awr_projects_list", json!({})).await);
    assert_eq!(catalog["projects"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["projects"][0]["key"], "alpha");
    assert!(!catalog.to_string().contains(a.root.to_str().unwrap()));
    let (alpha, beta) = tokio::join!(
        server.call(
            READER,
            "awr_work_get",
            json!({"project":"alpha","work":"W"})
        ),
        server.call(WRITER, "awr_work_get", json!({"project":"beta","work":"W"}))
    );
    assert_eq!(ok(alpha)["work"]["title"], "Write the alpha guide");
    assert_eq!(ok(beta)["work"]["title"], "Write the beta guide");
    error(
        server
            .call(READER, "awr_work_get", json!({"project":"beta","work":"W"}))
            .await,
        "RuleViolation",
    );
    error(
        server
            .call(WRITER, "awr_work_get", json!({"project":a.root,"work":"W"}))
            .await,
        "RuleViolation",
    );
    error(
        server
            .call(WRITER, "awr_work_get", json!({"work":"W"}))
            .await,
        "InvalidInput",
    );
    let before = a.revision();
    let event = json!({"project":"alpha","expected_revision":before,"event_type":"work.observed","summary":"Reviewed the guide outline"});
    error(
        server.call(READER, "awr_event_append", event.clone()).await,
        "RuleViolation",
    );
    assert_eq!(a.revision(), before);
    let beta_before = b.revision();
    let (one,two,other_project) = tokio::join!(
        server.call(WRITER,"awr_event_append",event.clone()),
        server.call(WRITER,"awr_event_append",event),
        server.call(WRITER,"awr_event_append",json!({"project":"beta","expected_revision":beta_before,"event_type":"work.observed","summary":"Reviewed beta input"})));
    let results = [one, two];
    assert_eq!(results.iter().filter(|r| r["isError"] != true).count(), 1);
    error(
        results
            .iter()
            .find(|r| r["isError"] == true)
            .unwrap()
            .clone(),
        "RevisionConflict",
    );
    ok(other_project);
    assert_eq!(a.revision(), before + 3);
    assert_eq!(b.revision(), beta_before + 3);
    server.stop().await;
    // A fresh service keeps the same project identities and committed revisions.
    let mut restarted = Server::start(&registry(&a, &b)).await;
    let current = ok(restarted
        .call(
            WRITER,
            "awr_work_get",
            json!({"project":"alpha","work":"W"}),
        )
        .await);
    assert_eq!(current["project_revision"], before + 3);
    restarted.stop().await;
}

#[tokio::test]
async fn every_http_request_requires_credentials_and_allowed_browser_origin() {
    let a = ProjectFixture::new("First guide");
    let b = ProjectFixture::new("Second guide");
    let mut server = Server::start(&registry(&a, &b)).await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(&server.url)
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(&server.url)
            .bearer_auth("incorrect")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(&server.url)
            .bearer_auth(WRITER)
            .header("Origin", "https://unregistered.example")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .post(&server.url)
            .bearer_auth(WRITER)
            .header("Host", "unregistered.example")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    server.stop().await;
}

#[tokio::test]
async fn wait_reply_and_successor_keep_progress_without_automatic_execution() {
    let a = ProjectFixture::new("Prepare the onboarding guide");
    let b = ProjectFixture::new("Prepare the reference guide");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let started = ok(server
        .call(
            WRITER,
            "awr_session_start",
            start_args("alpha", "W", "onboarding", a.revision(), true),
        )
        .await);
    let context = ok(server
        .call(
            WRITER,
            "awr_context_compile",
            json!({"project":"alpha","conversation":"onboarding"}),
        )
        .await);
    let wait_args = json!({"project":"alpha","conversation":"onboarding","expected_revision":a.revision(),"request_id":"ask-example-order","question":"Should the practical example come before the reference material?","context_hash":context["work_context"]["context_hash"],"digest":"The introduction is drafted and the example order needs a decision.","next_action":"Arrange the examples after the reply","open_loops":["Confirm the example order"]});
    let waiting = ok(server
        .call(WRITER, "awr_session_wait", wait_args.clone())
        .await);
    assert_eq!(waiting["wait"]["status"], "waiting_user");
    assert_eq!(
        waiting["wait"]["checkpoint_id"],
        waiting["checkpoint"]["id"]
    );
    let before = a.revision();
    ok(server.call(WRITER, "awr_session_wait", wait_args).await);
    assert_eq!(a.revision(), before);
    error(server.call(WRITER,"awr_work_transition",json!({"project":"alpha","conversation":"onboarding","work":"W","action":"progress","reason":"Proceed with the guide","expected_revision":a.revision(),"next_action":"Edit examples"})).await,"InvalidTransition");
    server.stop().await;
    let mut server = Server::start(&config).await;
    let state = ok(server
        .call(
            WRITER,
            "awr_session_get",
            json!({"project":"alpha","conversation":"onboarding"}),
        )
        .await);
    assert_eq!(state["continuity_state"], "waiting_user");
    assert_eq!(state["checkpoint"]["id"], waiting["checkpoint"]["id"]);
    error(server.call(COLLEAGUE,"awr_session_reply",json!({"project":"alpha","wait":waiting["wait"]["id"],"reply":"A reply from a different client","expected_revision":a.revision()})).await,"RuleViolation");
    let reply = json!({"project":"alpha","wait":waiting["wait"]["id"],"reply":"Put the practical example first, followed by the reference material.","expected_revision":a.revision(),"request_id":"example-order-reply"});
    let answered = ok(server
        .call(WRITER, "awr_session_reply", reply.clone())
        .await);
    assert_eq!(answered["wait"]["status"], "answered");
    let before = a.revision();
    ok(server.call(WRITER, "awr_session_reply", reply).await);
    assert_eq!(a.revision(), before);
    let context = ok(server
        .call(
            WRITER,
            "awr_context_compile",
            json!({"project":"alpha","conversation":"onboarding"}),
        )
        .await);
    assert_eq!(
        context["continuity"]["waits"][0]["reply"],
        answered["wait"]["reply"]
    );
    let resumed=ok(server.call(WRITER,"awr_session_resume",json!({"project":"alpha","session":started["session"]["id"],"conversation":"onboarding-next-day","agent":"guide-editor","provider":"synthetic","model":"fixture","expected_revision":a.revision()})).await);
    assert_eq!(
        resumed["predecessor_waits"][0]["reply"],
        answered["wait"]["reply"]
    );
    let context = ok(server
        .call(
            WRITER,
            "awr_context_compile",
            json!({"project":"alpha","conversation":"onboarding-next-day"}),
        )
        .await);
    assert_eq!(
        context["continuity"]["waits"][0]["reply"],
        answered["wait"]["reply"]
    );
    assert_eq!(
        fs::read_to_string(a.root.join("work.yaml"))
            .unwrap()
            .matches("status: ready")
            .count(),
        2
    );
    server.stop().await;
}

#[tokio::test]
async fn missing_response_receipt_is_recovered_from_correlated_events_without_replaying() {
    let a = ProjectFixture::new("Prepare the guide");
    let b = ProjectFixture::new("Prepare the reference");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let conn = rusqlite::Connection::open(a.root.join(".awr/state.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fixture_deny_response BEFORE INSERT ON events WHEN new.event_type='mcp.operation_finished' BEGIN SELECT RAISE(ABORT,'synthetic response receipt failure'); END;").unwrap();
    let mut args = start_args("alpha", "W", "lost-response", a.revision(), true);
    args["request_id"] = json!("lost-start-result");
    let result = server.call(WRITER, "awr_session_start", args.clone()).await;
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["write_outcome"], "unknown");
    let receipt = ok(server
        .call(
            WRITER,
            "awr_operation_get",
            json!({"project":"alpha","request_id":"lost-start-result"}),
        )
        .await);
    assert_eq!(receipt["outcome"], "unknown");
    assert_eq!(receipt["domain_receipts"].as_array().unwrap().len(), 1);
    assert_eq!(
        receipt["domain_receipts"][0]["event_type"],
        "session.started"
    );
    let before = a.revision();
    server.call(WRITER, "awr_session_start", args.clone()).await;
    assert_eq!(a.revision(), before);
    let mut conflict = args.clone();
    conflict["model"] = json!("different-fixture");
    error(
        server.call(WRITER, "awr_session_start", conflict).await,
        "SourceConflict",
    );
    error(
        server
            .call(
                COLLEAGUE,
                "awr_operation_get",
                json!({"project":"alpha","request_id":"lost-start-result"}),
            )
            .await,
        "NotFound",
    );
    conn.execute_batch("DROP TRIGGER fixture_deny_response;")
        .unwrap();
    drop(conn);
    server.stop().await;
    let mut server = Server::start(&config).await;
    let recover = json!({"project":"alpha","request_id":"lost-start-result","expected_revision":a.revision()});
    let recovered = ok(server
        .call(WRITER, "awr_operation_recover", recover.clone())
        .await);
    assert_eq!(recovered["write_outcome"], "committed");
    assert_eq!(recovered["recovered"], true);
    let before = a.revision();
    ok(server.call(WRITER, "awr_operation_recover", recover).await);
    assert_eq!(a.revision(), before);
    ok(server.call(WRITER, "awr_session_start", args).await);
    assert_eq!(a.revision(), before);
    let sessions = ok(server
        .call(WRITER, "awr_session_list", json!({"project":"alpha"}))
        .await);
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
    // Generic event callers cannot forge the runtime correlation metadata.
    error(server.call(WRITER,"awr_event_append",json!({"project":"alpha","expected_revision":a.revision(),"event_type":"work.observed","summary":"Observation with a forged correlation","payload":{"mcp_operation_id":receipt["operation"]["id"]}})).await,"InvalidInput");
    // A start marker alone remains unknown; it never authorizes replay or false success.
    let mut store = Store::open_existing(&a.root.join(".awr/state.db")).unwrap();
    store
        .begin_mcp_operation(
            a.id,
            a.revision(),
            "writer",
            "no-terminal-receipt",
            "awr_session_start",
            &"a".repeat(64),
        )
        .unwrap();
    drop(store);
    let before = a.revision();
    let unresolved=server.call(WRITER,"awr_operation_recover",json!({"project":"alpha","request_id":"no-terminal-receipt","expected_revision":before})).await;
    assert_eq!(unresolved["isError"], true);
    assert_eq!(unresolved["structuredContent"]["write_outcome"], "unknown");
    assert_eq!(a.revision(), before);
    server.stop().await;
}

#[tokio::test]
async fn authorized_source_refresh_is_available_at_the_shared_endpoint() {
    let a = ProjectFixture::new("Draft the first guide");
    let b = ProjectFixture::new("Draft the second guide");
    let mut server = Server::start(&registry(&a, &b)).await;
    let path = a.root.join("work.yaml");
    let source = fs::read_to_string(&path)
        .unwrap()
        .replace("Draft the first guide", "Review the updated guide");
    fs::write(&path, &source).unwrap();
    error(
        server
            .call(
                READER,
                "awr_source_reindex",
                json!({"project":"alpha","expected_revision":a.revision()}),
            )
            .await,
        "RuleViolation",
    );
    let refreshed = ok(server
        .call(
            WRITER,
            "awr_source_reindex",
            json!({"project":"alpha","expected_revision":a.revision()}),
        )
        .await);
    assert_eq!(refreshed["source_write_performed"], false);
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    let work = ok(server
        .call(
            READER,
            "awr_work_get",
            json!({"project":"alpha","work":"W"}),
        )
        .await);
    assert_eq!(work["work"]["title"], "Review the updated guide");
    server.stop().await;
}

#[tokio::test]
async fn management_is_client_bound_and_recovers_an_unknown_response_without_repeating() {
    let a = ProjectFixture::new("Coordinate the guide");
    let b = ProjectFixture::new("Other project guide");
    let config = registry(&a, &b);
    let mut server = Server::start(&config).await;
    let started = ok(server
        .call(
            WRITER,
            "awr_session_start",
            start_args("alpha", "W", "management", a.revision(), true),
        )
        .await);
    let assessment = ok(server
        .call(
            READER,
            "awr_work_assess",
            json!({"project":"alpha","work":"W"}),
        )
        .await);
    let args = json!({"project":"alpha","work":"W","session":started["session"]["id"],"expected_revision":a.revision(),
        "request_id":"management-receipt","request_key":"planning-observation","contract_fingerprint":assessment["contract_fingerprint"],
        "observation":{"observed_at":now_millis().unwrap(),"note":"Two independently schedulable fixture deliverables","independently_schedulable_units":2}});
    error(
        server.call(READER, "awr_work_manage", args.clone()).await,
        "RuleViolation",
    );
    error(
        server
            .call(COLLEAGUE, "awr_work_manage", args.clone())
            .await,
        "RuleViolation",
    );
    let source = fs::read(a.root.join("work.yaml")).unwrap();
    let conn = rusqlite::Connection::open(a.root.join(".awr/state.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fixture_management_response BEFORE INSERT ON events WHEN new.event_type='mcp.operation_finished' BEGIN SELECT RAISE(ABORT,'synthetic management response failure'); END;").unwrap();
    let lost = server.call(WRITER, "awr_work_manage", args.clone()).await;
    assert_eq!(lost["structuredContent"]["write_outcome"], "unknown");
    let revision = a.revision();
    server.call(WRITER, "awr_work_manage", args.clone()).await;
    assert_eq!(a.revision(), revision);
    let receipt = ok(server
        .call(
            WRITER,
            "awr_operation_get",
            json!({"project":"alpha","request_id":"management-receipt"}),
        )
        .await);
    assert_eq!(
        receipt["domain_receipts"][0]["event_type"],
        "management.assessed"
    );
    conn.execute_batch("DROP TRIGGER fixture_management_response;")
        .unwrap();
    drop(conn);
    server.stop().await;
    let mut server = Server::start(&config).await;
    let recovered=ok(server.call(WRITER,"awr_operation_recover",json!({"project":"alpha","request_id":"management-receipt","expected_revision":a.revision()})).await);
    assert_eq!(recovered["write_outcome"], "committed");
    let assessed = ok(server
        .call(
            READER,
            "awr_work_assess",
            json!({"project":"alpha","work":"W"}),
        )
        .await);
    assert_eq!(assessed["decision"]["mode"], "continuous");
    assert_eq!(fs::read(a.root.join("work.yaml")).unwrap(), source);
    server.stop().await;
}

const API_SCOPE: &str = "01K00000000000000000000001";
const CLIENT_SCOPE: &str = "01K00000000000000000000002";

fn scoped_registry(a: &ProjectFixture, b: &ProjectFixture) -> PathBuf {
    let path = a.root.join("service.toml");
    fs::write(
        &path,
        format!(
            r#"version=2
[[projects]]
key='alpha'
root={}
project_id='{}'
[[projects]]
key='beta'
root={}
project_id='{}'
[[clients]]
id='writer'
token_env='AWR_FIXTURE_WRITER'
write=['alpha','beta']
[[clients.workstreams]]
project='alpha'
workstream_id='{API_SCOPE}'
authority_version=1
[[clients.workstreams]]
project='alpha'
workstream_id='{CLIENT_SCOPE}'
authority_version=1
[[clients]]
id='reader'
token_env='AWR_FIXTURE_READER'
read=['alpha']
[[clients.workstreams]]
project='alpha'
workstream_id='{API_SCOPE}'
authority_version=1
[[clients]]
id='colleague'
token_env='AWR_FIXTURE_COLLEAGUE'
write=['alpha']
[[clients.workstreams]]
project='alpha'
workstream_id='{CLIENT_SCOPE}'
authority_version=1
"#,
            json!(a.root),
            a.id,
            json!(b.root),
            b.id
        ),
    )
    .unwrap();
    path
}
fn scoped(action: &str, work: Option<&str>, args: Value) -> Value {
    let mut request = json!({"project":"alpha","protocol_version":1,"action":action,"args":args});
    if let Some(work) = work {
        request["work"] = json!(work);
    }
    request
}

#[tokio::test]
async fn workstream_http_grants_filter_context_objects_counts_and_search() {
    let a = ProjectFixture::workstreams();
    let b = ProjectFixture::new("Unchanged legacy project");
    let mut server = Server::start(&scoped_registry(&a, &b)).await;
    let before = a.revision();
    let caps = ok(server
        .call(
            READER,
            "awr_workstream",
            scoped("capabilities", None, json!({})),
        )
        .await);
    assert_eq!(caps["writes"], json!([]));
    assert_eq!(caps["team_postgres"], false);
    let list = ok(server
        .call(READER, "awr_workstream", scoped("list", None, json!({})))
        .await);
    assert_eq!(list["workstreams"].as_array().unwrap().len(), 1);
    assert_eq!(list["workstreams"][0]["id"], API_SCOPE);
    let context = ok(server
        .call(
            READER,
            "awr_workstream",
            scoped("context", Some("API-1"), json!({"budget":8000})),
        )
        .await);
    assert_eq!(context["result"]["completeness"]["complete"], true);
    assert!(!context.to_string().contains("PRIVATE_CLIENT"));
    assert!(!context.to_string().contains("CLIENT-1"));
    let catalog = ok(server
        .call(
            READER,
            "awr_workstream",
            scoped("catalog", None, json!({"kind":"work"})),
        )
        .await);
    assert_eq!(catalog["result"]["page"]["total"], 2);
    assert!(!catalog.to_string().contains("PRIVATE_CLIENT"));
    let search = ok(server
        .call(
            READER,
            "awr_workstream",
            scoped("search", None, json!({"text":"PRIVATE_CLIENT"})),
        )
        .await);
    assert_eq!(search["result"]["hits"], json!([]));
    for hidden in ["CLIENT-1", "nonexistent"] {
        error(
            server
                .call(
                    READER,
                    "awr_workstream",
                    scoped("context", Some(hidden), json!({})),
                )
                .await,
            "WorkstreamAccessDenied",
        );
        error(
            server
                .call(
                    READER,
                    "awr_workstream",
                    scoped(
                        "object",
                        Some("API-1"),
                        json!({"kind":"work","reference":hidden}),
                    ),
                )
                .await,
            "WorkstreamAccessDenied",
        );
    }
    error(
        server
            .call(
                WRITER,
                "awr_workstream",
                scoped("catalog", None, json!({"kind":"work"})),
            )
            .await,
        "WorkstreamScopeRequired",
    );
    let mut conflict = scoped("context", Some("API-1"), json!({}));
    conflict["workstream"] = json!(CLIENT_SCOPE);
    error(
        server.call(WRITER, "awr_workstream", conflict).await,
        "WorkstreamBindingMismatch",
    );
    // The same actor's unrelated legacy project retains its established tools.
    ok(server
        .call(WRITER, "awr_work_get", json!({"project":"beta","work":"W"}))
        .await);
    assert_eq!(a.revision(), before);
    server.stop().await;
}

#[tokio::test]
async fn workstream_http_denies_legacy_aliases_writes_and_request_grants() {
    let a = ProjectFixture::workstreams();
    let b = ProjectFixture::new("Other project");
    let mut server = Server::start_mode(&scoped_registry(&a, &b), "hierarchical").await;
    let before = a.revision();
    for name in [
        "awr_project_status",
        "awr_work_get",
        "awr_search",
        "awr_context_compile",
        "awr_session_list",
        "awr_operation_get",
        "awr_work_transition",
        "awr_session_start",
        "awr_source_reindex",
        "awr_event_append",
        "awr_change_apply",
    ] {
        error(
            server
                .call(WRITER, name, json!({"project":"alpha","work":"API-1"}))
                .await,
            "Unsupported",
        );
    }
    error(
        server
            .call(
                WRITER,
                "awr_query",
                json!({"project":"alpha","child_tool":"awr_work_get","arguments":{"work":"API-1"}}),
            )
            .await,
        "Unsupported",
    );
    let mut forged = scoped("context", Some("CLIENT-1"), json!({}));
    forged["grants"] = json!([{ "workstream_id":CLIENT_SCOPE,"read":true,"authority_version":1 }]);
    error(
        server.call(READER, "awr_workstream", forged).await,
        "InvalidInput",
    );
    let mut unknown = scoped("context", Some("API-1"), json!({}));
    unknown["protocol_version"] = json!(2);
    error(
        server.call(READER, "awr_workstream", unknown).await,
        "Unsupported",
    );
    error(
        server
            .call(
                WRITER,
                "awr_workstream",
                scoped("claim", Some("API-1"), json!({})),
            )
            .await,
        "Unsupported",
    );
    ok(server
        .call(READER, "awr_workstream", scoped("list", None, json!({})))
        .await);
    assert_eq!(a.revision(), before);
    server.stop().await;
    // A legacy project-level grant is never upgraded to access all streams.
    let mut server = Server::start(&registry(&a, &b)).await;
    error(
        server
            .call(
                WRITER,
                "awr_work_get",
                json!({"project":"alpha","work":"API-1"}),
            )
            .await,
        "Unsupported",
    );
    error(
        server
            .call(WRITER, "awr_workstream", scoped("list", None, json!({})))
            .await,
        "WorkstreamAccessDenied",
    );
    server.stop().await;
}

#[tokio::test]
async fn workstream_http_cursors_are_bound_to_client_and_scope() {
    let a = ProjectFixture::workstreams();
    let b = ProjectFixture::new("Other project");
    let mut server = Server::start(&scoped_registry(&a, &b)).await;
    let page = ok(server
        .call(
            READER,
            "awr_workstream",
            scoped("catalog", None, json!({"kind":"work","limit":1})),
        )
        .await);
    let cursor = page["result"]["next_cursor"].clone();
    assert!(cursor.is_object());
    let next = ok(server
        .call(
            READER,
            "awr_workstream",
            scoped(
                "catalog",
                None,
                json!({"kind":"work","limit":1,"cursor":cursor}),
            ),
        )
        .await);
    assert_ne!(
        next["result"]["page"]["items"],
        page["result"]["page"]["items"]
    );
    error(
        server
            .call(
                COLLEAGUE,
                "awr_workstream",
                scoped(
                    "catalog",
                    None,
                    json!({"kind":"work","limit":1,"cursor":cursor}),
                ),
            )
            .await,
        "WorkstreamAccessDenied",
    );
    error(
        server
            .call(
                WRITER,
                "awr_workstream",
                scoped(
                    "catalog",
                    Some("API-1"),
                    json!({"kind":"work","limit":1,"cursor":cursor}),
                ),
            )
            .await,
        "WorkstreamAccessDenied",
    );
    server.stop().await;
}

#[tokio::test]
async fn workstream_http_stale_authority_never_falls_back_to_project_access() {
    let a = ProjectFixture::workstreams();
    let b = ProjectFixture::new("Other project");
    let mut server = Server::start(&scoped_registry(&a, &b)).await;
    let path = a.root.join("work.yaml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replacen("authority_version: 1", "authority_version: 2", 1),
    )
    .unwrap();
    error(
        server
            .call(
                READER,
                "awr_workstream",
                scoped("context", Some("API-1"), json!({})),
            )
            .await,
        "SourceStale",
    );
    a.reindex();
    error(
        server
            .call(
                READER,
                "awr_workstream",
                scoped("context", Some("API-1"), json!({})),
            )
            .await,
        "WorkstreamStaleAuthority",
    );
    error(
        server
            .call(
                WRITER,
                "awr_work_get",
                json!({"project":"alpha","work":"API-1"}),
            )
            .await,
        "Unsupported",
    );
    // A separately authorized stream is unaffected by another grant's staleness.
    ok(server
        .call(
            COLLEAGUE,
            "awr_workstream",
            scoped("catalog", None, json!({"kind":"work"})),
        )
        .await);
    server.stop().await;
}

#[tokio::test]
async fn workstream_http_events_recovery_and_session_selectors_stay_scoped() {
    let a = ProjectFixture::workstreams();
    let b = ProjectFixture::new("Other project");
    let mut store = Store::open_existing(&a.root.join(".awr/state.db")).unwrap();
    let mut sessions = Vec::new();
    for work in ["API-1", "CLIENT-1"] {
        let rev = store.project(a.id).unwrap().project_revision;
        let started = store
            .start_session(
                a.id,
                rev,
                SessionDraft {
                    work_item_key: Some(work.into()),
                    agent_id: format!("agent-{work}"),
                    provider: "fixture".into(),
                    model: "fixture".into(),
                    branch_id: None,
                    claim: false,
                    claim_ttl_ms: None,
                },
            )
            .unwrap()
            .0;
        sessions.push(started.session.id);
    }
    drop(store);
    let mut server = Server::start(&scoped_registry(&a, &b)).await;
    let events = ok(server
        .call(READER, "awr_workstream", scoped("events", None, json!({})))
        .await);
    assert_eq!(
        events["result"]["page"]["events"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        events["result"]["page"]["events"][0]["session_id"],
        sessions[0].to_string()
    );
    let recovery = ok(server
        .call(
            READER,
            "awr_workstream",
            scoped("recovery", None, json!({})),
        )
        .await);
    assert_eq!(
        recovery["result"]["candidates"].as_array().unwrap().len(),
        1
    );
    let mut own = scoped("recovery", None, json!({}));
    own["session"] = json!(sessions[0]);
    ok(server.call(READER, "awr_workstream", own).await);
    let mut foreign = scoped("recovery", None, json!({}));
    foreign["session"] = json!(sessions[1]);
    error(
        server.call(READER, "awr_workstream", foreign).await,
        "WorkstreamAccessDenied",
    );
    error(
        server
            .call(
                READER,
                "awr_workstream",
                scoped(
                    "object",
                    None,
                    json!({"kind":"session","reference":sessions[1]}),
                ),
            )
            .await,
        "WorkstreamAccessDenied",
    );
    server.stop().await;
}

#[tokio::test]
async fn workstream_http_artifacts_and_checkpoints_do_not_leak_through_objects() {
    let a = ProjectFixture::workstreams();
    let b = ProjectFixture::new("Other project");
    let mut store = Store::open_existing(&a.root.join(".awr/state.db")).unwrap();
    let mut objects = Vec::new();
    for work in ["API-1", "CLIENT-1"] {
        let rev = store.project(a.id).unwrap().project_revision;
        let (started, event) = store
            .start_session(
                a.id,
                rev,
                SessionDraft {
                    work_item_key: Some(work.into()),
                    agent_id: format!("agent-{work}"),
                    provider: "fixture".into(),
                    model: "fixture".into(),
                    branch_id: None,
                    claim: false,
                    claim_ttl_ms: None,
                },
            )
            .unwrap();
        let rev = store.project(a.id).unwrap().project_revision;
        let (cp, _) = store
            .create_checkpoint(
                a.id,
                rev,
                started.session.id,
                CheckpointDraft {
                    context_hash: "a".repeat(64),
                    digest: format!("{work} checkpoint"),
                    next_action: "Review the draft".into(),
                    open_loops: vec![],
                    changed_entities: vec![],
                },
            )
            .unwrap();
        let rev = store.project(a.id).unwrap().project_revision;
        let (artifact, _) = store
            .record_artifact(
                a.id,
                rev,
                ArtifactDraft {
                    artifact_type: "document".into(),
                    locator: format!("https://example.test/{work}"),
                    sha256: "a".repeat(64),
                    size: 17,
                    mime: "text/plain".into(),
                    source_event_id: event.id,
                },
            )
            .unwrap();
        objects.push((cp.id, artifact.id, event.id));
    }
    drop(store);
    let mut server = Server::start(&scoped_registry(&a, &b)).await;
    for (kind, own, other) in [
        ("checkpoint", objects[0].0, objects[1].0),
        ("artifact", objects[0].1, objects[1].1),
        ("event", objects[0].2, objects[1].2),
    ] {
        ok(server
            .call(
                READER,
                "awr_workstream",
                scoped("object", None, json!({"kind":kind,"reference":own})),
            )
            .await);
        let denied = server
            .call(
                READER,
                "awr_workstream",
                scoped("object", None, json!({"kind":kind,"reference":other})),
            )
            .await;
        assert!(!denied.to_string().contains("CLIENT-1"));
        error(denied, "WorkstreamAccessDenied");
    }
    let catalog = ok(server
        .call(
            READER,
            "awr_workstream",
            scoped("catalog", None, json!({"kind":"artifact"})),
        )
        .await);
    assert_eq!(catalog["result"]["page"]["total"], 1);
    assert!(!catalog.to_string().contains("CLIENT-1"));
    // Whole-source and artifact-byte readers have no scoped transport implementation.
    error(
        server
            .call(
                READER,
                "awr_workstream",
                scoped("catalog", None, json!({"kind":"source"})),
            )
            .await,
        "Unsupported",
    );
    error(
        server
            .call(
                READER,
                "awr_workstream",
                scoped("artifact_read", None, json!({})),
            )
            .await,
        "Unsupported",
    );
    server.stop().await;
}

#[tokio::test]
async fn workstream_registry_requires_explicit_valid_versioned_operator_grants() {
    let a = ProjectFixture::workstreams();
    let b = ProjectFixture::new("Other project");
    let path = scoped_registry(&a, &b);
    let original = fs::read_to_string(&path).unwrap();
    for bad in [
        original.replacen("version=2", "version=1", 1),
        original.replacen("authority_version=1", "authority_version=0", 1),
        original.replacen("authority_version=1", "authority_version=1\nwrite=true", 1),
        original.replace(
            "workstream_id='01K00000000000000000000002'",
            "workstream_id='01K00000000000000000000001'",
        ),
    ] {
        fs::write(&path, bad).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_awr-mcp"));
        child
            .args([
                "--registry",
                path.to_str().unwrap(),
                "--listen",
                "127.0.0.1:0",
            ])
            .env("AWR_FIXTURE_WRITER", WRITER)
            .env("AWR_FIXTURE_READER", READER)
            .env("AWR_FIXTURE_COLLEAGUE", COLLEAGUE)
            .kill_on_drop(true);
        let output = timeout(Duration::from_secs(10), child.output())
            .await
            .expect("invalid policy must exit")
            .unwrap();
        assert!(!output.status.success());
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(!err.contains(WRITER));
        assert!(!err.contains("listening at"));
    }
}

#[tokio::test]
async fn workstream_pending_enablement_cannot_use_old_shared_mutations() {
    let a = ProjectFixture::new("Legacy source");
    let b = ProjectFixture::new("Other project");
    let mut server = Server::start(&registry(&a, &b)).await;
    fs::write(a.root.join(".awr/project.toml"),"[project]\nname='Pending enablement'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-workstream-ledger-v1'\n").unwrap();
    fs::write(
        a.root.join("work.yaml"),
        include_str!("../../../tests/fixtures/workstreams/context.yaml"),
    )
    .unwrap();
    let before = a.revision();
    error(
        server
            .call(
                WRITER,
                "awr_work_get",
                json!({"project":"alpha","work":"W"}),
            )
            .await,
        "Unsupported",
    );
    error(
        server
            .call(
                WRITER,
                "awr_source_reindex",
                json!({"project":"alpha","expected_revision":before}),
            )
            .await,
        "Unsupported",
    );
    assert_eq!(a.revision(), before);
    server.stop().await;
}

#[tokio::test]
async fn work_graph_nodes_carry_the_newest_event_time_of_their_work() {
    let a = ProjectFixture::new("Continue the guide");
    let b = ProjectFixture::new("Separate guide");
    let mut server = Server::start(&registry(&a, &b)).await;
    let graph = ok(server
        .call(READER, "awr_work_graph", json!({"project":"alpha"}))
        .await);
    // Present and null while nothing happened to any work.
    for node in graph["nodes"].as_array().unwrap() {
        assert!(node["last_event_at"].is_null(), "{node}");
    }
    let event = ok(server
        .call(
            WRITER,
            "awr_event_append",
            json!({"project":"alpha","expected_revision":a.revision(),"event_type":"work.observed","work":"W","summary":"Reviewed the guide outline"}),
        )
        .await);
    let created_at = event["event"]["created_at"].as_i64().unwrap();
    let graph = ok(server
        .call(READER, "awr_work_graph", json!({"project":"alpha"}))
        .await);
    let at = |key: &str| {
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["key"] == key)
            .unwrap()["last_event_at"]
            .clone()
    };
    assert_eq!(at("W"), json!(created_at));
    assert!(at("NEXT").is_null());
    server.stop().await;
}

#[tokio::test]
async fn workstream_nav_goal_selector_follows_goals_declared_in_the_source() {
    let a = ProjectFixture::workstreams();
    let ledger = a.root.join("work.yaml");
    let mut text = fs::read_to_string(&ledger).unwrap();
    text.push_str("  - id: API-3\n    title: Declares the API goal\n    status: planned\n    workstream: api\n    goal: [api-delivery]\n    acceptance: [Deliver the API goal]\n");
    fs::write(&ledger, text).unwrap();
    a.reindex();
    let b = ProjectFixture::new("Unchanged legacy project");
    let mut server = Server::start(&scoped_registry(&a, &b)).await;
    let before = a.revision();
    let nav_with = |args: Value| {
        let mut request = scoped("nav", None, args);
        request["workstream"] = json!(API_SCOPE);
        request
    };
    let nav = ok(server
        .call(
            READER,
            "awr_workstream",
            nav_with(json!({"goal":"api-delivery"})),
        )
        .await);
    assert_eq!(nav["scope"]["selection"], json!(["API-3"]));
    assert_eq!(nav["scope"]["requested"]["goal"], "api-delivery");
    // An unknown goal fails instead of becoming an empty selection. This endpoint reports every nav failure
    // through its opaque boundary error so that a scoped reader learns nothing about other scopes.
    error(
        server
            .call(
                READER,
                "awr_workstream",
                nav_with(json!({"goal":"absent-goal"})),
            )
            .await,
        "SourceStale",
    );
    assert_eq!(a.revision(), before);
    server.stop().await;
}

#[tokio::test]
async fn workstream_nav_nodes_carry_the_newest_event_time_for_an_authorized_reader() {
    let a = ProjectFixture::workstreams();
    let b = ProjectFixture::new("Unchanged legacy project");
    let created_at = {
        let mut store = Store::open_existing(&a.root.join(".awr/state.db")).unwrap();
        let work = store.work_item(a.id, "API-1").unwrap().item.meta.id;
        let mut draft = EventDraft::new("work.observed", "Reviewed the interface");
        draft.work_item_id = Some(work);
        store
            .append_event(a.id, a.revision(), draft)
            .unwrap()
            .created_at
    };
    let mut server = Server::start(&scoped_registry(&a, &b)).await;
    let before = a.revision();
    let mut request = scoped("nav", None, json!({}));
    request["workstream"] = json!(API_SCOPE);
    let nav = ok(server.call(READER, "awr_workstream", request).await);
    let nodes = nav["mainline_graph"]["nodes"].as_array().unwrap();
    let at =
        |key: &str| nodes.iter().find(|n| n["work_key"] == key).unwrap()["last_event_at"].clone();
    assert_eq!(at("API-1"), json!(created_at));
    // The other work of the stream has no event: the field is present and null.
    assert!(at("API-2").is_null());
    assert_eq!(a.revision(), before);
    server.stop().await;
}

#[tokio::test]
async fn workstream_nav_explains_a_cancelled_dependency_for_an_authorized_reader() {
    let a = ProjectFixture::workstreams_with(
        "  - id: API-3\n    title: Retired interface draft\n    status: cancelled\n    workstream: api\n  - id: API-4\n    title: Build on the interface draft\n    status: planned\n    workstream: api\n    depends_on: [API-3]\n    acceptance: [Build on the draft]\n",
    );
    let b = ProjectFixture::new("Unchanged legacy project");
    let mut server = Server::start(&scoped_registry(&a, &b)).await;
    let before = a.revision();
    let mut request = scoped("nav", None, json!({}));
    request["workstream"] = json!(API_SCOPE);
    let nav = ok(server.call(READER, "awr_workstream", request).await);
    assert_eq!(nav["protocol"], "awr-mainline-nav");
    assert_eq!(nav["schema_version"], 1);
    assert_eq!(nav["read_only"], true);
    let nodes = nav["mainline_graph"]["nodes"].as_array().unwrap();
    let waiting = nodes.iter().find(|n| n["work_key"] == "API-4").unwrap();
    // The same wait the CLI reports: a cancelled prerequisite is explained, not silently skipped.
    assert_eq!(waiting["explainable_waits"].as_array().unwrap().len(), 1);
    assert_eq!(
        waiting["explainable_waits"][0]["kind"],
        "dependency_cancelled"
    );
    assert_eq!(
        waiting["explainable_waits"][0]["summary"],
        "Required dependency API-3 was cancelled"
    );
    assert_eq!(
        waiting["explainable_waits"][0]["release_condition"],
        "Remove or replace API-3 in the dependencies of API-4, or reopen API-3 and complete it"
    );
    assert!(!nav.to_string().contains("PRIVATE_CLIENT"));
    assert_eq!(a.revision(), before);
    server.stop().await;
}

#[tokio::test]
async fn team_tools_follow_the_runtime_switch() {
    let a = ProjectFixture::new("Write the team guide");
    let b = ProjectFixture::new("Write the other guide");
    let config = registry(&a, &b);
    let team = ["awr_team_handoff", "awr_team_review"];
    for mode in ["hierarchical", "flat"] {
        // Off by default: not advertised, and calls get a clear disabled error.
        let mut server = Server::start_with_team(&config, mode, None).await;
        let names = server.tool_names(WRITER).await;
        assert!(
            team.iter().all(|t| !names.iter().any(|n| n == t)),
            "{names:?}"
        );
        for tool in team {
            let body = server
                .rpc(
                    WRITER,
                    "tools/call",
                    Some(tool),
                    json!({"name":tool,"arguments":{}}),
                )
                .await;
            let message = body["error"]["message"]
                .as_str()
                .unwrap_or_else(|| panic!("{body}"));
            assert!(message.contains("team features are disabled"), "{message}");
        }
        server.stop().await;
    }
    // The registry key turns Team tools on.
    let original = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        original.replacen("version=1\n", "version=1\nteam_enabled=true\n", 1),
    )
    .unwrap();
    let mut server = Server::start_with_team(&config, "hierarchical", None).await;
    let names = server.tool_names(WRITER).await;
    assert!(
        team.iter().all(|t| names.iter().any(|n| n == t)),
        "{names:?}"
    );
    server.stop().await;
    // The environment override takes precedence over the registry key.
    let mut server = Server::start_with_team(&config, "hierarchical", Some("0")).await;
    let names = server.tool_names(WRITER).await;
    assert!(
        team.iter().all(|t| !names.iter().any(|n| n == t)),
        "{names:?}"
    );
    server.stop().await;
}
