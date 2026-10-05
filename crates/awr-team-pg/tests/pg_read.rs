#![cfg(feature = "pg-tests")]
//! TEAM-P4 consistent-read tests on real PostgreSQL.
//! Fixture isolation comes from tests/common (CR #38 P2-4): this file never
//! reads the runtime AWR_TEAM_DATABASE_URL and only cleans the database this
//! process created.

use awr_team::{SourceActivationPlan, WorkContract, WorkId};
use awr_team_pg::{
    CommandRequest, EventCursor, IngestRequest, PgError, ReadStore, SourceFile, SourceStore,
    TeamStore, dispatch_query,
};
use serde_json::json;
use std::sync::MutexGuard;
use tokio_postgres::Client;
use tokio_postgres::config::Config;

mod common;
use common::{connect_config, fresh_team_schema, test_config, with_app_role, with_db};

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";
const AUTHOR: &str = "actor-a";
const REVIEWER: &str = "actor-b";
const PARSER: &str = "awr-team-source/1";

fn admin_config(db: &str) -> Config {
    with_db(&test_config(), db)
}
fn app_config(db: &str) -> Config {
    with_app_role(&test_config(), db)
}

async fn setup() -> (MutexGuard<'static, ()>, Client, String) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
                ('tenant-a','actor-a','agent','A','active'),
                ('tenant-a','actor-b','human','B','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
                VALUES ('tenant-a','project-a','actor-b','reviewer');
             INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                VALUES ('tenant-a','project-a','main','main','active');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                VALUES ('tenant-a','project-a','work-a','W');",
        )
        .await
        .unwrap();
    (guard, admin, db)
}

fn contract_json_full(rule: &str) -> Vec<u8> {
    let contract = WorkContract {
        dependency_acceptance: Default::default(),
        execution_settlement: None,
        codec: WorkContract::CODEC.into(),
        work_id: WorkId::new("work-a").unwrap(),
        external_key: "W".into(),
        goals: vec!["修复 add 使其返回两数之和".into()],
        hard_rules: vec![rule.into()],
        scope_paths: vec!["src/math.rs".into()],
        acceptance: vec!["add(2, 3) == 5".into()],
        required_dependencies: vec![],
        completion_policy: "evidence".into(),
        verification_requirements: vec!["单元测试报告".into()],
    };
    serde_json::to_vec(&contract).unwrap()
}

async fn activate_rule(db: &str, rule: &str) -> String {
    let sources = SourceStore::from_config(app_config(db));
    let candidate = sources
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: AUTHOR.into(),
            parser_version: PARSER.into(),
            files: vec![SourceFile {
                path: "contract.json".into(),
                bytes: contract_json_full(rule),
            }],
        })
        .await
        .unwrap();
    sources
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            REVIEWER,
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    let epoch = sources
        .current(TENANT, PROJECT, "work-a")
        .await
        .map(|c| c.authority_epoch)
        .unwrap_or_else(|_| "0".into());
    let current = sources
        .activate(
            TENANT,
            PROJECT,
            AUTHOR,
            &candidate.proposal_id,
            &SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                parser_version: candidate.parser_version.clone(),
                expected_authority_epoch: epoch,
                approved_candidate_digest: candidate.manifest_digest,
            },
        )
        .await
        .unwrap();
    current.snapshot_id
}

fn touch(request_id: &str) -> CommandRequest {
    CommandRequest {
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        actor_id: AUTHOR.into(),
        client_id: "client-a".into(),
        request_id: request_id.into(),
        op: "work.touch".into(),
        args: json!({"work_id": "work-a", "scope_id": "main"}),
    }
}

#[tokio::test]
async fn unsupported_queries_are_explicit() {
    let err = dispatch_query("work.complete").unwrap_err();
    assert!(matches!(err, PgError::Unsupported(_)));
}

#[tokio::test]
async fn event_pages_include_event_index_and_survive_reconnect() {
    let (_lock, _, db) = setup().await;
    let store = TeamStore::from_config(app_config(&db));
    let revision = store
        .emit_revision_events(
            TENANT,
            PROJECT,
            AUTHOR,
            vec![
                ("note.a".into(), json!({"n": 1})),
                ("note.b".into(), json!({"n": 2})),
                ("note.c".into(), json!({"n": 3})),
            ],
        )
        .await
        .unwrap();
    let reads = ReadStore::from_config(app_config(&db));
    let mut after = None;
    let mut seen = Vec::new();
    loop {
        let page = reads
            .list_events(TENANT, PROJECT, after.as_deref(), 1)
            .await
            .unwrap();
        assert!(page.events.len() <= 1);
        for event in &page.events {
            let cursor = EventCursor::decode(&event.cursor).unwrap();
            assert_eq!(cursor.project_revision, revision);
            assert_eq!(cursor.event_index, event.event_index);
            seen.push((
                event.project_revision.clone(),
                event.event_index,
                event.event_type.clone(),
            ));
        }
        if page.exhausted {
            break;
        }
        after = Some(page.next_cursor);
    }
    assert_eq!(
        seen,
        vec![
            (revision.to_string(), 0, "note.a".into()),
            (revision.to_string(), 1, "note.b".into()),
            (revision.to_string(), 2, "note.c".into())
        ]
    );
}

// CR #38 P2-3: event revisions are decimal strings at the response
// boundary, even past the JavaScript safe-integer range; cursor pagination remains exact.
#[tokio::test]
async fn event_revision_is_decimal_string_beyond_js_safe_integer() {
    let (_lock, admin, db) = setup().await;
    let big: i64 = 9_007_199_254_740_992; // 2^53
    admin
        .execute(
            "UPDATE awr_team.projects SET project_revision=$1 WHERE id='project-a'",
            &[&big],
        )
        .await
        .unwrap();
    let store = TeamStore::from_config(app_config(&db));
    let revision = store
        .emit_revision_events(
            TENANT,
            PROJECT,
            AUTHOR,
            vec![("note.big".into(), json!({}))],
        )
        .await
        .unwrap();
    assert_eq!(revision, 9_007_199_254_740_993);
    let page = ReadStore::from_config(app_config(&db))
        .list_events(TENANT, PROJECT, None, 10)
        .await
        .unwrap();
    let event = page.events.last().expect("event present");
    assert_eq!(event.project_revision, "9007199254740993");
    let serialized = serde_json::to_value(event).unwrap();
    assert!(
        serialized["project_revision"].is_string(),
        "project_revision must be a JSON string: {serialized}"
    );
    // Positive path: the cursor continues verbatim.
    let next = ReadStore::from_config(app_config(&db))
        .list_events(TENANT, PROJECT, Some(&event.cursor), 10)
        .await
        .unwrap();
    assert!(next.events.is_empty());
}

#[tokio::test]
async fn concurrent_commits_are_ordered_by_project_revision_not_a_sequence() {
    let (_lock, _, db) = setup().await;
    let store = TeamStore::from_config(app_config(&db));
    let a = store.execute(touch("c1"));
    let b = store.execute(touch("c2"));
    let (ra, rb) = tokio::join!(a, b);
    ra.unwrap();
    rb.unwrap();
    let page = ReadStore::from_config(app_config(&db))
        .list_events(TENANT, PROJECT, None, 10)
        .await
        .unwrap();
    let revs: Vec<i64> = page
        .events
        .iter()
        .map(|e| e.project_revision.parse().unwrap())
        .collect();
    let mut sorted = revs.clone();
    sorted.sort();
    assert_eq!(revs, sorted);
    assert_eq!(sorted, vec![1, 2]);
    assert!(page.events.iter().all(|e| e.event_index == 0));
}

#[tokio::test]
async fn prepare_keeps_required_rules_when_budget_is_too_small() {
    let (_lock, _, db) = setup().await;
    let snapshot = activate_rule(&db, "never-omit-this-rule").await;
    let prepared = ReadStore::from_config(app_config(&db))
        .prepare(TENANT, PROJECT, "work-a", Some(3))
        .await
        .unwrap();
    assert_eq!(prepared.authority_snapshot_id, snapshot);
    assert!(prepared.hard_rules.contains(&"never-omit-this-rule".into()));
    assert_eq!(prepared.completeness, "incomplete");
    assert!(
        prepared
            .completeness_reasons
            .contains(&"required_content_exceeds_budget".into())
    );
}

// CR #38 P2-2: every required contract field reaches the consumer, and
// completeness is judged over the full required content.
#[tokio::test]
async fn prepare_delivers_full_required_contract_content() {
    let (_lock, _, db) = setup().await;
    activate_rule(&db, "不得修改公共接口").await;
    let prepared = ReadStore::from_config(app_config(&db))
        .prepare(TENANT, PROJECT, "work-a", None)
        .await
        .unwrap();
    assert_eq!(prepared.completeness, "complete");
    assert_eq!(prepared.goals, vec!["修复 add 使其返回两数之和"]);
    assert_eq!(prepared.scope_paths, vec!["src/math.rs"]);
    assert_eq!(prepared.acceptance, vec!["add(2, 3) == 5"]);
    assert_eq!(prepared.completion_policy, "evidence");
    assert_eq!(prepared.verification_requirements, vec!["单元测试报告"]);
    for fragment in [
        "goal: 修复 add 使其返回两数之和",
        "scope: src/math.rs",
        "acceptance: add(2, 3) == 5",
        "completion_policy: evidence",
        "verification: 单元测试报告",
        "rule: 不得修改公共接口",
    ] {
        assert!(
            prepared.required_context.iter().any(|c| c == fragment),
            "required_context missing {fragment:?}"
        );
    }
}

// CR #38 P2-2: a contract without goals must NOT read as complete.
#[tokio::test]
async fn prepare_marks_missing_goals_as_incomplete() {
    let (_lock, admin, db) = setup().await;
    let snapshot = activate_rule(&db, "r").await;
    admin
        .execute(
            "INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
             VALUES ('tenant-a','project-a','work-b','W2')",
            &[],
        )
        .await
        .unwrap();
    // Store a goal-less contract WITH a valid recomputed hash, so the only
    // defect under test is the missing required content (not the hash).
    let mut goalless: serde_json::Value = serde_json::from_slice(&contract_json_full("r")).unwrap();
    goalless["goals"] = serde_json::json!([]);
    goalless["work_id"] = serde_json::json!("work-b");
    goalless["external_key"] = serde_json::json!("work-b");
    let goalless_hash = serde_json::from_value::<WorkContract>(goalless.clone())
        .unwrap()
        .hash()
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.work_contracts(tenant_id,project_id,snapshot_id,scope_id,work_id,contract_hash,definition_state,title,contract_json)
             VALUES ('tenant-a','project-a',$1,'main','work-b',$2,'enabled','W2',$3)",
            &[&snapshot, &goalless_hash, &goalless],
        )
        .await
        .unwrap();
    let prepared = ReadStore::from_config(app_config(&db))
        .prepare(TENANT, PROJECT, "work-b", None)
        .await
        .unwrap();
    assert_eq!(prepared.completeness, "incomplete");
    assert!(
        prepared
            .completeness_reasons
            .contains(&"missing_goals".into())
    );
}

// CR #57 P2-2: a legacy record whose contract_hash is the raw child id
// (the old propose_split output shape) must NOT come back from prepare as a
// valid identity; it surfaces as an integrity error instead.
#[tokio::test]
async fn prepare_rejects_legacy_child_id_hash_records() {
    let (_lock, admin, db) = setup().await;
    let snapshot = activate_rule(&db, "r").await;
    let mut legacy_json: serde_json::Value =
        serde_json::from_slice(&contract_json_full("r")).unwrap();
    legacy_json["work_id"] = serde_json::json!("work-a-1");
    legacy_json["external_key"] = serde_json::json!("work-a-1");
    admin
        .execute(
            "INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
             VALUES ('tenant-a','project-a','work-a-1','W1')",
            &[],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.work_contracts(tenant_id,project_id,snapshot_id,scope_id,work_id,contract_hash,definition_state,title,contract_json)
             VALUES ('tenant-a','project-a',$1,'main','work-a-1','work-a-1','enabled','W1',$2)",
            &[&snapshot, &legacy_json],
        )
        .await
        .unwrap();
    let err = ReadStore::from_config(app_config(&db))
        .prepare(TENANT, PROJECT, "work-a-1", None)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Protocol(_)), "got {err}");
    // The healthy record next to it still prepares fine (positive control).
    let ok = ReadStore::from_config(app_config(&db))
        .prepare(TENANT, PROJECT, "work-a", None)
        .await
        .unwrap();
    assert_eq!(ok.completeness, "complete");
}

// CR #38 P2-1a: a higher work_version in ANOTHER scope must not leak into
// the main-scope prepare result.
#[tokio::test]
async fn prepare_does_not_mix_versions_across_scopes() {
    let (_lock, admin, db) = setup().await;
    activate_rule(&db, "r").await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
             VALUES ('tenant-a','project-a','review','review','active');
             INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
             VALUES ('tenant-a','project-a','main','work-a','active',1,0),
                    ('tenant-a','project-a','review','work-a','active',9,0);",
        )
        .await
        .unwrap();
    let prepared = ReadStore::from_config(app_config(&db))
        .prepare(TENANT, PROJECT, "work-a", None)
        .await
        .unwrap();
    assert_eq!(prepared.scope_id, "main");
    assert_eq!(prepared.work_version, "1", "leaked another scope's version");
}

// CR #38 P2-1b: contracts for the same snapshot/work in two scopes must not
// error or mix; the main-scope contract wins by binding, not by LIMIT.
#[tokio::test]
async fn prepare_binds_contract_to_main_scope_when_two_exist() {
    let (_lock, admin, db) = setup().await;
    let snapshot = activate_rule(&db, "main-rule").await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
             VALUES ('tenant-a','project-a','review','review','active');",
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.work_contracts(tenant_id,project_id,snapshot_id,scope_id,work_id,contract_hash,definition_state,title,contract_json)
             SELECT 'tenant-a','project-a',$1,'review','work-a','hash-review','enabled','WR', contract_json
             FROM awr_team.work_contracts
             WHERE tenant_id='tenant-a' AND project_id='project-a' AND work_id='work-a'",
            &[&snapshot],
        )
        .await
        .unwrap();
    let prepared = ReadStore::from_config(app_config(&db))
        .prepare(TENANT, PROJECT, "work-a", None)
        .await
        .unwrap();
    assert_eq!(prepared.scope_id, "main");
    assert_eq!(prepared.hard_rules, vec!["main-rule".to_string()]);
    assert_ne!(prepared.contract_hash, "hash-review");
}

// CR #38 P3: the REAL prepare() must keep one snapshot even when a source
// activation commits between its contract read and its runtime/event reads.
// Deterministic interleave via a barrier, not timing luck.
#[tokio::test]
async fn prepare_reads_one_snapshot_across_a_concurrent_activation() {
    let (_lock, _admin, db) = setup().await;
    let first = activate_rule(&db, "old-rule").await;
    let boundary_before: i64 = 1; // one activation event at revision 1
    let barrier = tokio::sync::Barrier::new(2);
    let reader_db = db.clone();
    let reader = async {
        let store = ReadStore::from_config(app_config(&reader_db));
        store
            .prepare_with_sync_point(TENANT, PROJECT, "work-a", None, &barrier)
            .await
    };
    let writer = async {
        barrier.wait().await; // reader finished its pre-sync reads
        activate_rule(&db, "new-rule").await; // commits revision 2 mid-read
        barrier.wait().await; // release the reader's remaining reads
    };
    let (prepared, _) = tokio::join!(reader, writer);
    let prepared = prepared.unwrap();
    assert_eq!(
        prepared.authority_snapshot_id, first,
        "prepare mixed in the newer snapshot"
    );
    assert_eq!(prepared.hard_rules, vec!["old-rule".to_string()]);
    let cursor = EventCursor::decode(&prepared.snapshot_cursor).unwrap();
    assert_eq!(
        cursor.project_revision, boundary_before,
        "event boundary moved to the mid-read commit (isolation degraded?)"
    );
}

#[tokio::test]
async fn repeatable_read_prepare_does_not_mix_source_versions() {
    let (_lock, _, db) = setup().await;
    let first = activate_rule(&db, "old-rule").await;
    let mut reader = connect_config(&admin_config(&db)).await;
    let tx = reader
        .build_transaction()
        .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
        .start()
        .await
        .unwrap();
    tx.execute("SELECT set_config('awr.tenant_id', $1, true)", &[&TENANT])
        .await
        .unwrap();
    tx.execute("SELECT set_config('awr.project_id', $1, true)", &[&PROJECT])
        .await
        .unwrap();
    let before: String = tx
        .query_one(
            "SELECT p.active_snapshot_id FROM awr_team.projects p WHERE p.id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, first);
    let second = activate_rule(&db, "new-rule").await;
    assert_ne!(first, second);
    let during: String = tx
        .query_one(
            "SELECT p.active_snapshot_id FROM awr_team.projects p WHERE p.id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let rule: serde_json::Value = tx
        .query_one(
            "SELECT c.contract_json FROM awr_team.work_contracts c
             WHERE c.snapshot_id=$1 AND c.work_id='work-a' AND c.scope_id='main'",
            &[&during],
        )
        .await
        .unwrap()
        .get(0);
    tx.commit().await.unwrap();
    assert_eq!(during, first);
    assert_eq!(rule["hard_rules"][0], "old-rule");
    let after = ReadStore::from_config(app_config(&db))
        .prepare(TENANT, PROJECT, "work-a", None)
        .await
        .unwrap();
    assert_eq!(after.authority_snapshot_id, second);
    assert_eq!(after.hard_rules, vec!["new-rule".to_string()]);
}

#[tokio::test]
async fn stale_epoch_cursor_and_missing_session_are_explicit() {
    let (_lock, _, db) = setup().await;
    let reads = ReadStore::from_config(app_config(&db));
    let err = reads
        .inspect_session(TENANT, PROJECT, "missing")
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::SessionNotFound));
    let err = reads
        .list_events(
            TENANT,
            PROJECT,
            Some("awr-team-cursor-v1:other-epoch:0:-1"),
            10,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::EpochChanged));
}
