//! Synthetic credentials only. Every fixture is an isolated temporary project.
use awr_core::*;
use awr_runtime::{ArtifactFile, Runtime};
use awr_source::{Manifest, index_project};
use awr_store::{SearchQuery, Store};
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const SENTINEL: &str = "fixture-value-only-never-real";
const WORK: &str = "work_items:\n- id: W\n  title: Prepare customer analysis\n  status: in_progress\n  next_action: Draft the analysis\n  acceptance: [Deliver a reviewed report]\n";
const MANIFEST: &str = "[project]\nname='Secret boundary fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n";
struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-secret-boundary-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join(".awr/project.toml"), MANIFEST).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(root.join("rules.md"), "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve exact acceptance and source facts.\n").unwrap();
        fs::write(
            root.join("goal.md"),
            "# Deliver useful analysis\n\nDeliver reviewed customer results.\n",
        )
        .unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let report =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(report.ok);
        Self {
            root,
            store,
            project: report.project_id,
        }
    }
    fn revision(&self) -> Revision {
        self.store.project(self.project).unwrap().project_revision
    }
    fn reject<T>(&self, result: Result<T>, revision: Revision) {
        let error = match result {
            Err(e) => e,
            Ok(_) => panic!("sensitive write succeeded"),
        };
        assert_eq!(error.code(), "RuleViolation");
        assert!(
            !serde_json::to_string(&error.report())
                .unwrap()
                .contains(SENTINEL)
        );
        assert_eq!(self.revision(), revision);
        assert!(
            self.store
                .events_since(self.project, revision, 100)
                .unwrap()
                .is_empty()
        );
        self.no_secret_in_database();
    }
    fn no_secret_in_database(&self) {
        let conn = self.sql();
        let tables = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for table in tables {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))
                .unwrap();
            let columns = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for i in 0..columns {
                    if let rusqlite::types::ValueRef::Text(bytes) = row.get_ref(i).unwrap() {
                        assert!(
                            !String::from_utf8_lossy(bytes).contains(SENTINEL),
                            "secret persisted in a database text column"
                        );
                    }
                }
            }
        }
    }
    fn sql(&self) -> Connection {
        Connection::open(self.root.join(".awr/state.db")).unwrap()
    }
    fn session(&mut self) -> Session {
        self.store
            .start_session(
                self.project,
                self.revision(),
                SessionDraft {
                    work_item_key: Some("W".into()),
                    agent_id: "fixture-agent".into(),
                    provider: "fixture".into(),
                    model: "fixture".into(),
                    branch_id: None,
                    claim: false,
                    claim_ttl_ms: None,
                },
            )
            .unwrap()
            .0
            .session
    }
    fn event(&mut self, summary: &str, payload: Value) -> Result<Event> {
        let mut event = EventDraft::new("work.observed", summary);
        event.payload = payload;
        self.store
            .append_event(self.project, self.revision(), event)
    }
    fn evidence(&self, summary: &str) -> EvidenceDraft {
        EvidenceDraft {
            external_key: "E".into(),
            work_item_key: Some("W".into()),
            evidence_type: "report".into(),
            level: EvidenceLevel::Implemented,
            summary: summary.into(),
            locator: "report.txt".into(),
            sha256: None,
            source_sha: None,
            command: None,
            scope: vec!["W".into()],
            branch_id: None,
            verified_at: None,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn source_adapters_reject_secrets_and_keep_old_projection_with_safe_diagnostics() {
    for (path, text) in [
        (
            "work.yaml",
            format!("{WORK}  summary: 'password: {SENTINEL}'\n"),
        ),
        ("rules.md", format!("# Authority\nAPI_KEY={SENTINEL}\n")),
        ("goal.md", format!("# Analysis\n私有提示词：{SENTINEL}\n")),
    ] {
        let mut f = Fixture::new();
        fs::write(f.root.join(path), &text).unwrap();
        let report = index_project(
            &mut f.store,
            &f.root,
            &Manifest::load(&f.root).unwrap(),
            false,
        )
        .unwrap();
        assert!(!report.ok);
        assert!(report.issues.iter().any(|i| i.code == "RuleViolation"));
        assert!(!serde_json::to_string(&report).unwrap().contains(SENTINEL));
        assert_eq!(
            f.store.work_item(f.project, "W").unwrap().item.title,
            "Prepare customer analysis"
        );
        assert_eq!(fs::read_to_string(f.root.join(path)).unwrap(), text);
        f.no_secret_in_database();
    }
    // Parser diagnostics must not echo even unrecognizable private content in malformed YAML.
    let mut f = Fixture::new();
    fs::write(
        f.root.join("work.yaml"),
        format!("work_items: [\n{SENTINEL}: ["),
    )
    .unwrap();
    let report = index_project(
        &mut f.store,
        &f.root,
        &Manifest::load(&f.root).unwrap(),
        false,
    )
    .unwrap();
    assert!(!report.ok);
    assert!(!serde_json::to_string(&report).unwrap().contains(SENTINEL));
    println!("AWR_PAYLOAD_CASE source_secret_rejection");
    println!("AWR_PAYLOAD_CASE source_diagnostic_no_secret");
}

#[test]
fn manifest_guards_text_typed_options_and_unknown_fields_without_echo() {
    for text in [
        MANIFEST.replace(
            "name='Secret boundary fixture'",
            &format!("name='API_KEY={SENTINEL}'"),
        ),
        format!("{MANIFEST}password='{SENTINEL}'\n"),
        format!("{MANIFEST}\"pa\\u0073sword\"='{SENTINEL}'\n"),
        format!("{MANIFEST}{SENTINEL} = [\n"),
    ] {
        let error = Manifest::parse(&text).unwrap_err();
        assert!(!error.to_string().contains(SENTINEL));
    }
    let mut manifest = Manifest::parse(MANIFEST).unwrap();
    manifest.sources[0]
        .options
        .insert("password".into(), SENTINEL.into());
    assert!(manifest.validate().is_err());
    println!("AWR_PAYLOAD_CASE manifest_secret_rejection");
}

#[test]
fn events_reject_all_observation_fields_and_domain_writes_atomically() {
    let mut f = Fixture::new();
    for field in [
        "body", "stdout", "stderr", "detail", "command", "report", "status",
    ] {
        let revision = f.revision();
        let result = f.event(
            "Reviewed the report",
            json!({field:format!("password: {SENTINEL}")}),
        );
        f.reject(result, revision);
    }
    let revision = f.revision();
    let result = f.event(&format!("API_KEY={SENTINEL}"), json!({}));
    f.reject(result, revision);
    let result = f.store.start_session(
        f.project,
        revision,
        SessionDraft {
            work_item_key: Some("W".into()),
            agent_id: format!("token={SENTINEL}"),
            provider: "fixture".into(),
            model: "fixture".into(),
            branch_id: None,
            claim: true,
            claim_ttl_ms: Some(60_000),
        },
    );
    f.reject(result, revision);
    assert_eq!(
        f.sql()
            .query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        f.sql()
            .query_row("SELECT count(*) FROM claims", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    println!("AWR_PAYLOAD_CASE event_secret_rejection");
}

#[test]
fn checkpoints_and_evidence_check_fields_not_in_their_events() {
    let mut f = Fixture::new();
    let session = f.session();
    for field in ["digest", "next_action", "open_loops", "changed_entities"] {
        let mut draft = CheckpointDraft {
            context_hash: "a".repeat(64),
            digest: "Reviewed results".into(),
            next_action: "Continue review".into(),
            open_loops: vec![],
            changed_entities: vec![],
        };
        let secret = format!("password: {SENTINEL}");
        match field {
            "digest" => draft.digest = secret,
            "next_action" => draft.next_action = secret,
            "open_loops" => draft.open_loops = vec![secret],
            _ => draft.changed_entities = vec![secret],
        }
        let revision = f.revision();
        let result = f
            .store
            .create_checkpoint(f.project, revision, session.id, draft);
        f.reject(result, revision);
    }
    assert!(
        f.store
            .latest_checkpoint(f.project, session.id)
            .unwrap()
            .is_none()
    );
    for field in [
        "summary",
        "locator",
        "command",
        "scope",
        "external_key",
        "evidence_type",
    ] {
        let mut draft = f.evidence("Reviewed results");
        let secret = format!("password: {SENTINEL}");
        match field {
            "summary" => draft.summary = secret,
            "locator" => draft.locator = secret,
            "command" => draft.command = Some(secret),
            "scope" => draft.scope = vec![secret],
            "external_key" => draft.external_key = secret,
            _ => draft.evidence_type = secret,
        }
        let revision = f.revision();
        let result = f.store.record_evidence(f.project, revision, draft);
        f.reject(result, revision);
    }
    println!("AWR_PAYLOAD_CASE checkpoint_secret_rejection");
    println!("AWR_PAYLOAD_CASE evidence_secret_rejection");
}

#[test]
fn direct_projection_and_configuration_cannot_bypass_source_guard() {
    let mut f = Fixture::new();
    let mut work = f.store.work_item(f.project, "W").unwrap().item;
    let source = f
        .store
        .source(f.project, work.meta.source_ref.source_id)
        .unwrap();
    work.next_action = format!("private_prompt: {SENTINEL}");
    let revision = f.revision();
    // Same-fingerprint early-return must not acknowledge an unchecked batch.
    let result = f.store.commit_source_projection(
        &source,
        &source.fingerprint,
        ProjectionBatch {
            work_items: vec![work],
            ..Default::default()
        },
    );
    f.reject(result, revision);
    let result = f
        .store
        .configure_source(&source, json!({"env":{"HOME":SENTINEL}}));
    f.reject(result, revision);
    assert_eq!(
        f.store.source(f.project, source.id).unwrap().freshness,
        Freshness::Fresh
    );
    println!("AWR_PAYLOAD_CASE source_projection_secret_guard");
}

#[test]
fn managed_artifacts_scan_complete_bytes_before_any_file_or_record_and_guard_legacy_reads() {
    let mut f = Fixture::new();
    let event = f.event("Produced report", json!({})).unwrap();
    for bytes in [
        format!(
            "{}\npassword:{}{SENTINEL}",
            "a".repeat(32760),
            " ".repeat(70_000)
        )
        .into_bytes(),
        [vec![0xff, 0, 1], format!("API_KEY={SENTINEL}").into_bytes()].concat(),
    ] {
        fs::write(f.root.join("report.txt"), &bytes).unwrap();
        let revision = f.revision();
        let result = Runtime::attach(&mut f.store, f.project)
            .unwrap()
            .import_artifact(
                revision,
                ArtifactFile {
                    path: f.root.join("report.txt"),
                    artifact_type: "report".into(),
                    mime: "application/octet-stream".into(),
                    source_event_id: event.id,
                    max_bytes: 64 * 1024 * 1024,
                },
            );
        f.reject(result, revision);
        assert!(!f.root.join(".awr/artifacts").exists());
        assert_eq!(fs::read(f.root.join("report.txt")).unwrap(), bytes);
    }
    // Metadata-only registration is not body verification. Existing external reports may
    // contain sensitive values; explicit reads still withhold the complete body.
    let bytes = format!("password: {SENTINEL}").into_bytes();
    fs::write(f.root.join("report.txt"), &bytes).unwrap();
    let artifact = f
        .store
        .record_artifact(
            f.project,
            f.revision(),
            ArtifactDraft {
                artifact_type: "report".into(),
                locator: "report.txt".into(),
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                size: bytes.len() as u64,
                mime: "text/plain".into(),
                source_event_id: event.id,
            },
        )
        .unwrap()
        .0;
    let revision = f.revision();
    let result = Runtime::attach(&mut f.store, f.project)
        .unwrap()
        .read_artifact(artifact.id, 16 * 1024 * 1024);
    f.reject(result, revision);
    let draft = f.evidence("Review report");
    f.store
        .record_evidence(f.project, f.revision(), draft)
        .unwrap();
    let revision = f.revision();
    let result = Runtime::attach(&mut f.store, f.project)
        .unwrap()
        .read_evidence_report("E", 16 * 1024 * 1024);
    f.reject(result, revision);
    println!("AWR_PAYLOAD_CASE artifact_secret_rejection");
}

#[test]
fn labelled_unicode_environment_private_prompt_and_recognizable_tokens_reach_real_writes() {
    for (case, payloads) in [
        (
            "environment_rejection",
            vec![
                format!("export HOME={SENTINEL}"),
                format!("env: {{HOME: {SENTINEL}}}"),
                format!("export PASSWORD=false\" {SENTINEL}\""),
                format!("export PASSWORD=true' {SENTINEL}'"),
            ],
        ),
        (
            "private_prompt_rejection",
            vec![
                format!("private_prompt: {SENTINEL}"),
                format!("# 私有提示词\n{SENTINEL}"),
                format!("# Private prompt\nTrue customer identities must be included.\n{SENTINEL}"),
                format!("# Private prompt\nfalse\n{SENTINEL}"),
            ],
        ),
        (
            "unicode_secret_labels",
            vec![
                format!("ＡＰＩ＿ＫＥＹ：{SENTINEL}"),
                format!("pass\u{200b}word: {SENTINEL}"),
                format!(r#"{{"pa\u0073sword":"{SENTINEL}"}}"#),
            ],
        ),
        (
            "known_token_patterns",
            ["sk-", "ghp_", "github_pat_", "xoxb-"]
                .iter()
                .map(|p| format!("{p}{}", "a".repeat(40)))
                .chain(
                    [
                        "Basic dXNlcjpwYXNz",
                        "Basic dXNlcjo",
                        "Basic YTpi",
                        "Basic Og==",
                    ]
                    .into_iter()
                    .map(String::from),
                )
                .collect(),
        ),
    ] {
        let mut f = Fixture::new();
        for payload in payloads {
            let revision = f.revision();
            let result = f.event("Reviewed report", json!({"body":payload}));
            f.reject(result, revision);
        }
        println!("AWR_PAYLOAD_CASE {case}");
    }
}

#[test]
fn prior_content_caches_are_rebuilt_without_rewriting_authority() {
    for (policy, authority, cached, expected) in [
        (3, "Basic YTpi", "Basic YTpi", "[redacted]"),
        (
            8,
            "Call(refreshToken=false); checkpoint_token=null、continue.",
            "[redacted]",
            "Call(refreshToken=false); checkpoint_token=null、continue.",
        ),
        (
            7,
            "Environment:\n\n- Python: 3.12.11",
            "[redacted]",
            "Environment:",
        ),
        (
            7,
            "Environment:\n  HOME: /synthetic-private-value",
            "[redacted]",
            "[redacted]",
        ),
        (
            3,
            "Review basic source-intake requirements",
            "[redacted]",
            "Review basic source-intake requirements",
        ),
        (
            4,
            "Review Bearer authentication requirements",
            "[redacted]",
            "Review Bearer authentication requirements",
        ),
    ] {
        let mut f = Fixture::new();
        let query = SearchQuery {
            kind: Some("work_item".into()),
            ..Default::default()
        };
        f.store.search(f.project, &query).unwrap();
        let conn = f.sql();
        conn.execute(
            "UPDATE work_items SET summary=?1 WHERE external_key='W'",
            [authority],
        )
        .unwrap();
        conn.execute(
            "UPDATE search_documents SET summary=?1 WHERE external_key='W' AND kind='work_item'",
            [cached],
        )
        .unwrap();
        conn.execute("INSERT INTO search_fts(search_fts) VALUES('rebuild')", [])
            .unwrap();
        conn.execute("UPDATE search_state SET policy_version=?1", [policy])
            .unwrap();
        let revision = f.revision();
        let report = f.store.search(f.project, &query).unwrap();
        let hit = report.hits.iter().find(|h| h.external_key == "W").unwrap();
        assert_eq!(hit.summary, expected);
        let stored: String = conn
            .query_row(
                "SELECT summary FROM work_items WHERE external_key='W'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, authority);
        let rebuilt: String = conn
            .query_row(
                "SELECT summary FROM search_documents WHERE external_key='W' AND kind='work_item'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rebuilt, expected);
        assert_eq!(f.revision(), revision);
        assert!(
            f.store
                .events_since(f.project, revision, 100)
                .unwrap()
                .is_empty()
        );
        assert_eq!(fs::read_to_string(f.root.join("work.yaml")).unwrap(), WORK);
    }
    println!("AWR_PAYLOAD_CASE fts_secret_redaction");
}

#[test]
fn legacy_search_redacts_summaries_omits_sensitive_identity_and_rebuilds_the_old_cache() {
    let mut f = Fixture::new();
    f.store
        .search(
            f.project,
            &SearchQuery {
                kind: Some("work_item".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let conn = f.sql();
    conn.execute(
        "UPDATE work_items SET summary=?1 WHERE external_key='W'",
        [format!("password:\n{SENTINEL}")],
    )
    .unwrap();
    conn.execute(
        "UPDATE search_documents SET summary=?1 WHERE kind='work_item'",
        [SENTINEL],
    )
    .unwrap();
    conn.execute("INSERT INTO search_fts(search_fts) VALUES('rebuild')", [])
        .unwrap();
    conn.execute("UPDATE search_state SET policy_version=2", [])
        .unwrap();
    let report = f
        .store
        .search(
            f.project,
            &SearchQuery {
                kind: Some("work_item".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(report.index_policy_version, 9);
    let hit = report.hits.iter().find(|h| h.external_key == "W").unwrap();
    assert_eq!(hit.summary, "[redacted]");
    assert!(!serde_json::to_string(&report).unwrap().contains(SENTINEL));
    let cached: String = conn
        .query_row(
            "SELECT group_concat(summary) FROM search_documents",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!cached.contains(SENTINEL));
    for column in ["external_key", "source_ref_json"] {
        let original: String = conn
            .query_row(
                &format!("SELECT {column} FROM work_items LIMIT 1"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        let replacement = if column == "source_ref_json" {
            let mut reference: Value = serde_json::from_str(&original).unwrap();
            reference["pointer"] = json!(format!("token={SENTINEL}"));
            reference.to_string()
        } else {
            format!("token={SENTINEL}")
        };
        conn.execute(
            &format!(
                "UPDATE work_items SET {column}=?1 WHERE id=(SELECT id FROM work_items LIMIT 1)"
            ),
            [replacement],
        )
        .unwrap();
        conn.execute("UPDATE search_state SET policy_version=2", [])
            .unwrap();
        let report = f
            .store
            .search(
                f.project,
                &SearchQuery {
                    kind: Some("work_item".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!report.hits.iter().any(|h| h.kind == "work_item"));
        assert!(!serde_json::to_string(&report).unwrap().contains(SENTINEL));
        conn.execute(&format!("UPDATE work_items SET {column}=?1"), [original])
            .unwrap();
    }
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM search_documents WHERE kind='work_item'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    // The legacy authority is left intact; no history rewrite is claimed as erasure.
    let summary: String = conn
        .query_row("SELECT summary FROM work_items LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(summary.contains(SENTINEL));
    println!("AWR_PAYLOAD_CASE fts_secret_redaction");
}

#[test]
fn legacy_required_facts_withhold_l0_l1_and_checkpoint_delta_without_false_completion() {
    let mut f = Fixture::new();
    let session = f.session();
    let request = awr_context::ContextRequest {
        work_item_key: Some("W".into()),
        session_id: Some(session.id),
        token_budget: 10_000,
        ..Default::default()
    };
    let before = awr_context::compile_context(&mut f.store, &f.root, &request).unwrap();
    assert!(before.completeness.complete);
    let conn = f.sql();
    conn.execute("UPDATE work_items SET next_action=?1,payload_json=json_set(payload_json,'$.next_action',?1) WHERE external_key='W'",[format!("password: {SENTINEL}")]).unwrap();
    let l1 = awr_context::compile_context(&mut f.store, &f.root, &request).unwrap_err();
    assert_eq!(l1.code(), "ContextIncomplete");
    assert!(!l1.to_string().contains(SENTINEL));
    let l0 = awr_context::bootstrap(
        &mut f.store,
        &f.root,
        &awr_context::BootstrapRequest {
            work_item_key: Some("W".into()),
            session_id: Some(session.id),
            token_budget: 10_000,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(l0.code(), "ContextIncomplete");
    assert!(!l0.to_string().contains(SENTINEL));
    conn.execute("UPDATE work_items SET next_action='Draft the analysis',payload_json=json_set(payload_json,'$.next_action','Draft the analysis')",[]).unwrap();
    let cp = f
        .store
        .create_checkpoint(
            f.project,
            f.revision(),
            session.id,
            CheckpointDraft {
                context_hash: "a".repeat(64),
                digest: "Reviewed progress".into(),
                next_action: "Continue".into(),
                open_loops: vec![],
                changed_entities: vec![],
            },
        )
        .unwrap()
        .0;
    conn.execute(
        "UPDATE checkpoints SET digest=?1 WHERE id=?2",
        rusqlite::params![format!("private_prompt: {SENTINEL}"), cp.id.to_string()],
    )
    .unwrap();
    // Delta exposes checkpoint identity/baseline only; excluded digest content must stay excluded.
    let delta = awr_context::recent_delta(
        &f.store,
        f.project,
        "W",
        None,
        &awr_context::DeltaRequest {
            session_id: Some(session.id),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!serde_json::to_string(&delta).unwrap().contains(SENTINEL));
    let l0 = awr_context::bootstrap(
        &mut f.store,
        &f.root,
        &awr_context::BootstrapRequest {
            work_item_key: Some("W".into()),
            session_id: Some(session.id),
            token_budget: 10_000,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(l0.code(), "ContextIncomplete");
    assert!(!l0.to_string().contains(SENTINEL));
    println!("AWR_PAYLOAD_CASE context_secret_withholding");
}

#[test]
fn ordinary_security_discussion_stays_writable_searchable_and_context_complete() {
    let mut f = Fixture::new();
    let session = f.session();
    let text = "Review password protection and token budgets; discuss API keys and environment variables. Use refreshToken=false; checkpoint_token=null、continue. The basic source-intake example covers basic authentication and Bearer authentication concepts. Schema: {\"token\":{\"type\":\"string\"},\"authorization\":{\"type\":\"http\",\"scheme\":\"bearer\"}}";
    fs::write(
        f.root.join("work.yaml"),
        WORK.replace("Draft the analysis", &serde_json::to_string(text).unwrap()),
    )
    .unwrap();
    let report = index_project(
        &mut f.store,
        &f.root,
        &Manifest::load(&f.root).unwrap(),
        false,
    )
    .unwrap();
    assert!(report.ok, "{:?}", report.issues);
    f.event(
        text,
        json!({"body":"API_KEY=${EXAMPLE_API_KEY}","metrics":{"pending_secret_conditions":16}}),
    )
    .unwrap();
    let draft = f.evidence(text);
    f.store
        .record_evidence(f.project, f.revision(), draft)
        .unwrap();
    let result = f
        .store
        .search(
            f.project,
            &SearchQuery {
                text: Some("password protection".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!result.hits.is_empty());
    let report = awr_context::compile_context(
        &mut f.store,
        &f.root,
        &awr_context::ContextRequest {
            work_item_key: Some("W".into()),
            session_id: Some(session.id),
            token_budget: 10_000,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.completeness.complete);
    assert!(report.rendered_context().contains(text));
    println!("AWR_PAYLOAD_CASE ordinary_text_allowed");
}
