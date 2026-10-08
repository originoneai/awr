#![cfg(feature = "pg-tests")]
//! TEAM-P6 dependency graph / split / resource tests on real PostgreSQL.
//! Fixture isolation comes from tests/common (CR #40 P2-9): this file never
//! reads the runtime AWR_TEAM_DATABASE_URL and only cleans the database this
//! process created.

use awr_team::{SourceActivationPlan, WorkContract, WorkId};
use awr_team_pg::{
    DependencyEdge, EdgeMutation, GraphStore, IngestRequest, LeaseStore, PgError, ResourceBound,
    ResourceLeaseBind, SourceFile, SourceStore, paths_conflict, reference_shared_outcome,
    require_main_scope, resources_conflict, validate_required_graph,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::MutexGuard;
use tokio_postgres::Client;

mod common;
use common::{fresh_team_schema, test_config, with_app_role};

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";
const ACTOR: &str = "actor-a";
const REVIEWER: &str = "actor-b";

fn parent_contract() -> WorkContract {
    WorkContract {
        dependency_acceptance: Default::default(),
        execution_settlement: None,
        codec: WorkContract::CODEC.into(),
        work_id: WorkId::new("work-a").unwrap(),
        external_key: "W".into(),
        goals: vec!["parent goal".into()],
        hard_rules: vec!["parent rule".into()],
        scope_paths: vec!["src".into()],
        acceptance: vec!["parent acceptance".into()],
        required_dependencies: vec![],
        completion_policy: "evidence".into(),
        verification_requirements: vec!["report".into()],
    }
}

/// A valid contract for a graph node. Source admission derives its impact from the
/// persisted contracts and refuses rows whose stored hash does not match their
/// content, so the fixture rows must be real contracts, not placeholders.
fn node_contract(work: &str, external_key: &str) -> WorkContract {
    WorkContract {
        work_id: WorkId::new(work).unwrap(),
        external_key: external_key.into(),
        acceptance: vec!["child".into()],
        ..parent_contract()
    }
}

async fn setup() -> (MutexGuard<'static, ()>, Client, GraphStore, String) {
    let (guard, admin, db) = fresh_team_schema().await;
    let contract_json = serde_json::to_string(&parent_contract()).unwrap();
    let contract_hash = parent_contract().hash().unwrap();
    let (work_b, work_c) = (node_contract("work-b", "X"), node_contract("work-c", "Y"));
    let (hash_b, hash_c) = (work_b.hash().unwrap(), work_c.hash().unwrap());
    let (json_b, json_c) = (
        serde_json::to_string(&work_b).unwrap(),
        serde_json::to_string(&work_c).unwrap(),
    );
    admin
        .batch_execute(&format!(
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
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
                ('tenant-a','project-a','work-a','W'),
                ('tenant-a','project-a','work-b','X'),
                ('tenant-a','project-a','work-c','Y');
             INSERT INTO awr_team.source_snapshots(
                tenant_id, project_id, id, manifest_digest, source_ref_json, parser_version, created_by)
                VALUES ('tenant-a','project-a','snap-1','digest','{{}}','p1','actor-a');
             UPDATE awr_team.projects SET active_snapshot_id='snap-1'
                WHERE tenant_id='tenant-a' AND id='project-a';
             INSERT INTO awr_team.work_contracts(
                tenant_id, project_id, snapshot_id, scope_id, work_id, contract_hash,
                definition_state, title, contract_json)
                VALUES
                ('tenant-a','project-a','snap-1','main','work-a','{contract_hash}','enabled','W','{}'),
                ('tenant-a','project-a','snap-1','main','work-b','{hash_b}','enabled','X','{json_b}'),
                ('tenant-a','project-a','snap-1','main','work-c','{hash_c}','enabled','Y','{json_c}');",
            contract_json.replace('\'', "''"),
            json_b = json_b.replace('\'', "''"),
            json_c = json_c.replace('\'', "''"),
        ))
        .await
        .unwrap();
    let store = GraphStore::from_config(with_app_role(&test_config(), &db));
    (guard, admin, store, db)
}

#[test]
fn prefix_rules_are_segment_based() {
    assert!(paths_conflict("prefix", "src/foo", "file", "src/foo/a.rs"));
    assert!(!paths_conflict("prefix", "src/a", "file", "src/abc"));
    assert!(
        validate_required_graph(
            &["a".into()],
            &[DependencyEdge {
                from: "a".into(),
                to: "missing".into(),
                relation: "requires".into(),
                required: true
            }]
        )
        .is_err()
    );
    assert!(require_main_scope("legacy").is_err());
}

#[tokio::test]
async fn invalid_graph_is_rejected_without_partial_edges() {
    let (_lock, admin, store, _db) = setup().await;
    let err = store
        .replace_edges(
            TENANT,
            PROJECT,
            "snap-1",
            "main",
            &["work-a".into(), "work-b".into()],
            &[
                DependencyEdge {
                    from: "work-a".into(),
                    to: "work-b".into(),
                    relation: "requires".into(),
                    required: true,
                },
                DependencyEdge {
                    from: "work-b".into(),
                    to: "work-a".into(),
                    relation: "requires".into(),
                    required: true,
                },
            ],
        )
        .await
        .unwrap_err();
    match &err {
        PgError::DependencyCycle(path) => {
            assert_eq!(path.first(), path.last());
            assert!(path.len() >= 3, "explainable path: {path:?}");
        }
        other => panic!("expected cycle path, got {other}"),
    }
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.dependency_edges", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
}

#[tokio::test]
async fn prefix_conflict_is_not_string_equality() {
    let (_lock, _, store, _db) = setup().await;
    store
        .reserve(TENANT, PROJECT, "work-a", "prefix", "src/foo")
        .await
        .unwrap();
    let err = store
        .reserve(TENANT, PROJECT, "work-b", "file", "src/foo/bar.rs")
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::ResourceConflict));
    store
        .reserve(TENANT, PROJECT, "work-b", "file", "src/foobar.rs")
        .await
        .unwrap();
}

#[tokio::test]
async fn split_children_do_not_complete_parent_and_bindings_invalidate() {
    let (_lock, _, store, _db) = setup().await;
    let split = store
        .propose_split(
            TENANT,
            PROJECT,
            "work-a",
            &["work-a-1".into(), "work-a-2".into()],
            &json!({"acceptance": "inherited"}),
        )
        .await
        .unwrap();
    assert_eq!(split.child_work_ids.len(), 2);
    let err = store
        .complete_parent_from_children(TENANT, PROJECT, "work-a")
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::ParentEvidenceRequired));
    store
        .bind_dependency(TENANT, PROJECT, "work-b", "work-a", "bind-1")
        .await
        .unwrap();
    store
        .invalidate_downstream(TENANT, PROJECT, "work-a")
        .await
        .unwrap();
    let valid = store
        .current_binding_valid(TENANT, PROJECT, "work-b", "work-a")
        .await
        .unwrap();
    assert!(!valid);
}

#[tokio::test]
async fn claimed_work_blocks_contract_change_and_unknown_scope_is_explicit() {
    let (_lock, admin, store, _db) = setup().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.sessions(
                tenant_id, project_id, id, scope_id, work_id, actor_id, client_id, conversation_id, state)
             VALUES ('tenant-a','project-a','s1','main','work-a','actor-a','c1','conv','active');
             INSERT INTO awr_team.claims(
                tenant_id, project_id, id, scope_id, work_id, session_id, actor_id, fence, expires_at, state)
             VALUES ('tenant-a','project-a','cl1','main','work-a','s1','actor-a',1, now() + interval '1 hour','active');",
        )
        .await
        .unwrap();
    let err = store
        .activation_blocked_by_claims(TENANT, PROJECT, "work-a", "hash-new")
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::ClaimBlocksActivation));
    let current_hash = parent_contract().hash().unwrap();
    store
        .activation_blocked_by_claims(TENANT, PROJECT, "work-a", &current_hash)
        .await
        .unwrap();
    let err = store
        .replace_edges(
            TENANT,
            PROJECT,
            "snap-1",
            "feature",
            &["work-a".into()],
            &[],
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::ScopeUnsupported));
    let err = store
        .graph_within_budget(
            &vec![
                DependencyEdge {
                    from: "a".into(),
                    to: "b".into(),
                    relation: "requires".into(),
                    required: true,
                };
                3
            ],
            2,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::GraphBudgetExceeded));
}

// CR #40 P2-1: two conflicting reservations entering the check/insert gap
// concurrently must not both succeed; non-overlapping ones both succeed.
#[tokio::test]
async fn concurrent_conflicting_reservations_do_not_both_commit() {
    let (_lock, _, store, _db) = setup().await;
    let (a, b) = tokio::join!(
        store.reserve(TENANT, PROJECT, "work-a", "prefix", "src/foo"),
        store.reserve(TENANT, PROJECT, "work-b", "file", "src/foo/a.rs")
    );
    let wins = [a.is_ok(), b.is_ok()].iter().filter(|x| **x).count();
    assert_eq!(
        wins, 1,
        "conflicting reservations both committed: {a:?} {b:?}"
    );
    store
        .reserve(TENANT, PROJECT, "work-a", "file", "src/other/b.rs")
        .await
        .unwrap();
    store
        .reserve(TENANT, PROJECT, "work-b", "file", "src/other/c.rs")
        .await
        .unwrap();
}

// CR #40 P2-2: path aliases of one file conflict, and the stored key is the
// canonical form; '..' is rejected at the entry.
#[tokio::test]
async fn path_aliases_cannot_bypass_the_conflict_check() {
    let (_lock, admin, store, _db) = setup().await;
    store
        .reserve(TENANT, PROJECT, "work-a", "file", "src/foo/a.rs")
        .await
        .unwrap();
    for alias in ["src//foo/a.rs", "src/./foo/a.rs", "./src/foo/a.rs"] {
        let err = store
            .reserve(TENANT, PROJECT, "work-b", "file", alias)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PgError::ResourceConflict),
            "alias {alias}: got {err}"
        );
    }
    // Mixed separators: the parent segment must be recognized AFTER
    // separator unification, and nothing may be reserved (CR #57 P2-1).
    let before: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.resource_reservations", &[])
        .await
        .unwrap()
        .get(0);
    for traversal in [
        "src/../secret",
        "src\\..\\secret",
        "src\\../secret",
        "src/..\\secret",
    ] {
        let err = store
            .reserve(TENANT, PROJECT, "work-b", "file", traversal)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PgError::UnsafeSourcePath(_)),
            "traversal {traversal}: got {err}"
        );
    }
    let after: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.resource_reservations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after, "rejected traversals left reservations");
    // Positive control: plain backslashes normalize to the same file.
    store
        .reserve(TENANT, PROJECT, "work-b", "file", "src\\foo\\b.rs")
        .await
        .unwrap();
    let err = store
        .reserve(TENANT, PROJECT, "work-a", "file", "src/foo/b.rs")
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::ResourceConflict), "got {err}");
    let stored: String = admin
        .query_one(
            "SELECT canonical_key FROM awr_team.resource_reservations LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored, "src/foo/a.rs");
}

// CR #40 P2-3: two individually acyclic replacements serialize; the final
// graph is one complete replacement, never a cyclic union.
#[tokio::test]
async fn concurrent_graph_replacements_cannot_form_a_cyclic_union() {
    let (_lock, admin, store, _db) = setup().await;
    let forward = DependencyEdge {
        from: "work-a".into(),
        to: "work-b".into(),
        relation: "requires".into(),
        required: true,
    };
    let backward = DependencyEdge {
        from: "work-b".into(),
        to: "work-a".into(),
        relation: "requires".into(),
        required: true,
    };
    let nodes = vec!["work-a".to_string(), "work-b".to_string()];
    let forward_edges = [forward.clone()];
    let backward_edges = [backward.clone()];
    let (a, b) = tokio::join!(
        store.replace_edges(TENANT, PROJECT, "snap-1", "main", &nodes, &forward_edges),
        store.replace_edges(TENANT, PROJECT, "snap-1", "main", &nodes, &backward_edges)
    );
    a.unwrap();
    b.unwrap();
    let edges: Vec<(String, String)> = admin
        .query(
            "SELECT from_work_id, to_work_id FROM awr_team.dependency_edges WHERE snapshot_id='snap-1'",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    assert_eq!(edges.len(), 1, "cyclic union committed: {edges:?}");
    assert!(
        edges == vec![forward.clone().into_tuple()] || edges == vec![backward.clone().into_tuple()],
        "surviving edges are not one complete replacement: {edges:?}"
    );
}

trait EdgeTuple {
    fn into_tuple(self) -> (String, String);
}
impl EdgeTuple for DependencyEdge {
    fn into_tuple(self) -> (String, String) {
        (self.from, self.to)
    }
}

// CR #40 P2-4: child contracts are real contracts whose stored hash
// recomputes; CR #40 P2-5: invalid child identities are rejected.
#[tokio::test]
async fn split_child_contracts_recompute_hashes_and_reject_bad_identities() {
    let (_lock, admin, store, _db) = setup().await;
    let split = store
        .propose_split(TENANT, PROJECT, "work-a", &["work-a-1".into()], &json!({}))
        .await
        .unwrap();
    assert_eq!(split.child_work_ids, vec!["work-a-1".to_string()]);
    let contract_row = admin
        .query_one(
            "SELECT contract_hash, contract_json FROM awr_team.work_contracts
             WHERE work_id='work-a-1'",
            &[],
        )
        .await
        .unwrap();
    let stored_hash: String = contract_row.get(0);
    let stored_json: serde_json::Value = contract_row.get(1);
    let child_contract: WorkContract = serde_json::from_value(stored_json).unwrap();
    assert_eq!(child_contract.work_id.as_str(), "work-a-1");
    assert_eq!(
        child_contract.acceptance,
        vec!["parent acceptance".to_string()]
    );
    assert_eq!(stored_hash, child_contract.hash().unwrap());
    assert_ne!(stored_hash, "work-a-1", "hash must not be the raw child id");
    // identity rules
    let err = store
        .propose_split(TENANT, PROJECT, "work-b", &["work-b".into()], &json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Protocol(_)), "self split: got {err}");
    let err = store
        .propose_split(TENANT, PROJECT, "work-a", &["work-a-1".into()], &json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Protocol(_)),
        "existing child: got {err}"
    );
    let err = store
        .propose_split(
            TENANT,
            PROJECT,
            "work-a",
            &["work-dup".into(), "work-dup".into()],
            &json!({}),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Protocol(_)),
        "duplicate child: got {err}"
    );
}

// CR #40 P2-6: graph.json with edges is refused in V1 instead of being
// validated and silently dropped.
#[tokio::test]
async fn graph_json_with_edges_is_refused_not_dropped() {
    let (_lock, _, store, db) = setup().await;
    let sources = SourceStore::from_config(with_app_role(&test_config(), &db));
    let pkg = |files: Vec<SourceFile>| IngestRequest {
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        actor_id: ACTOR.into(),
        parser_version: "p2".into(),
        files,
    };
    let contract_file = SourceFile {
        path: "contract.json".into(),
        bytes: serde_json::to_vec(&parent_contract()).unwrap(),
    };
    let graph_file = SourceFile {
        path: "graph.json".into(),
        bytes: serde_json::to_vec(&json!({
            "nodes": ["work-a", "ghost"],
            "edges": [{"from": "work-a", "to": "ghost", "relation": "requires", "required": true}]
        }))
        .unwrap(),
    };
    let candidate = sources
        .ingest(pkg(vec![contract_file, graph_file]))
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
    let err = sources
        .activate(
            TENANT,
            PROJECT,
            ACTOR,
            &candidate.proposal_id,
            &SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                parser_version: candidate.parser_version.clone(),
                expected_authority_epoch: "0".into(),
                approved_candidate_digest: candidate.manifest_digest.clone(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Protocol(_)), "got {err}");
    let _ = store;
}

// CR #40 P2-7/P2-8: a claimed split child that the new projection REMOVES
// blocks activation; an expired-but-unswept claim does NOT block.
#[tokio::test]
async fn removed_claimed_child_blocks_activation_but_expired_claim_does_not() {
    let (_lock, admin, store, db) = setup().await;
    store
        .propose_split(TENANT, PROJECT, "work-a", &["work-a-1".into()], &json!({}))
        .await
        .unwrap();
    let leases = LeaseStore::from_config(with_app_role(&test_config(), &db));
    let session = leases
        .start_session(
            TENANT, PROJECT, ACTOR, "client-a", "conv", "main", "work-a-1",
        )
        .await
        .unwrap();
    let claim = leases
        .claim(TENANT, PROJECT, &session.id, ACTOR, "client-a", "c1", 3600)
        .await
        .unwrap();
    let sources = SourceStore::from_config(with_app_role(&test_config(), &db));
    let attempt_activate = |epoch: &str| {
        let sources = &sources;
        let epoch = epoch.to_string();
        async move {
            let candidate = sources
                .ingest(IngestRequest {
                    tenant_id: TENANT.into(),
                    project_id: PROJECT.into(),
                    actor_id: ACTOR.into(),
                    parser_version: "p2".into(),
                    files: vec![SourceFile {
                        path: "contract.json".into(),
                        bytes: serde_json::to_vec(&parent_contract()).unwrap(),
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
            sources
                .activate(
                    TENANT,
                    PROJECT,
                    ACTOR,
                    &candidate.proposal_id,
                    &SourceActivationPlan {
                        candidate_digest: candidate.manifest_digest.clone(),
                        parser_version: candidate.parser_version.clone(),
                        expected_authority_epoch: epoch,
                        approved_candidate_digest: candidate.manifest_digest.clone(),
                    },
                )
                .await
        }
    };
    // The child is claimed and the new projection removes it: blocked.
    let err = attempt_activate("0").await.unwrap_err();
    assert!(matches!(err, PgError::ClaimBlocksActivation), "got {err}");
    let still: String = admin
        .query_one(
            "SELECT active_snapshot_id FROM awr_team.projects WHERE id='project-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(still, "snap-1", "blocked activation switched the source");
    // Backdate the claim past expiry (state stays active, sweep not run):
    // an expired claim must NOT block (CR #40 P2-8).
    admin
        .execute(
            "UPDATE awr_team.claims SET expires_at = clock_timestamp() - interval '1 second' WHERE id=$1",
            &[&claim.id],
        )
        .await
        .unwrap();
    attempt_activate("0")
        .await
        .expect("expired claim must not block activation");
}

// WS-031: concurrent edge mutations serialize; a cyclic union cannot commit,
// and dangling endpoints are refused against authoritative contracts.
#[tokio::test]
async fn concurrent_edge_mutations_cannot_form_a_cycle_or_dangling_ref() {
    let (_lock, admin, store, _db) = setup().await;
    let forward = [EdgeMutation::Upsert(DependencyEdge {
        from: "work-a".into(),
        to: "work-b".into(),
        relation: "requires".into(),
        required: true,
    })];
    let backward = [EdgeMutation::Upsert(DependencyEdge {
        from: "work-b".into(),
        to: "work-a".into(),
        relation: "requires".into(),
        required: true,
    })];
    let (a, b) = tokio::join!(
        store.apply_edge_mutations(TENANT, PROJECT, "snap-1", "main", &forward),
        store.apply_edge_mutations(TENANT, PROJECT, "snap-1", "main", &backward),
    );
    let outcomes = [a, b];
    let oks: Vec<_> = outcomes.iter().filter(|r| r.is_ok()).collect();
    let errs: Vec<_> = outcomes.iter().filter(|r| r.is_err()).collect();
    assert_eq!(
        oks.len(),
        1,
        "exactly one mutation may commit: {outcomes:?}"
    );
    assert_eq!(
        errs.len(),
        1,
        "the other must refuse the cyclic union: {outcomes:?}"
    );
    match errs[0].as_ref().unwrap_err() {
        PgError::DependencyCycle(path) => {
            assert_eq!(path.first(), path.last());
            assert!(path.len() >= 3, "explainable path: {path:?}");
        }
        other => panic!("expected cycle path, got {other}"),
    }
    let edges: Vec<(String, String)> = admin
        .query(
            "SELECT from_work_id, to_work_id FROM awr_team.dependency_edges WHERE snapshot_id='snap-1'",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    assert_eq!(edges.len(), 1, "cyclic union committed: {edges:?}");

    let dangling = store
        .apply_edge_mutations(
            TENANT,
            PROJECT,
            "snap-1",
            "main",
            &[EdgeMutation::Upsert(DependencyEdge {
                from: "work-a".into(),
                to: "missing-work".into(),
                relation: "requires".into(),
                required: true,
            })],
        )
        .await
        .unwrap_err();
    assert!(matches!(dangling, PgError::MissingDependency));
}

#[tokio::test]
async fn cross_stream_chain_ready_only_when_all_necessary_deps_satisfied() {
    let (_lock, _, store, _db) = setup().await;
    // A(work-a) → B(work-b) → later A consumer(work-c): acyclic cross-stream shape.
    store
        .apply_edge_mutations(
            TENANT,
            PROJECT,
            "snap-1",
            "main",
            &[
                EdgeMutation::Upsert(DependencyEdge {
                    from: "work-b".into(),
                    to: "work-a".into(),
                    relation: "requires".into(),
                    required: true,
                }),
                EdgeMutation::Upsert(DependencyEdge {
                    from: "work-c".into(),
                    to: "work-b".into(),
                    relation: "requires".into(),
                    required: true,
                }),
            ],
        )
        .await
        .unwrap();
    let partial = BTreeSet::from(["work-a".into()]);
    let ready = store
        .work_necessary_deps_ready(TENANT, PROJECT, "snap-1", "main", "work-c", &partial)
        .await
        .unwrap();
    assert_eq!(ready, Err(vec!["work-b".into()]));
    let full = BTreeSet::from(["work-a".into(), "work-b".into()]);
    let ready = store
        .work_necessary_deps_ready(TENANT, PROJECT, "snap-1", "main", "work-c", &full)
        .await
        .unwrap();
    assert_eq!(ready, Ok(()));
    let left = reference_shared_outcome("work-a", "art", "contract");
    let right = reference_shared_outcome("work-a", "art", "contract");
    assert_eq!(left, right);
}

#[tokio::test]
async fn replace_edges_rejects_cycle_with_explainable_path() {
    let (_lock, _, store, _db) = setup().await;
    let err = store
        .replace_edges(
            TENANT,
            PROJECT,
            "snap-1",
            "main",
            &["work-a".into(), "work-b".into(), "work-c".into()],
            &[
                DependencyEdge {
                    from: "work-a".into(),
                    to: "work-b".into(),
                    relation: "requires".into(),
                    required: true,
                },
                DependencyEdge {
                    from: "work-b".into(),
                    to: "work-c".into(),
                    relation: "requires".into(),
                    required: true,
                },
                DependencyEdge {
                    from: "work-c".into(),
                    to: "work-a".into(),
                    relation: "requires".into(),
                    required: true,
                },
            ],
        )
        .await
        .unwrap_err();
    match err {
        PgError::DependencyCycle(path) => {
            assert_eq!(path.first(), path.last());
            assert!(
                path.windows(2).all(|w| {
                    [
                        ("work-a", "work-b"),
                        ("work-b", "work-c"),
                        ("work-c", "work-a"),
                    ]
                    .iter()
                    .any(|(a, b)| w[0] == *a && w[1] == *b)
                }),
                "path must follow real edges: {path:?}"
            );
        }
        other => panic!("expected explainable cycle, got {other}"),
    }
}

#[tokio::test]
async fn worktree_local_and_shared_external_resources_are_distinct() {
    let (_lock, _admin, store, _db) = setup().await;
    store
        .reserve_bound(
            TENANT,
            PROJECT,
            "work-a",
            &ResourceBound {
                kind: "file".into(),
                key: "src/shared.rs".into(),
                worktree_id: "wt-a".into(),
            },
            ResourceLeaseBind {
                lease_generation: 1,
                fence: 1,
            },
            None,
        )
        .await
        .unwrap();
    // Same path in another worktree is allowed.
    store
        .reserve_bound(
            TENANT,
            PROJECT,
            "work-b",
            &ResourceBound {
                kind: "file".into(),
                key: "src/shared.rs".into(),
                worktree_id: "wt-b".into(),
            },
            ResourceLeaseBind {
                lease_generation: 1,
                fence: 2,
            },
            None,
        )
        .await
        .expect("different worktrees must not contend over local files");
    // Shared external resource contends regardless of worktree.
    store
        .reserve_bound(
            TENANT,
            PROJECT,
            "work-a",
            &ResourceBound {
                kind: "external".into(),
                key: "postgres://shared/db".into(),
                worktree_id: String::new(),
            },
            ResourceLeaseBind {
                lease_generation: 1,
                fence: 1,
            },
            None,
        )
        .await
        .unwrap();
    let err = store
        .reserve_bound(
            TENANT,
            PROJECT,
            "work-b",
            &ResourceBound {
                kind: "external".into(),
                key: "postgres://shared/db".into(),
                worktree_id: String::new(),
            },
            ResourceLeaseBind {
                lease_generation: 2,
                fence: 3,
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::ResourceConflict), "got {err}");
    let err = store
        .reserve_bound(
            TENANT,
            PROJECT,
            "work-b",
            &ResourceBound {
                kind: "external".into(),
                key: "postgres://shared/db".into(),
                worktree_id: "wt-b".into(),
            },
            ResourceLeaseBind::default(),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Protocol(_)),
        "shared external must reject worktree_id: {err}"
    );
}

#[test]
fn resource_conflict_helper_matches_acceptance() {
    assert!(!resources_conflict(
        &ResourceBound {
            kind: "dir".into(),
            key: "src".into(),
            worktree_id: "a".into(),
        },
        &ResourceBound {
            kind: "dir".into(),
            key: "src".into(),
            worktree_id: "b".into(),
        },
    ));
    assert!(resources_conflict(
        &ResourceBound {
            kind: "integration".into(),
            key: "ref/prod".into(),
            worktree_id: String::new(),
        },
        &ResourceBound {
            kind: "integration".into(),
            key: "ref/prod".into(),
            worktree_id: String::new(),
        },
    ));
}

#[test]
fn exclusive_workspace_conflicts_with_contained_paths() {
    let wt = "wt-1";
    let workspace = ResourceBound {
        kind: "workspace".into(),
        key: "root".into(),
        worktree_id: wt.into(),
    };
    let file = ResourceBound {
        kind: "file".into(),
        key: "src/main.rs".into(),
        worktree_id: wt.into(),
    };
    let dir = ResourceBound {
        kind: "dir".into(),
        key: "src".into(),
        worktree_id: wt.into(),
    };
    let other_wt_file = ResourceBound {
        kind: "file".into(),
        key: "src/main.rs".into(),
        worktree_id: "wt-2".into(),
    };
    assert!(resources_conflict(&workspace, &file));
    assert!(resources_conflict(&file, &workspace));
    assert!(resources_conflict(&workspace, &dir));
    assert!(resources_conflict(&workspace, &workspace));
    assert!(!resources_conflict(&workspace, &other_wt_file));
}
