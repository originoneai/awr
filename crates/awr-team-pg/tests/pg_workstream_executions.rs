#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::{
    AdoptedConsumerEdge, PgError, ProviderChangeKind, RecordPlanningChangeRequest,
    SelectiveInvalidateRequest, SelectiveInvalidationStore, WorkstreamCommand, WorkstreamReadStore,
};
use common::{test_config, with_app_role};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

mod workspace_settlement_tests {
    use super::*;
    use awr_team::{ExecutionSettlementMode, ExecutionSettlementPolicy, WorkContract};
    use std::{
        path::{Path, PathBuf},
        process::Command,
    };

    const BASELINE_RESOURCE: &str = "local-repository:z-regression";

    struct GitFixture(PathBuf);

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct GitTransition {
        _fixture: GitFixture,
        statement: Value,
        actual_paths: Vec<String>,
        contribution_paths: Vec<String>,
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn commit(root: &Path, path: Option<&str>, contents: &str, message: &str) -> String {
        if let Some(path) = path {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            git(
                root,
                &[
                    "add",
                    "--",
                    path.strip_prefix(root).unwrap().to_str().unwrap(),
                ],
            );
            git(root, &["commit", "-q", "-m", message]);
        } else {
            git(root, &["commit", "-q", "--allow-empty", "-m", message]);
        }
        git(root, &["rev-parse", "HEAD"])
    }

    fn revision(value: &str) -> Value {
        json!({"resource":BASELINE_RESOURCE,"format":"git_sha1","value":value})
    }

    fn changed_paths(root: &Path, before: &str, after: &str) -> Vec<String> {
        let output = git(root, &["diff", "--name-only", before, after]);
        output
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn init_git_fixture() -> GitFixture {
        let path =
            std::env::temp_dir().join(format!("awr-z-baseline-transition-{}", awr_core::Id::new()));
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["config", "user.name", "AWR regression"]);
        git(
            &path,
            &["config", "user.email", "awr-regression@example.invalid"],
        );
        GitFixture(path)
    }

    fn fast_forward_transition() -> GitTransition {
        let fixture = init_git_fixture();
        let before = commit(&fixture.0, Some("README.md"), "baseline\n", "baseline");
        let imported = commit(
            &fixture.0,
            Some("src/api/baseline/imported.rs"),
            "pub const IMPORTED: bool = true;\n",
            "imported baseline",
        );
        git(&fixture.0, &["reset", "--hard", &before]);
        git(&fixture.0, &["merge", "--ff-only", &imported]);
        let after = git(&fixture.0, &["rev-parse", "HEAD"]);
        let actual_paths = changed_paths(&fixture.0, &before, &after);
        let contribution_paths = changed_paths(&fixture.0, &imported, &after);
        assert_eq!(after, imported);
        assert_eq!(contribution_paths, Vec::<String>::new());
        GitTransition {
            statement: json!({
                "kind":"caller_asserted_git_baseline_v1",
                "operation":"fast_forward_only",
                "workspace_before_revision":revision(&before),
                "imported_revision":revision(&imported),
                "workspace_after_revision":revision(&after),
                "contribution_paths":contribution_paths,
            }),
            _fixture: fixture,
            actual_paths,
            contribution_paths: Vec::new(),
        }
    }

    fn no_ff_transition() -> GitTransition {
        let fixture = init_git_fixture();
        let merge_base = commit(&fixture.0, Some("README.md"), "baseline\n", "baseline");
        git(&fixture.0, &["switch", "-q", "-c", "imported"]);
        let imported = commit(
            &fixture.0,
            Some("src/api/baseline/imported.rs"),
            "pub const IMPORTED: bool = true;\n",
            "imported baseline",
        );
        git(
            &fixture.0,
            &["switch", "-q", "-c", "workspace", &merge_base],
        );
        let before = commit(
            &fixture.0,
            Some("src/api/frontend/preexisting.rs"),
            "pub const PREEXISTING: bool = true;\n",
            "pre-existing workspace contribution",
        );
        git(
            &fixture.0,
            &[
                "merge",
                "-q",
                "--no-ff",
                "imported",
                "-m",
                "merge imported baseline",
            ],
        );
        let after = git(&fixture.0, &["rev-parse", "HEAD"]);
        let actual_paths = changed_paths(&fixture.0, &before, &after);
        let contribution_paths = changed_paths(&fixture.0, &imported, &after);
        assert_eq!(
            git(&fixture.0, &["rev-list", "--parents", "-n", "1", &after])
                .split_whitespace()
                .count(),
            3,
            "the fixture must be a genuine two-parent merge"
        );
        GitTransition {
            statement: json!({
                "kind":"caller_asserted_git_baseline_v1",
                "operation":"merge_no_ff",
                "workspace_before_revision":revision(&before),
                "imported_revision":revision(&imported),
                "workspace_after_revision":revision(&after),
                "contribution_paths":contribution_paths,
            }),
            _fixture: fixture,
            actual_paths,
            contribution_paths,
        }
    }

    fn conflicted_no_ff_transition(include_imported_path: bool) -> GitTransition {
        let fixture = init_git_fixture();
        let conflict_path = "src/api/frontend/conflict.rs";
        let merge_base = commit(
            &fixture.0,
            Some(conflict_path),
            "pub const VALUE: &str = \"base\";\n",
            "baseline",
        );
        git(&fixture.0, &["switch", "-q", "-c", "imported"]);
        std::fs::write(
            fixture.0.join(conflict_path),
            "pub const VALUE: &str = \"imported\";\n",
        )
        .unwrap();
        if include_imported_path {
            let imported_path = fixture.0.join("src/api/baseline/imported.rs");
            std::fs::create_dir_all(imported_path.parent().unwrap()).unwrap();
            std::fs::write(imported_path, "pub const IMPORTED: bool = true;\n").unwrap();
        }
        git(&fixture.0, &["add", "--", "src/api"]);
        git(&fixture.0, &["commit", "-q", "-m", "imported baseline"]);
        let imported = git(&fixture.0, &["rev-parse", "HEAD"]);
        git(
            &fixture.0,
            &["switch", "-q", "-c", "workspace", &merge_base],
        );
        let before = commit(
            &fixture.0,
            Some(conflict_path),
            "pub const VALUE: &str = \"workspace\";\n",
            "workspace before baseline merge",
        );
        let merge = Command::new("git")
            .arg("-C")
            .arg(&fixture.0)
            .args([
                "merge",
                "--no-ff",
                "imported",
                "-m",
                "merge imported baseline",
            ])
            .output()
            .unwrap();
        assert!(
            !merge.status.success(),
            "fixture must produce a merge conflict"
        );
        std::fs::write(
            fixture.0.join(conflict_path),
            "pub const VALUE: &str = \"authored-resolution\";\n",
        )
        .unwrap();
        git(&fixture.0, &["add", "--", conflict_path]);
        git(
            &fixture.0,
            &["commit", "-q", "-m", "resolve imported baseline"],
        );
        let after = git(&fixture.0, &["rev-parse", "HEAD"]);
        let actual_paths = changed_paths(&fixture.0, &before, &after);
        let contribution_paths = changed_paths(&fixture.0, &imported, &after);
        assert_eq!(
            git(&fixture.0, &["rev-list", "--parents", "-n", "1", &after])
                .split_whitespace()
                .count(),
            3,
            "the fixture must retain the two-parent merge after resolution"
        );
        GitTransition {
            statement: json!({
                "kind":"caller_asserted_git_baseline_v1",
                "operation":"merge_no_ff",
                "workspace_before_revision":revision(&before),
                "imported_revision":revision(&imported),
                "workspace_after_revision":revision(&after),
                "contribution_paths":contribution_paths,
            }),
            _fixture: fixture,
            actual_paths,
            contribution_paths,
        }
    }

    async fn policy(admin: &Client, work: &str, workspace: &str) {
        policy_mode(
            admin,
            work,
            workspace,
            ExecutionSettlementMode::IndependentWorkspaceV1,
        )
        .await;
    }

    async fn policy_mode(
        admin: &Client,
        work: &str,
        workspace: &str,
        mode: ExecutionSettlementMode,
    ) {
        let mut contract: WorkContract = serde_json::from_value(admin.query_one(
            "SELECT contract_json FROM awr_team.work_contracts WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
            &[&TENANT,&PROJECT,&work],
        ).await.unwrap().get(0)).unwrap();
        contract.codec = WorkContract::CODEC_V3.into();
        contract.completion_policy = ExecutionSettlementPolicy::COMPLETION_POLICY.into();
        contract.execution_settlement = Some(ExecutionSettlementPolicy {
            mode,
            workspace_id: workspace.into(),
        });
        admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE tenant_id=$3 AND project_id=$4 AND work_id=$5",
            &[&json!(contract),&contract.hash().unwrap(),&TENANT,&PROJECT,&work]).await.unwrap();
    }

    async fn start(store: &WorkstreamReadStore, c: &Value, e: &Value) -> Value {
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                admission(store, c, e, "workspace-start").await,
            )
            .await
            .unwrap()["receipt"]["data"]
            .clone()
    }

    async fn declaration(
        store: &WorkstreamReadStore,
        c: &Value,
        e: &Value,
        id: &str,
        outcome: &str,
    ) -> WorkstreamCommand {
        let mut cmd = report(store, e, id, outcome).await;
        cmd.args["workspace_settlement"] = json!({"workspace_id":"clone-alpha","input_digest":"a".repeat(64),
            "environment_digest":"c".repeat(64),"claim_id":c["claim_id"],"expected_fence":c["fence"],
            "expected_lease_version":c["lease_version"],"executor_stopped":true,"no_external_effects":true});
        cmd
    }

    async fn running_with_scope(
        admin: &Client,
        store: &WorkstreamReadStore,
        scope: &str,
    ) -> (Value, Value) {
        policy(admin, "a", "clone-alpha").await;
        let mut contract: WorkContract = serde_json::from_value(
            admin
                .query_one(
                    "SELECT contract_json FROM awr_team.work_contracts
                     WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'",
                    &[&TENANT, &PROJECT],
                )
                .await
                .unwrap()
                .get(0),
        )
        .unwrap();
        contract.scope_paths = vec![scope.into()];
        admin
            .execute(
                "UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2
                 WHERE tenant_id=$3 AND project_id=$4 AND work_id='a'",
                &[
                    &json!(contract),
                    &contract.hash().unwrap(),
                    &TENANT,
                    &PROJECT,
                ],
            )
            .await
            .unwrap();
        let c = claim(store).await;
        let mut prepare = intent(store, &c, "prepare-transition").await;
        prepare.args["declared_scope"] = json!([scope]);
        let e = store
            .commands()
            .execute(TENANT, PROJECT, A, prepare)
            .await
            .unwrap()["receipt"]["data"]
            .clone();
        let started = start(store, &c, &e).await;
        (c, started)
    }

    async fn transition_declaration(
        store: &WorkstreamReadStore,
        claim: &Value,
        execution: &Value,
        request_id: &str,
        observed_paths: &[String],
        transition: &Value,
    ) -> WorkstreamCommand {
        let mut cmd = declaration(store, claim, execution, request_id, "succeeded").await;
        cmd.args["observed_paths"] = json!(observed_paths);
        cmd.args["baseline_transition"] = transition.clone();
        cmd
    }

    fn normalize_legacy_payload(mut payload: Value) -> Value {
        payload["execution_id"] = json!("execution-id");
        payload["claim_id"] = json!("claim-id");
        payload["workspace_settlement"]["claim_id"] = json!("claim-id");
        payload["contract_hash"] = json!("0".repeat(64));
        payload["coordinator_epoch"] = json!("coordinator-epoch");
        payload["workstream_id"] = json!("workstream-id");
        payload["resource_proof"][0]["reservation_id"] = json!("reservation-id");
        if let Some(observation) = payload
            .get_mut("lease_observation")
            .and_then(Value::as_object_mut)
        {
            observation.insert("expires_at".into(), json!("lease-expires-at"));
            observation.insert("observed_at".into(), json!("lease-observed-at"));
        }
        payload
    }

    fn expected_legacy_payload(mode: ExecutionSettlementMode) -> Value {
        let mut payload = json!({
            "outcome":"succeeded",
            "output_digest":"b".repeat(64),
            "observed_paths":["src/api/result.json"],
            "note":"Client observed the process exit.",
            "client_id":"cli-a",
            "session_id":"session-a",
            "session_version":"1",
            "workstream_id":"workstream-id",
            "ownership_version":"1",
            "execution_version":"2",
            "coordinator_epoch":"coordinator-epoch",
            "contract_hash":"0".repeat(64),
            "scope_violation":false,
            "execution_id":"execution-id",
            "work_id":"a",
            "claim_id":"claim-id",
            "fence":"1",
            "input_digest":"a".repeat(64),
            "environment_digest":"c".repeat(64),
            "admission_mode":"caller_managed",
            "admission_lease_version":"1",
            "settlement_policy":{"mode":mode,"workspace_id":"clone-alpha"},
            "workspace_settlement":{
                "workspace_id":"clone-alpha",
                "input_digest":"a".repeat(64),
                "environment_digest":"c".repeat(64),
                "claim_id":"claim-id",
                "expected_fence":"1",
                "expected_lease_version":"1",
                "executor_stopped":true,
                "no_external_effects":true
            },
            "terminal_reported":true,
            "effects_settled":true,
            "artifact_verified":false,
            "settlement_scope":"admitted_workspace_paths",
            "resource_proof":[{
                "reservation_id":"reservation-id",
                "kind":"dir",
                "key":"src/api",
                "workspace_id":"clone-alpha",
                "fence":"1",
                "admission_lease_version":"1"
            }]
        });
        if mode == ExecutionSettlementMode::IndependentWorkspaceV2 {
            payload["lease_observation"] = json!({
                "basis":"live",
                "expires_at":"lease-expires-at",
                "observed_at":"lease-observed-at"
            });
        }
        payload
    }

    #[tokio::test]
    async fn terminal_reporting_binds_live_workspace_runs_without_inferred_facts() {
        for mode in [
            ExecutionSettlementMode::IndependentWorkspaceV1,
            ExecutionSettlementMode::IndependentWorkspaceV2,
        ] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            policy_mode(&admin, "a", "clone-alpha", mode).await;
            let (c, e) = ready_intent(&store).await;
            let started = start(&store, &c, &e).await;
            let before = snapshot(&admin).await;
            let observed = inspect(&store, &e).await;
            let hint = &started["terminal_reporting"];
            assert_eq!(observed["terminal_reporting"], *hint);
            let mut q = query("work.observe");
            q.work_id = Some("a".into());
            let work = store.query(TENANT, PROJECT, A, q).await.unwrap();
            assert_eq!(work["data"]["execution"]["terminal_reporting"], *hint);
            assert!(started["next_action"].as_str().unwrap().contains(
                "Stop and report workspace execution before repository push/publication, external review, shared-target merge or deployment"
            ));
            assert!(hint["action"]["note"].as_str().unwrap().contains(
                "Settle before push/publication, external review, shared-target merge or deployment"
            ));
            for note in [
                started["next_action"].as_str().unwrap(),
                hint["action"]["note"].as_str().unwrap(),
            ] {
                assert!(note.contains("use separate delivery operations"));
                assert!(
                    note.contains(
                        "Prior or unknown external effects from this run require recovery"
                    )
                );
                assert!(note.contains("never declare no_external_effects=true"));
            }
            assert!(hint.to_string().len() < 1500);
            for field in ["when", "because", "recheck_on"] {
                assert!(!hint[field].as_str().unwrap().is_empty());
            }
            assert_eq!(hint["action"]["op"], "execution.report");
            let binding = &hint["action"]["workspace_settlement"];
            assert_eq!(binding["workspace_id"], "clone-alpha");
            assert_eq!(binding["input_digest"], "a".repeat(64));
            assert_eq!(binding["claim_id"], c["claim_id"]);
            assert_eq!(binding["expected_fence"], c["fence"]);
            assert_eq!(binding["expected_lease_version"], c["lease_version"]);
            for field in [
                "environment_digest",
                "executor_stopped",
                "no_external_effects",
            ] {
                assert!(binding[field].is_null(), "{field} must remain unobserved");
            }
            assert_eq!(observed["effects_settled"], false);
            assert_eq!(observed["artifact_verified"], false);
            assert_eq!(observed["execution_authorized"], false);
            assert_eq!(snapshot(&admin).await, before);
            admin
                .execute(
                    "UPDATE awr_team.executions SET environment_digest=$1 WHERE id=$2",
                    &[&"c".repeat(64), &e["execution_id"].as_str().unwrap()],
                )
                .await
                .unwrap();
            assert_eq!(
                inspect(&store, &e).await["terminal_reporting"]["action"]["workspace_settlement"]["environment_digest"],
                "c".repeat(64)
            );
        }
    }

    #[tokio::test]
    async fn terminal_reporting_uses_current_execution_delegation_on_read() {
        use awr_core::*;
        use awr_team_pg::AuthorizationStore;
        use std::collections::BTreeSet;
        let (_g, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        policy_mode(
            &admin,
            "a",
            "clone-alpha",
            ExecutionSettlementMode::IndependentWorkspaceV2,
        )
        .await;
        admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
            INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
            VALUES('reader-tenant','reader-project','alice','Alice','active');
            INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
            VALUES('reader-tenant','reader-project','bind-agent','alice','agent','active')").await.unwrap();
        let authz = AuthorizationStore::from_config(with_app_role(&test_config(), &db));
        for (id, actions) in [
            ("a-read", BTreeSet::from([AuthorizedAction::Inspect])),
            (
                "b-execute",
                BTreeSet::from([
                    AuthorizedAction::StartWork,
                    AuthorizedAction::ClaimCoordination,
                ]),
            ),
        ] {
            let grant = AgentAuthorization {
                id: id.into(),
                authorizer_person_id: PersonId::new("alice").unwrap(),
                responsible_person_id: PersonId::new("alice").unwrap(),
                subject_kind: ExecutionSubjectKind::Agent,
                subject_id: "agent".into(),
                client_id: "cli-a".into(),
                session_id: None,
                model_id: None,
                scope: AuthorizationScope::Task {
                    project_id: PROJECT.into(),
                    work_item_id: "a".into(),
                },
                actions,
                expires_at_ms: None,
                status: AuthorizationStatus::Active,
                revoked_at_ms: None,
                revoked_by: None,
                verifiable_capabilities: vec![],
                self_reported_skill_hints: vec![],
                parent_authorization_id: None,
                maintainer_person_id: None,
                created_at_ms: 1_000,
                binding_id: Some("bind-agent".into()),
            };
            authz
                .issue(
                    TENANT,
                    PROJECT,
                    &IssueAuthorizationRequest {
                        request_key: format!("issue-{id}"),
                        authorization: grant,
                    },
                )
                .await
                .unwrap();
        }
        let (c, e) = ready_intent(&store).await;
        let started = start(&store, &c, &e).await;
        assert!(started["terminal_reporting"].is_object());
        let before = snapshot(&admin).await;
        assert_eq!(
            inspect(&store, &e).await["terminal_reporting"],
            started["terminal_reporting"]
        );
        let mut q = query("work.observe");
        q.work_id = Some("a".into());
        let observed = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert_eq!(
            observed["data"]["execution"]["terminal_reporting"],
            started["terminal_reporting"]
        );
        assert_eq!(snapshot(&admin).await, before);
        admin
            .batch_execute(
                "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='b-execute'",
            )
            .await
            .unwrap();
        let before = snapshot(&admin).await;
        assert!(inspect(&store, &e).await["terminal_reporting"].is_null());
        let cmd = declaration(&store, &c, &started, "revoked-report", "succeeded").await;
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, cmd).await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, before);
    }

    #[tokio::test]
    async fn terminal_reporting_is_omitted_for_nonowners_and_protected_runs() {
        for case in [
            "other_client",
            "write_revoked",
            "membership_read_only",
            "claim_revoked",
            "session_interrupted",
            "new_fence",
            "new_contract",
            "archived_definition",
            "new_epoch",
            "unknown",
            "recovery_blocked",
        ] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            policy_mode(
                &admin,
                "a",
                "clone-alpha",
                ExecutionSettlementMode::IndependentWorkspaceV2,
            )
            .await;
            let (c, e) = ready_intent(&store).await;
            start(&store, &c, &e).await;
            match case {
                "other_client" => {
                    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
                    VALUES($1,$2,'agent','cli-b',$3,1,true,true)", &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
                }
                "write_revoked" => {
                    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
                }
                "membership_read_only" => {
                    admin
                        .batch_execute("UPDATE awr_team.project_memberships SET role='reader'")
                        .await
                        .unwrap();
                }
                "claim_revoked" => {
                    admin
                        .batch_execute("UPDATE awr_team.claims SET state='revoked'")
                        .await
                        .unwrap();
                }
                "session_interrupted" => {
                    admin
                        .batch_execute("UPDATE awr_team.sessions SET state='interrupted'")
                        .await
                        .unwrap();
                }
                "new_fence" => {
                    admin
                        .batch_execute("UPDATE awr_team.work_runtime SET last_fence=last_fence+1")
                        .await
                        .unwrap();
                }
                "new_contract" => {
                    admin
                        .batch_execute("UPDATE awr_team.work_contracts SET contract_hash='changed'")
                        .await
                        .unwrap();
                }
                "archived_definition" => {
                    admin
                        .batch_execute(
                            "UPDATE awr_team.work_contracts SET definition_state='archived'",
                        )
                        .await
                        .unwrap();
                }
                "new_epoch" => {
                    admin
                        .batch_execute("UPDATE awr_team.projects SET coordinator_epoch='new-epoch'")
                        .await
                        .unwrap();
                }
                "unknown" => {
                    admin
                        .batch_execute("UPDATE awr_team.executions SET state='unknown'")
                        .await
                        .unwrap();
                }
                "recovery_blocked" => {
                    admin
                        .batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=true")
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let before = snapshot(&admin).await;
            let mut q = query("execution.inspect");
            q.work_id = Some("a".into());
            q.execution_id = Some(e["execution_id"].as_str().unwrap().into());
            let observed = store
                .query(
                    TENANT,
                    PROJECT,
                    if case == "other_client" { B } else { A },
                    q,
                )
                .await
                .unwrap();
            assert!(observed["data"]["terminal_reporting"].is_null(), "{case}");
            assert_eq!(observed["data"]["execution_authorized"], false, "{case}");
            assert_eq!(snapshot(&admin).await, before, "{case}");
        }
    }

    #[tokio::test]
    async fn expired_reporting_advice_is_only_for_v2_current_claim_and_does_not_authorize_execution()
     {
        for mode in [
            ExecutionSettlementMode::IndependentWorkspaceV1,
            ExecutionSettlementMode::IndependentWorkspaceV2,
        ] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            policy_mode(&admin, "a", "clone-alpha", mode).await;
            let (c, e) = ready_intent(&store).await;
            start(&store, &c, &e).await;
            admin
                .batch_execute(
                    "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
                )
                .await
                .unwrap();
            let before = snapshot(&admin).await;
            let observed = inspect(&store, &e).await;
            assert_eq!(observed["lease_live"], false);
            assert_eq!(observed["execution_authorized"], false);
            assert_eq!(
                observed["terminal_reporting"].is_object(),
                mode == ExecutionSettlementMode::IndependentWorkspaceV2
            );
            if mode == ExecutionSettlementMode::IndependentWorkspaceV2 {
                assert!(
                    observed["terminal_reporting"]["action"]["note"]
                        .as_str()
                        .unwrap()
                        .contains("never execution after expiry")
                );
            }
            assert_eq!(snapshot(&admin).await, before);
        }
    }

    #[tokio::test]
    async fn ordinary_outcomes_settle_exact_resources_and_replay_without_acceptance() {
        for outcome in ["succeeded", "failed", "cancelled"] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            policy(&admin, "a", "clone-alpha").await;
            let (c, e) = ready_intent(&store).await;
            let started = start(&store, &c, &e).await;
            assert_eq!(started["result_authority"], "caller_asserted");
            let cmd = declaration(&store, &c, &started, "workspace-report", outcome).await;
            let response = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd.clone())
                .await
                .unwrap();
            let result = &response["receipt"]["data"];
            assert_eq!(result["state"], outcome);
            assert_eq!(result["receipt_kind"], "caller_asserted");
            assert_eq!(result["terminal_reported"], true);
            assert_eq!(result["effects_settled"], true);
            assert_eq!(result["artifact_verified"], false);
            assert_eq!(result["work_completed"], false);
            assert_eq!(result["recovery_blocked"], false);
            assert_eq!(result["resources_released"], 1);
            let action = result["next_action"].as_str().unwrap();
            if outcome == "succeeded" {
                assert!(action.contains("independent Agent review"));
            } else {
                assert!(action.contains("not task completion"));
                assert!(!action.contains("submit"));
            }
            let observed = inspect(&store, &e).await;
            assert_eq!(observed["state"], outcome);
            assert_eq!(observed["execution_authorized"], false);
            assert_eq!(
                observed["latest_receipt"]["payload"]["effects_settled"],
                true
            );
            let before = snapshot(&admin).await;
            assert_eq!(before["resources"][0]["state"], "released");
            assert_eq!(before["resources"][0]["worktree_id"], "clone-alpha");
            assert_eq!(before["execution"][0]["admission_mode"], "caller_managed");
            let replay = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd.clone())
                .await
                .unwrap();
            assert_eq!(replay["receipt"], response["receipt"]);
            assert_eq!(replay["replayed"], true);
            assert_eq!(snapshot(&admin).await, before);
            let mut changed = cmd;
            changed.args["note"] = json!("Changed assertion");
            assert!(
                store
                    .commands()
                    .execute(TENANT, PROJECT, A, changed)
                    .await
                    .is_err()
            );
            assert_eq!(snapshot(&admin).await, before);
            let again = declaration(&store, &c, result, "another-report", outcome).await;
            assert!(matches!(
                store.commands().execute(TENANT, PROJECT, A, again).await,
                Err(PgError::PreconditionsChanged)
            ));
            assert_eq!(snapshot(&admin).await, before);
        }
    }

    #[tokio::test]
    async fn real_git_fast_forward_records_full_receipt_but_only_returns_provenance() {
        let transition = fast_forward_transition();
        assert_eq!(
            transition.actual_paths,
            vec!["src/api/baseline/imported.rs"]
        );
        assert!(transition.contribution_paths.is_empty());
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        let (claim, execution) = running_with_scope(&admin, &store, "src/api/baseline").await;
        let cmd = transition_declaration(
            &store,
            &claim,
            &execution,
            "fast-forward-transition",
            &transition.actual_paths,
            &transition.statement,
        )
        .await;
        let response = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await
            .unwrap();
        let result = &response["receipt"]["data"];
        assert_eq!(result["state"], "succeeded");
        assert_eq!(result["scope_violation"], false);
        assert_eq!(result["effects_settled"], true);
        assert!(result.get("baseline_transition").is_none());
        assert_eq!(
            result["baseline_transition_provenance"],
            json!({"origin":"caller_asserted","purpose":"provenance_only"})
        );
        let receipt_id = result["receipt_id"].as_str().unwrap();
        let row = admin
            .query_one(
                "SELECT digest,payload_json FROM awr_team.execution_receipts WHERE id=$1",
                &[&receipt_id],
            )
            .await
            .unwrap();
        let stored_digest: String = row.get(0);
        let payload: Value = row.get(1);
        assert_eq!(payload["baseline_transition"], transition.statement);
        assert_eq!(
            payload["baseline_transition_provenance"],
            result["baseline_transition_provenance"]
        );
        assert_eq!(stored_digest, awr_team::request_hash(&payload).unwrap());
        let before_replay = snapshot(&admin).await;
        let replay = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await
            .unwrap();
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["receipt"], response["receipt"]);
        assert_eq!(snapshot(&admin).await, before_replay);
        let mut changed = cmd;
        changed.args["baseline_transition"]["contribution_paths"] =
            json!(["src/api/baseline/relabelled.rs"]);
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, changed).await,
            Err(PgError::IdempotencyConflict)
        ));
        assert_eq!(snapshot(&admin).await, before_replay);
    }

    #[tokio::test]
    async fn real_git_actual_effects_remain_the_unconditional_scope_boundary() {
        let fast_forward = fast_forward_transition();
        let authored_only = conflicted_no_ff_transition(false);
        let mixed = conflicted_no_ff_transition(true);
        assert_eq!(
            authored_only.actual_paths,
            vec!["src/api/frontend/conflict.rs"]
        );
        assert_eq!(authored_only.actual_paths, authored_only.contribution_paths);
        assert_eq!(
            mixed.actual_paths,
            vec![
                "src/api/baseline/imported.rs".to_owned(),
                "src/api/frontend/conflict.rs".to_owned(),
            ]
        );
        assert_eq!(
            mixed.contribution_paths,
            vec!["src/api/frontend/conflict.rs"]
        );
        let mut omitted = mixed.statement.clone();
        omitted["contribution_paths"] = json!([]);
        let mut relabelled = mixed.statement.clone();
        relabelled["contribution_paths"] = json!(["src/api/baseline/imported.rs"]);
        for (case, actual_paths, statement) in [
            (
                "same-import-under-old-frontend-scope",
                fast_forward.actual_paths.clone(),
                fast_forward.statement.clone(),
            ),
            (
                "authored-outside-baseline-scope",
                authored_only.actual_paths.clone(),
                authored_only.statement.clone(),
            ),
            (
                "mixed-import-and-authored-effects",
                mixed.actual_paths.clone(),
                mixed.statement.clone(),
            ),
            (
                "omitted-contribution-attribution",
                mixed.actual_paths.clone(),
                omitted,
            ),
            (
                "relabelled-contribution-attribution",
                mixed.actual_paths.clone(),
                relabelled,
            ),
        ] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            let scope = if case == "same-import-under-old-frontend-scope" {
                "src/api/frontend"
            } else {
                "src/api/baseline"
            };
            let (claim, execution) = running_with_scope(&admin, &store, scope).await;
            let cmd =
                transition_declaration(&store, &claim, &execution, case, &actual_paths, &statement)
                    .await;
            let result = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd)
                .await
                .unwrap()["receipt"]["data"]
                .clone();
            assert_eq!(result["state"], "unknown", "{case}");
            assert_eq!(result["scope_violation"], true, "{case}");
            assert_eq!(result["effects_settled"], false, "{case}");
            assert_eq!(result["resource_release_performed"], false, "{case}");
            assert_eq!(
                result["baseline_transition_provenance"],
                json!({"origin":"caller_asserted","purpose":"provenance_only"}),
                "{case}"
            );
            let state = snapshot(&admin).await;
            assert!(
                state["resources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|resource| resource["state"] == "unknown"),
                "{case}"
            );
            assert_eq!(state["work"][0]["recovery_blocked"], true, "{case}");
        }
    }

    #[tokio::test]
    async fn real_git_no_ff_uses_pre_merge_workspace_as_b_and_imported_revision_as_i() {
        let transition = no_ff_transition();
        assert_eq!(
            transition.actual_paths,
            vec!["src/api/baseline/imported.rs"]
        );
        assert_eq!(
            transition.contribution_paths,
            vec!["src/api/frontend/preexisting.rs"]
        );
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        let (claim, execution) = running_with_scope(&admin, &store, "src/api/baseline").await;
        let cmd = transition_declaration(
            &store,
            &claim,
            &execution,
            "no-ff-transition",
            &transition.actual_paths,
            &transition.statement,
        )
        .await;
        let result = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap()["receipt"]["data"]
            .clone();
        assert_eq!(result["state"], "succeeded");
        assert_eq!(result["scope_violation"], false);
        assert_eq!(result["effects_settled"], true);
        assert_eq!(result["resources_released"], 1);
        assert_eq!(
            result["baseline_transition_provenance"],
            json!({"origin":"caller_asserted","purpose":"provenance_only"})
        );
    }

    #[tokio::test]
    async fn malformed_baseline_transitions_fail_closed_before_any_write() {
        let transition = fast_forward_transition();
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        let (claim, execution) = running_with_scope(&admin, &store, "src/api").await;
        let base = transition_declaration(
            &store,
            &claim,
            &execution,
            "valid-transition",
            &transition.actual_paths,
            &transition.statement,
        )
        .await;
        let mut malformed = Vec::new();
        let mut value = transition.statement.clone();
        value.as_object_mut().unwrap().remove("operation");
        malformed.push(("missing-operation", value));
        let mut value = transition.statement.clone();
        value["operation"] = json!("rebase");
        malformed.push(("unknown-operation", value));
        let mut value = transition.statement.clone();
        value["imported_revision"]["resource"] = json!("local-repository:other");
        malformed.push(("mismatched-resource", value));
        let mut value = transition.statement.clone();
        value["imported_revision"]["format"] = json!("git_sha256");
        value["imported_revision"]["value"] = json!("d".repeat(64));
        malformed.push(("mismatched-format", value));
        let mut value = transition.statement.clone();
        value["workspace_after_revision"]["value"] = json!("NOT-A-GIT-SHA");
        malformed.push(("invalid-revision", value));
        let mut value = transition.statement.clone();
        value["contribution_paths"] = json!(["src/api/../secret"]);
        malformed.push(("noncanonical-path", value));
        let mut value = transition.statement.clone();
        value["contribution_paths"] = json!(["src/api/a", "src/api/a"]);
        malformed.push(("duplicate-path", value));
        let mut value = transition.statement.clone();
        value["contribution_paths"] = json!(
            (0..129)
                .map(|index| format!("src/api/path-{index}"))
                .collect::<Vec<_>>()
        );
        malformed.push(("excessive-paths", value));
        let mut value = transition.statement.clone();
        value["verified"] = json!(true);
        malformed.push(("unknown-field", value));
        let before = snapshot(&admin).await;
        for (index, (case, transition)) in malformed.into_iter().enumerate() {
            let mut cmd = base.clone();
            cmd.request_id = format!("malformed-transition-{index}");
            cmd.args["baseline_transition"] = transition;
            assert!(
                store
                    .commands()
                    .execute(TENANT, PROJECT, A, cmd)
                    .await
                    .is_err(),
                "{case}"
            );
            assert_eq!(snapshot(&admin).await, before, "{case}");
        }
    }

    #[tokio::test]
    async fn baseline_transition_does_not_bypass_source_or_claim_fencing() {
        let transition = fast_forward_transition();
        for mutation in ["claim", "source"] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            let (claim, execution) = running_with_scope(&admin, &store, "src/api").await;
            let cmd = transition_declaration(
                &store,
                &claim,
                &execution,
                &format!("fenced-{mutation}"),
                &transition.actual_paths,
                &transition.statement,
            )
            .await;
            if mutation == "claim" {
                store.commands().execute(TENANT,PROJECT,A,command(&prepare(&store,A,"a").await,"renew-before-transition","claim.renew",
                    json!({"session_id":"session-a","expected_session_version":"1","claim_id":claim["claim_id"],
                        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],"ttl_seconds":600}))).await.unwrap();
            } else {
                policy(&admin, "a", "changed-workspace").await;
            }
            let before = snapshot(&admin).await;
            assert!(matches!(
                store.commands().execute(TENANT, PROJECT, A, cmd).await,
                Err(PgError::PreconditionsChanged)
            ));
            assert_eq!(snapshot(&admin).await, before, "{mutation}");
        }
    }

    #[tokio::test]
    async fn legacy_v1_v2_receipts_keep_exact_absent_shape_and_normalized_hash_goldens() {
        for (mode, golden) in [
            (
                ExecutionSettlementMode::IndependentWorkspaceV1,
                "a1b7a19610712f1c2fca2924a1907f354b5f875ed0b0448d7551d83c7b8665c7",
            ),
            (
                ExecutionSettlementMode::IndependentWorkspaceV2,
                "e5ad077f05791d1af901d15b50b9592db452d4b7cc3f470af84a52d550a12049",
            ),
        ] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            policy_mode(&admin, "a", "clone-alpha", mode).await;
            let (claim, execution) = ready_intent(&store).await;
            let started = start(&store, &claim, &execution).await;
            let cmd = declaration(
                &store,
                &claim,
                &started,
                "legacy-workspace-report",
                "succeeded",
            )
            .await;
            let response = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd)
                .await
                .unwrap();
            let result = &response["receipt"]["data"];
            assert!(result.get("baseline_transition").is_none());
            assert!(result.get("baseline_transition_provenance").is_none());
            let receipt_id = result["receipt_id"].as_str().unwrap();
            let row = admin
                .query_one(
                    "SELECT digest,payload_json FROM awr_team.execution_receipts WHERE id=$1",
                    &[&receipt_id],
                )
                .await
                .unwrap();
            let stored_digest: String = row.get(0);
            let payload: Value = row.get(1);
            assert!(payload.get("baseline_transition").is_none());
            assert!(payload.get("baseline_transition_provenance").is_none());
            assert_eq!(stored_digest, awr_team::request_hash(&payload).unwrap());
            let normalized = normalize_legacy_payload(payload);
            assert_eq!(normalized, expected_legacy_payload(mode));
            assert_eq!(awr_team::request_hash(&normalized).unwrap(), golden);
        }
    }

    #[tokio::test]
    async fn valid_renewal_uses_original_admission_generation_and_refuses_stale_declaration() {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        policy(&admin, "a", "clone-alpha").await;
        let (c, e) = ready_intent(&store).await;
        let started = start(&store, &c, &e).await;
        let renewed=store.commands().execute(TENANT,PROJECT,A,command(&prepare(&store,A,"a").await,"renew-workspace","claim.renew",
            json!({"session_id":"session-a","expected_session_version":"1","claim_id":c["claim_id"],
                "expected_fence":c["fence"],"expected_lease_version":c["lease_version"],"ttl_seconds":600}))).await.unwrap()["receipt"]["data"].clone();
        let observed = inspect(&store, &e).await;
        assert_eq!(
            observed["terminal_reporting"]["action"]["workspace_settlement"]["expected_lease_version"],
            "2"
        );
        let stale = declaration(&store, &c, &started, "stale-renewal", "succeeded").await;
        let before = snapshot(&admin).await;
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, stale).await,
            Err(PgError::PreconditionsChanged)
        ));
        assert_eq!(snapshot(&admin).await, before);
        let cmd = declaration(&store, &renewed, &started, "renewed-report", "succeeded").await;
        let result = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap();
        assert_eq!(result["receipt"]["data"]["effects_settled"], true);
        let after = snapshot(&admin).await;
        assert_eq!(after["resources"][0]["lease_generation"], 1);
        assert_eq!(after["resources"][0]["state"], "released");
        assert_eq!(
            after["receipts"][0]["payload_json"]["workspace_settlement"]["expected_lease_version"],
            "2"
        );
        assert_eq!(
            after["receipts"][0]["payload_json"]["admission_lease_version"],
            "1"
        );
    }

    #[tokio::test]
    async fn declaration_rejects_malformed_trust_and_wrong_bindings_without_writes() {
        for mode in [
            ExecutionSettlementMode::IndependentWorkspaceV1,
            ExecutionSettlementMode::IndependentWorkspaceV2,
        ] {
            check_declaration_rejects_malformed_trust_and_wrong_bindings_without_writes_policy(
                mode,
            )
            .await;
        }
    }

    async fn check_declaration_rejects_malformed_trust_and_wrong_bindings_without_writes_policy(
        mode: ExecutionSettlementMode,
    ) {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        policy_mode(&admin, "a", "clone-alpha", mode).await;
        let (c, e) = ready_intent(&store).await;
        let started = start(&store, &c, &e).await;
        let base = declaration(&store, &c, &started, "valid", "succeeded").await;
        let before = snapshot(&admin).await;
        for (index, (field, value)) in [
            ("trusted_executor", json!(true)),
            ("unknown_field", json!(true)),
            ("workspace_id", json!("other-clone")),
            ("workspace_id", json!("../unsafe")),
            ("input_digest", json!("d".repeat(64))),
            ("environment_digest", json!("malformed")),
            ("claim_id", json!("another-claim")),
            ("expected_fence", json!("999")),
            ("expected_lease_version", json!("999")),
            ("executor_stopped", json!("true")),
            ("no_external_effects", json!(1)),
        ]
        .into_iter()
        .enumerate()
        {
            let mut cmd = base.clone();
            cmd.request_id = format!("reject-{index}");
            cmd.args["workspace_settlement"][field] = value;
            assert!(
                store
                    .commands()
                    .execute(TENANT, PROJECT, A, cmd)
                    .await
                    .is_err(),
                "{field}"
            );
            assert_eq!(snapshot(&admin).await, before, "{field}");
        }
    }

    #[tokio::test]
    async fn uncertain_or_incomplete_terminal_declarations_keep_caller_protection() {
        for mode in [
            ExecutionSettlementMode::IndependentWorkspaceV1,
            ExecutionSettlementMode::IndependentWorkspaceV2,
        ] {
            check_uncertain_or_incomplete_terminal_declarations_keep_caller_protection_policy(mode)
                .await;
        }
    }

    async fn check_uncertain_or_incomplete_terminal_declarations_keep_caller_protection_policy(
        mode: ExecutionSettlementMode,
    ) {
        for case in [
            "omitted",
            "not-stopped",
            "unknown-stop",
            "external-effects",
            "unknown-outcome",
            "scope",
            "expired",
        ] {
            if mode == ExecutionSettlementMode::IndependentWorkspaceV2 && case == "expired" {
                continue;
            }
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            policy_mode(&admin, "a", "clone-alpha", mode).await;
            let (c, e) = ready_intent(&store).await;
            let started = start(&store, &c, &e).await;
            if case == "expired" || mode == ExecutionSettlementMode::IndependentWorkspaceV2 {
                admin.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
            }
            let mut cmd = declaration(&store, &c, &started, case, "succeeded").await;
            match case {
                "omitted" => {
                    cmd.args
                        .as_object_mut()
                        .unwrap()
                        .remove("workspace_settlement");
                }
                "not-stopped" => {
                    cmd.args["workspace_settlement"]["executor_stopped"] = json!(false)
                }
                "unknown-stop" => {
                    cmd.args["workspace_settlement"]["executor_stopped"] = Value::Null
                }
                "external-effects" => {
                    cmd.args["workspace_settlement"]["no_external_effects"] = json!(false)
                }
                "unknown-outcome" => cmd.args["outcome"] = json!("unknown"),
                "scope" => cmd.args["observed_paths"] = json!(["outside/result.json"]),
                _ => {}
            }
            let response = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd)
                .await
                .unwrap();
            let result = &response["receipt"]["data"];
            assert_eq!(result["state"], "unknown", "{case}");
            assert_eq!(result["effects_settled"], false, "{case}");
            assert_eq!(result["artifact_verified"], false, "{case}");
            assert_eq!(result["resource_release_performed"], false, "{case}");
            let snap = snapshot(&admin).await;
            assert_eq!(snap["work"][0]["recovery_blocked"], true, "{case}");
            assert_eq!(snap["resources"][0]["state"], "unknown", "{case}");
            assert_eq!(snap["receipts"][0]["receipt_kind"], "caller_asserted");
        }
    }

    #[tokio::test]
    async fn mismatched_resources_barriers_and_exposed_intents_never_partially_settle() {
        for mode in [
            ExecutionSettlementMode::IndependentWorkspaceV1,
            ExecutionSettlementMode::IndependentWorkspaceV2,
        ] {
            check_mismatched_resources_barriers_and_exposed_intents_never_partially_settle_policy(
                mode,
            )
            .await;
        }
    }

    async fn check_mismatched_resources_barriers_and_exposed_intents_never_partially_settle_policy(
        mode: ExecutionSettlementMode,
    ) {
        for case in [
            "missing",
            "extra",
            "shared",
            "unbound",
            "fence",
            "generation",
            "workspace",
            "unknown",
            "barrier",
            "outbox",
            "other-run",
            "old-contract",
        ] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            policy_mode(&admin, "a", "clone-alpha", mode).await;
            let (c, e) = ready_intent(&store).await;
            let started = start(&store, &c, &e).await;
            if mode == ExecutionSettlementMode::IndependentWorkspaceV2 {
                expire(&admin).await;
            }
            match case {
                "missing" => {
                    admin
                        .batch_execute("DELETE FROM awr_team.resource_reservations")
                        .await
                        .unwrap();
                }
                "extra" => {
                    admin.execute("INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state,execution_id,worktree_id,lease_generation,fence)
                VALUES($1,$2,'extra','a','dir','src/extra','reserved',$3,'clone-alpha',1,1)",&[&TENANT,&PROJECT,&e["execution_id"].as_str().unwrap()]).await.unwrap();
                }
                "shared" => {
                    admin.execute("INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state,execution_id)
                VALUES($1,$2,'shared','a','external','external-test-target','reserved',$3)",&[&TENANT,&PROJECT,&e["execution_id"].as_str().unwrap()]).await.unwrap();
                }
                "unbound" => {
                    admin
                        .batch_execute(
                            "UPDATE awr_team.resource_reservations SET execution_id=NULL",
                        )
                        .await
                        .unwrap();
                }
                "fence" => {
                    admin
                        .batch_execute("UPDATE awr_team.resource_reservations SET fence=fence+1")
                        .await
                        .unwrap();
                }
                "generation" => {
                    admin.batch_execute("UPDATE awr_team.resource_reservations SET lease_generation=lease_generation+1").await.unwrap();
                }
                "workspace" => {
                    admin.batch_execute("UPDATE awr_team.resource_reservations SET worktree_id='different-clone'").await.unwrap();
                }
                "unknown" => {
                    admin
                        .batch_execute("UPDATE awr_team.resource_reservations SET state='unknown'")
                        .await
                        .unwrap();
                }
                "barrier" => {
                    admin.batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=true WHERE work_id='a'").await.unwrap();
                }
                "outbox" => {
                    admin.execute("INSERT INTO awr_team.outbox(tenant_id,project_id,id,state,payload_json,action_kind,aggregate_id)
                VALUES($1,$2,'exposed','sending','{}','execution.dispatch',$3)",&[&TENANT,&PROJECT,&e["execution_id"].as_str().unwrap()]).await.unwrap();
                }
                "other-run" => {
                    admin.execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
                SELECT tenant_id,project_id,'other-run',work_id,fence,contract_hash,executor_actor_id,'unknown' FROM awr_team.executions WHERE id=$1",
                &[&e["execution_id"].as_str().unwrap()]).await.unwrap();
                }
                _ => {
                    policy(&admin, "a", "changed-workspace").await;
                }
            }
            let cmd = declaration(&store, &c, &started, case, "succeeded").await;
            let response = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd)
                .await
                .unwrap();
            let result = &response["receipt"]["data"];
            assert_eq!(result["state"], "unknown", "{case}");
            assert_eq!(result["resource_release_performed"], false, "{case}");
            let released: i64 = admin
                .query_one(
                    "SELECT count(*) FROM awr_team.resource_reservations WHERE state='released'",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            assert_eq!(released, 0, "{case}");
            let blocked: bool = admin
                .query_one(
                    "SELECT recovery_blocked FROM awr_team.work_runtime WHERE work_id='a'",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            assert!(blocked, "{case}");
        }
    }

    #[tokio::test]
    async fn source_policy_requires_nonempty_unique_admitted_scope() {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        policy(&admin, "a", "clone-alpha").await;
        let c = claim(&store).await;
        for (paths, malformed) in [(json!([]), false), (json!(["src/api", "src/api"]), true)] {
            let mut cmd = intent(&store, &c, "invalid-scope").await;
            cmd.args["declared_scope"] = paths;
            let before = snapshot(&admin).await;
            let error = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd)
                .await
                .unwrap_err();
            // Duplicate paths already fail the legacy closed input parser;
            // an empty scope reaches the new source-selected policy check.
            assert!(
                if malformed {
                    matches!(error, PgError::Protocol(_))
                } else {
                    matches!(error, PgError::ScopeExceeded)
                },
                "{error:?}"
            );
            assert_eq!(snapshot(&admin).await, before);
        }
    }

    #[tokio::test]
    async fn legacy_and_controlled_runs_cannot_opt_in_through_report_fields() {
        for controlled in [false, true] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            if controlled {
                policy(&admin, "a", "clone-alpha").await;
                admin.batch_execute("UPDATE awr_team.actors SET kind='system' WHERE id='agent';
                    UPDATE awr_team.workstream_grants SET can_attest_execution=true,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
            }
            let (c, e) = ready_intent(&store).await;
            let mut cmd = admission(&store, &c, &e, "start-mode").await;
            if controlled {
                cmd.args["execution_mode"] = json!("reference_write_v1");
                cmd.args["expected_input_digest"] = json!("a".repeat(64));
            }
            let started = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd)
                .await
                .unwrap()["receipt"]["data"]
                .clone();
            assert!(started["terminal_reporting"].is_null());
            assert!(inspect(&store, &e).await["terminal_reporting"].is_null());
            let note = started["next_action"].as_str().unwrap();
            assert!(!note.contains("workspace execution"));
            assert!(!note.contains("no_external_effects"));
            assert!(note.contains("A replay or unknown response never authorizes another start"));
            let cmd = declaration(&store, &c, &started, "injected-policy", "succeeded").await;
            let before = snapshot(&admin).await;
            assert!(matches!(
                store.commands().execute(TENANT, PROJECT, A, cmd).await,
                Err(PgError::PreconditionsChanged)
            ));
            assert_eq!(snapshot(&admin).await, before);
        }
    }

    #[tokio::test]
    async fn independent_namespaces_run_in_parallel_and_settlement_preserves_other_work() {
        for same_workspace in [false, true] {
            let (_g, admin, _, store) = setup().await;
            admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1").await.unwrap();
            policy(&admin, "a", "clone-alpha").await;
            policy(
                &admin,
                "b-private",
                if same_workspace {
                    "clone-alpha"
                } else {
                    "clone-beta"
                },
            )
            .await;
            let (c, e) = ready_intent(&store).await;
            let started = start(&store, &c, &e).await;
            let p = prepare(&store, B, "b-private").await;
            let c_b=store.commands().execute(TENANT,PROJECT,B,command(&p,"claim-beta","claim.acquire",
                json!({"session_id":"session-b","expected_session_version":"1","expected_work_version":"0","ttl_seconds":600}))).await.unwrap()["receipt"]["data"].clone();
            let p = prepare(&store, B, "b-private").await;
            let e_b=store.commands().execute(TENANT,PROJECT,B,command(&p,"prepare-beta","execution.prepare",
                json!({"session_id":"session-b","expected_session_version":"1","claim_id":c_b["claim_id"],"expected_fence":c_b["fence"],
                    "expected_lease_version":c_b["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
                    "input_digest":"d".repeat(64),"declared_scope":["src/api"]}))).await.unwrap()["receipt"]["data"].clone();
            let p = prepare(&store, B, "b-private").await;
            let cmd = command(
                &p,
                "start-beta",
                "execution.start",
                json!({"session_id":"session-b","expected_session_version":"1",
                "claim_id":c_b["claim_id"],"expected_fence":c_b["fence"],"expected_lease_version":c_b["lease_version"],
                "execution_id":e_b["execution_id"],"expected_execution_version":e_b["execution_version"],
                "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"}),
            );
            let before = snapshot(&admin).await;
            let result = store.commands().execute(TENANT, PROJECT, B, cmd).await;
            if same_workspace {
                assert!(matches!(result, Err(PgError::ResourceConflict)));
                assert_eq!(snapshot(&admin).await, before);
            } else {
                assert_eq!(result.unwrap()["receipt"]["data"]["state"], "running");
                let cmd = declaration(&store, &c, &started, "finish-alpha", "succeeded").await;
                let result = store
                    .commands()
                    .execute(TENANT, PROJECT, A, cmd)
                    .await
                    .unwrap();
                assert_eq!(result["receipt"]["data"]["effects_settled"], true);
                let other=admin.query_one("SELECT state,worktree_id FROM awr_team.resource_reservations WHERE work_id='b-private'",&[]).await.unwrap();
                assert_eq!(other.get::<_, String>(0), "reserved");
                assert_eq!(other.get::<_, String>(1), "clone-beta");
            }
        }
    }

    include!("cases/late_workspace_reports.rs");
}

#[tokio::test]
async fn nonterminal_checkpoint_feedback_does_not_settle_running_execution() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c, &e, "feedback-start").await,
        )
        .await
        .unwrap();
    let before = inspect(&store, &e).await;
    let p = prepare(&store, A, "a").await;
    store.commands().execute(TENANT,PROJECT,A,command(&p,"feedback","session.checkpoint",
        json!({"session_id":"session-a","expected_session_version":"1","context_hash":p["data"]["context_hash"],
            "next_action":"Wait for test results","open_loops":[],
            "progress":{"phase":"testing","summary":"Implementation complete; tests are running."}}))).await.unwrap();
    assert_eq!(inspect(&store, &e).await, before);
    assert_eq!(before["state"], "running");
    assert_eq!(before["recovery_blocked"], false);
    let rows: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.execution_receipts", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn observation_preserves_receipt_privacy_and_exposes_expired_claims_without_mutation() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c, &e, "observe-start").await,
        )
        .await
        .unwrap();
    let running = inspect(&store, &e).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            report(&store, &running, "observe-report", "succeeded").await,
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-b',$3,1,true)", &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    admin.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second' WHERE work_id='a'").await.unwrap();
    let before = snapshot(&admin).await;
    let mut q = query("work.observe");
    q.work_id = Some("a".into());
    let own = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(own["data"]["claim"]["lease_live"], false);
    assert_eq!(own["data"]["execution"]["state"], "unknown");
    assert_eq!(
        own["data"]["execution"]["latest_receipt"]["receipt_kind"],
        "caller_asserted"
    );
    assert_eq!(own["data"]["execution_authorized"], false);
    let peer = store.query(TENANT, PROJECT, B, q).await.unwrap();
    assert_eq!(
        peer["data"]["execution"]["receipt_details_available"],
        false
    );
    assert!(peer["data"]["execution"]["latest_receipt"].is_null());
    assert_eq!(
        peer["data"]["missing"]["execution_receipt"],
        "permission_restricted"
    );
    assert!(
        !peer
            .to_string()
            .contains("Client observed the process exit.")
    );
    assert_eq!(before, snapshot(&admin).await);
}

#[tokio::test]
async fn later_session_cannot_hide_execution_or_gain_its_receipt_authority() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c, &e, "origin-start").await,
        )
        .await
        .unwrap();
    admin.batch_execute("UPDATE awr_team.executions SET state='succeeded',terminal_reported=true WHERE work_id='a';
        UPDATE awr_team.claims SET state='released' WHERE work_id='a';
        UPDATE awr_team.sessions SET state='ended' WHERE id='session-a';
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
          VALUES('reader-tenant','reader-project','agent','cli-b','00000000000000000000000001',1,true);
        INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
          SELECT tenant_id,project_id,'later-session',scope_id,work_id,'reviewer','review-client','later-conversation','active',workstream_id,ownership_version
          FROM awr_team.sessions WHERE id='session-a'").await.unwrap();
    let before = snapshot(&admin).await;
    let mut q = query("work.observe");
    q.work_id = Some("a".into());
    let own = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"].clone();
    assert_eq!(own["session"]["id"], "later-session");
    assert!(own["session_execution"].is_null());
    assert_eq!(own["execution"]["execution_id"], e["execution_id"]);
    assert_eq!(own["execution"]["origin"]["actor_id"], "agent");
    assert_eq!(own["execution"]["origin"]["client_id"], "cli-a");
    assert!(own["execution"]["origin"]["model"].is_null());
    let peer = store.query(TENANT, PROJECT, B, q.clone()).await.unwrap();
    assert_eq!(
        peer["data"]["execution"]["receipt_details_available"],
        false
    );
    assert!(peer["data"]["execution"]["latest_receipt"].is_null());
    assert_eq!(before, snapshot(&admin).await);
    q.session_id = Some("later-session".into());
    assert!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["execution"].is_null()
    );
    q.session_id = Some("session-a".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["execution"]["execution_id"],
        e["execution_id"]
    );
    // A reassigned responsibility does not rewrite the recorded executor.
    admin.batch_execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','reader-project','new-owner','New owner','active');
        INSERT INTO awr_team.task_responsibilities(tenant_id,project_id,work_id,owner_person_id,version)
        VALUES('reader-tenant','reader-project','a','new-owner',1)
        ON CONFLICT(tenant_id,project_id,work_id) DO UPDATE SET owner_person_id='new-owner',version=awr_team.task_responsibilities.version+1").await.unwrap();
    q.session_id = None;
    let reassigned = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(
        reassigned["data"]["responsibility"]["owner_person_id"],
        "new-owner"
    );
    assert_eq!(
        reassigned["data"]["execution"]["origin"]["client_id"],
        "cli-a"
    );
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_ownership SET ownership_version=2 WHERE work_id='a';
        UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=2 WHERE work_id='a';
        UPDATE awr_team.sessions SET ownership_version=2 WHERE id='later-session'",
        )
        .await
        .unwrap();
    assert!(store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["execution"].is_null());
}

async fn claim(store: &WorkstreamReadStore) -> Value {
    store.commands().execute(TENANT,PROJECT,A,command(&prepare(store,A,"a").await,"take","claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":"0","ttl_seconds":60})))
        .await.unwrap()["receipt"]["data"].clone()
}
async fn intent(store: &WorkstreamReadStore, c: &Value, id: &str) -> WorkstreamCommand {
    let p = prepare(store, A, "a").await;
    command(
        &p,
        id,
        "execution.prepare",
        json!({"session_id":"session-a","expected_session_version":"1",
        "claim_id":c["claim_id"],"expected_fence":c["fence"],"expected_lease_version":c["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":"a".repeat(64),"declared_scope":["src/api"]}),
    )
}
async fn cancel(store: &WorkstreamReadStore, e: &Value, id: &str) -> WorkstreamCommand {
    command(
        &prepare(store, A, "a").await,
        id,
        "execution.cancel",
        json!({"session_id":"session-a",
        "expected_session_version":"1","execution_id":e["execution_id"],"expected_execution_version":e["execution_version"]}),
    )
}
async fn inspect(store: &WorkstreamReadStore, e: &Value) -> Value {
    let mut q = query("execution.inspect");
    q.work_id = Some("a".into());
    q.execution_id = Some(e["execution_id"].as_str().unwrap().into());
    store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone()
}
async fn snapshot(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'claims',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM awr_team.claims c),
        'execution',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.executions e),
        'work',(SELECT jsonb_agg(to_jsonb(w) ORDER BY work_id) FROM awr_team.work_runtime w),
        'outbox',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM awr_team.outbox o),
        'receipts',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.execution_receipts r),
        'resources',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.resource_reservations r),
        'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.events e),
        'operations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM awr_team.operations o),
        'revision',(SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2))",
        &[&TENANT,&PROJECT]).await.unwrap().get(0)
}

async fn ready_intent(store: &WorkstreamReadStore) -> (Value, Value) {
    let c = claim(store).await;
    let e = store
        .commands()
        .execute(TENANT, PROJECT, A, intent(store, &c, "prepare").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    (c, e)
}
async fn admission(
    store: &WorkstreamReadStore,
    c: &Value,
    e: &Value,
    id: &str,
) -> WorkstreamCommand {
    let p = prepare(store, A, "a").await;
    command(
        &p,
        id,
        "execution.start",
        json!({"session_id":"session-a","expected_session_version":"1",
        "execution_id":e["execution_id"],"expected_execution_version":e["execution_version"],
        "claim_id":c["claim_id"],"expected_fence":c["fence"],"expected_lease_version":c["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"}),
    )
}
async fn report(
    store: &WorkstreamReadStore,
    e: &Value,
    id: &str,
    outcome: &str,
) -> WorkstreamCommand {
    command(
        &prepare(store, A, "a").await,
        id,
        "execution.report",
        json!({"session_id":"session-a",
        "expected_session_version":"1","execution_id":e["execution_id"],"expected_execution_version":e["execution_version"],
        "outcome":outcome,"output_digest":"b".repeat(64),"observed_paths":["src/api/result.json"],"note":"Client observed the process exit."}),
    )
}

#[tokio::test]
async fn admission_is_atomic_and_only_the_original_response_authorizes_one_start() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    let cmd = admission(&store, &c, &e, "start").await;
    let s = store.commands();
    let (a, b) = tokio::join!(
        s.execute(TENANT, PROJECT, A, cmd.clone()),
        s.execute(TENANT, PROJECT, A, cmd.clone())
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a["receipt"], b["receipt"]);
    assert_ne!(a["execution_authorized"], b["execution_authorized"]);
    assert_eq!(a["execution_authorized"], !a["replayed"].as_bool().unwrap());
    assert_eq!(b["execution_authorized"], !b["replayed"].as_bool().unwrap());
    assert_eq!(a["receipt"]["execution_authorized"], false);
    assert_eq!(a["receipt"]["data"]["admission"], "granted_at_commit");
    let snap = snapshot(&admin).await;
    assert_eq!(snap["resources"].as_array().unwrap().len(), 1);
    assert_eq!(snap["resources"][0]["state"], "reserved");
    assert!(snap["outbox"].is_null());
    assert_eq!(inspect(&store, &e).await["execution_authorized"], false);
    let running = a["receipt"]["data"].clone();
    assert!(matches!(
        s.execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c, &running, "repeat-start").await
        )
        .await,
        Err(PgError::PreconditionsChanged)
    ));
    // Cancellation cannot falsely confirm an already authorized external stop.
    let cancelled = s
        .execute(TENANT, PROJECT, A, cancel(&store, &running, "stop").await)
        .await
        .unwrap();
    assert_eq!(cancelled["receipt"]["data"]["stop_confirmed"], false);
    let before = snapshot(&admin).await;
    let replay = s.execute(TENANT, PROJECT, A, cmd).await.unwrap();
    assert_eq!(replay["execution_authorized"], false);
    assert_eq!(replay["receipt"], a["receipt"]);
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn start_rechecks_wait_contract_lease_and_resources_without_partial_reservations() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    admin.batch_execute("INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state)
        VALUES('reader-tenant','reader-project','wait','session-a','a','Confirm the change?','open')").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, admission(&store, &c, &e, "wait").await)
            .await,
        Err(PgError::WaitOpen)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin.batch_execute("UPDATE awr_team.wait_items SET state='replied';
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','private-owner','b-private','file','src/api/result.json','unknown')").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                admission(&store, &c, &e, "resource").await
            )
            .await,
        Err(PgError::ResourceConflict)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute(
            "UPDATE awr_team.resource_reservations SET state='released';
        UPDATE awr_team.work_contracts SET definition_state='archived' WHERE work_id='a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                admission(&store, &c, &e, "archived").await
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.work_contracts SET definition_state='enabled' WHERE work_id='a';
        UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                admission(&store, &c, &e, "expired").await
            )
            .await,
        Err(PgError::LeaseExpired)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn start_cannot_use_changed_contract_or_claim_caller_selected_trust() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    let cmd = admission(&store, &c, &e, "start").await;
    let before = snapshot(&admin).await;
    for (field, value) in [
        ("expected_execution_version", json!("3")),
        ("expected_work_version", json!("999")),
        ("expected_lease_version", json!("999")),
        ("expected_session_version", json!("999")),
    ] {
        let mut request = cmd.clone();
        request.args[field] = value;
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, request).await,
            Err(PgError::PreconditionsChanged)
        ));
    }
    let mut bad = cmd.clone();
    bad.args["execution_mode"] = json!("hard_fence");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, bad).await,
        Err(PgError::Unsupported(_))
    ));
    for field in [
        "receipt_kind",
        "executor_actor_id",
        "client_id",
        "fencing_class",
    ] {
        let mut bad = cmd.clone();
        bad.args[field] = json!("trusted_executor");
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, bad).await,
            Err(PgError::Protocol(_))
        ));
    }
    assert_eq!(snapshot(&admin).await, before);
    let mut contract: awr_team::WorkContract = serde_json::from_value(
        admin
            .query_one(
                "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='a'",
                &[],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    contract.acceptance.push("new acceptance".into());
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='a'",
        &[&json!(contract),&contract.hash().unwrap()]).await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                admission(&store, &c, &e, "current-contract").await
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn required_dependencies_need_current_receipts_and_do_not_infer_cross_stream_exports() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    // Create a contract before preparation; upstream source/runtime status alone
    // cannot substitute for a current accepted completion receipt.
    let mut contract: awr_team::WorkContract = serde_json::from_value(
        admin
            .query_one(
                "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='a'",
                &[],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    contract.required_dependencies = vec!["c".into()];
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='a'",
        &[&json!(contract),&contract.hash().unwrap()]).await.unwrap();
    assert!(
        admin
            .batch_execute(
                "INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state)
        VALUES('reader-tenant','reader-project','main','c','completed')",
            )
            .await
            .is_err()
    );
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state)
        VALUES('reader-tenant','reader-project','main','c','ready')",
        )
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &prepare(&store, A, "a").await,
                    "blocked-claim",
                    "claim.acquire",
                    json!({"session_id":"session-a","expected_session_version":"1",
                        "expected_work_version":"0","ttl_seconds":60})
                )
            )
            .await,
        Err(PgError::MissingDependency)
    ));
    assert_eq!(snapshot(&admin).await, before);
    // Obtain the lease and intent under a valid dependency receipt. Then
    // invalidate that receipt to test execution admission independently of
    // the earlier claim guard; neither entry can rely on an upstream flag.
    admin.batch_execute("INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
        dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json)
        SELECT tenant_id,project_id,'initially-accepted',work_id,scope_id,contract_hash,'result','dependencies','evidence','review','[\"reviewer\"]'
        FROM awr_team.work_contracts WHERE work_id='c';
        UPDATE awr_team.work_runtime SET state='completed',selected_completion_id='initially-accepted' WHERE work_id='c'").await.unwrap();
    let (c, e) = ready_intent(&store).await;
    admin.batch_execute("UPDATE awr_team.work_runtime SET state='ready',selected_completion_id=NULL WHERE work_id='c'").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                admission(&store, &c, &e, "dependency").await
            )
            .await,
        Err(PgError::BindingInvalid)
    ));
    assert_eq!(snapshot(&admin).await, before);
    // The same check fails closed even when the caller can read both streams.
    admin
        .execute(
            "UPDATE awr_team.workstream_snapshot_ownership SET workstream_id=$1 WHERE work_id='c'",
            &[&awr_core::Id::from(2).to_string()],
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'agent','cli-a',$3,1,true,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(2).to_string()]).await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                admission(&store, &c, &e, "cross-stream").await
            )
            .await,
        Err(PgError::BindingInvalid)
    ));
    // A current accepted receipt is usable within its own stream; an old
    // contract's receipt is not. These are pre-existing synthetic delivery rows,
    // not a claim that this scoped surface already exposes completion creation.
    admin
        .execute(
            "UPDATE awr_team.workstream_snapshot_ownership SET workstream_id=$1 WHERE work_id='c'",
            &[&awr_core::Id::from(1).to_string()],
        )
        .await
        .unwrap();
    admin.batch_execute("INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
        dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json)
        VALUES('reader-tenant','reader-project','accepted','c','main','old-contract','result','dependencies','evidence','review','[\"reviewer\"]');
        UPDATE awr_team.work_runtime SET state='completed',selected_completion_id='accepted' WHERE work_id='c'").await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                admission(&store, &c, &e, "old-receipt").await
            )
            .await,
        Err(PgError::BindingInvalid)
    ));
    admin.batch_execute("INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
        dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json)
        SELECT tenant_id,project_id,'accepted-current',work_id,scope_id,contract_hash,'result','dependencies','evidence','review','[\"reviewer\"]'
        FROM awr_team.work_contracts WHERE work_id='c';
        UPDATE awr_team.work_runtime SET selected_completion_id='accepted-current' WHERE work_id='c'").await.unwrap();
    let accepted = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c, &e, "accepted").await,
        )
        .await
        .unwrap();
    assert_eq!(
        accepted["receipt"]["data"]["dependency_receipts"],
        json!([["c", "accepted-current"]])
    );
    assert_eq!(accepted["execution_authorized"], true);
}

#[tokio::test]
async fn caller_reports_preserve_observations_and_unknown_effects_after_lease_expiry() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    let running = store
        .commands()
        .execute(TENANT, PROJECT, A, admission(&store, &c, &e, "start").await)
        .await
        .unwrap();
    let e = &running["receipt"]["data"];
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let cmd = report(&store, e, "report", "succeeded").await;
    let result = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd.clone())
        .await
        .unwrap();
    assert_eq!(result["receipt"]["data"]["state"], "unknown");
    assert_eq!(result["receipt"]["data"]["reported_outcome"], "succeeded");
    assert_eq!(result["receipt"]["data"]["work_completed"], false);
    assert_eq!(result["execution_authorized"], false);
    let snap = snapshot(&admin).await;
    assert_eq!(snap["resources"][0]["state"], "unknown");
    assert_eq!(snap["work"][0]["recovery_blocked"], true);
    assert_eq!(snap["receipts"][0]["receipt_kind"], "caller_asserted");
    assert_eq!(
        snap["receipts"][0]["payload_json"]["output_digest"],
        "b".repeat(64)
    );
    assert!(snap["execution"][0]["result_digest"].is_null());
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    assert_eq!(replay["receipt"], result["receipt"]);
    assert_eq!(snapshot(&admin).await, snap);
    // Contradictory later observations are audit history, not a terminal rewrite.
    let mut e = result["receipt"]["data"].clone();
    for outcome in ["cancelled", "failed", "unknown"] {
        let mut cmd = report(&store, &e, outcome, outcome).await;
        cmd.args["observed_paths"] = json!(["outside/escaped.json"]);
        let res = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap();
        assert_eq!(res["receipt"]["data"]["scope_violation"], true);
        assert_eq!(res["receipt"]["data"]["state"], "unknown");
        e = res["receipt"]["data"].clone();
    }
    assert_eq!(
        snapshot(&admin).await["receipts"].as_array().unwrap().len(),
        4
    );
}

#[tokio::test]
async fn caller_report_marks_only_execution_bound_resources_unknown_after_takeover() {
    let (_g, admin, _, store, legacy) = setup_with_legacy_resource().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    let started = store
        .commands()
        .execute(TENANT, PROJECT, A, admission(&store, &c, &e, "start").await)
        .await
        .unwrap();
    let running = &started["receipt"]["data"];
    let owned = running["resources"][0]["reservation_id"].as_str().unwrap();
    admin.batch_execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
        VALUES('reader-tenant','reader-project','other-execution','a',0,'old','old-runner','unknown');
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state,execution_id)
        VALUES('reader-tenant','reader-project','other-resource','a','named','other','reserved','other-execution')").await.unwrap();
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            report(&store, running, "report", "succeeded").await,
        )
        .await
        .unwrap();
    let states = admin
        .query_one(
            "SELECT
                (SELECT state FROM awr_team.resource_reservations WHERE id=$1),
                (SELECT execution_id FROM awr_team.resource_reservations WHERE id=$1),
                (SELECT state FROM awr_team.resource_reservations WHERE id='other-resource'),
                (SELECT state FROM awr_team.resource_reservations WHERE id=$2)",
            &[&owned, &legacy],
        )
        .await
        .unwrap();
    assert_eq!(states.get::<_, String>(0), "unknown");
    assert_eq!(
        states.get::<_, Option<String>>(1).as_deref(),
        running["execution_id"].as_str()
    );
    assert_eq!(states.get::<_, String>(2), "reserved");
    assert_eq!(states.get::<_, String>(3), "reserved");
}

#[tokio::test]
async fn empty_admission_resources_remain_a_valid_report_binding() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let mut prepare = intent(&store, &c, "prepare-empty").await;
    prepare.args["declared_scope"] = json!([]);
    let e = store
        .commands()
        .execute(TENANT, PROJECT, A, prepare)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let started = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c, &e, "start-empty").await,
        )
        .await
        .unwrap();
    assert_eq!(started["receipt"]["data"]["resources"], json!([]));
    let mut observation = report(
        &store,
        &started["receipt"]["data"],
        "report-empty",
        "succeeded",
    )
    .await;
    observation.args["observed_paths"] = json!([]);
    let result = store
        .commands()
        .execute(TENANT, PROJECT, A, observation)
        .await
        .unwrap();
    assert_eq!(result["receipt"]["data"]["state"], "unknown");
    assert_eq!(snapshot(&admin).await["resources"], Value::Null);
}

#[tokio::test]
async fn another_client_cannot_start_or_report_and_report_cannot_invent_trust() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'agent','cli-b',$3,1,true,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let cmd = admission(&store, &c, &e, "start").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                report(&store, &e, "early", "succeeded").await
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let started = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    let cmd = report(&store, &started["receipt"]["data"], "report", "succeeded").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    let mut trusted = cmd.clone();
    trusted.args["receipt_kind"] = json!("trusted_executor");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, trusted).await,
        Err(PgError::Protocol(_))
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, cmd).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn failed_start_and_report_events_roll_back_resources_receipts_and_runtime() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let (c, e) = ready_intent(&store).await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_lifecycle_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type IN ('execution.start','execution.report') THEN RAISE EXCEPTION 'synthetic failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER reject_lifecycle_event BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_lifecycle_event()").await.unwrap();
    let cmd = admission(&store, &c, &e, "start").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute("ALTER TABLE awr_team.events DISABLE TRIGGER reject_lifecycle_event")
        .await
        .unwrap();
    let started = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    admin
        .batch_execute("ALTER TABLE awr_team.events ENABLE TRIGGER reject_lifecycle_event")
        .await
        .unwrap();
    let cmd = report(&store, &started["receipt"]["data"], "report", "succeeded").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, cmd).await,
        Err(PgError::Db(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn intents_cancel_without_dispatch_and_replay_never_resurrects_them() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let cmd = intent(&store, &c, "prepare").await;
    let one = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd.clone())
        .await
        .unwrap();
    let e = &one["receipt"]["data"];
    assert_eq!(one["receipt"]["execution_authorized"], false);
    assert_eq!(e["dispatched"], false);
    assert_eq!(e["admission"], "not_evaluated");
    assert_eq!(e["execution_state_basis"], "at_commit");
    assert_eq!(e["work_version"], "2");
    assert!(snapshot(&admin).await["outbox"].is_null());
    assert_eq!(inspect(&store, e).await["owned_by_client"], true);
    assert_eq!(inspect(&store, e).await["execution_authorized"], false);
    let mut start = cmd.clone();
    start.op = "execution.dispatch".into();
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, start).await,
        Err(PgError::Unsupported(_))
    ));
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, intent(&store, &c, "second").await)
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    // Cancellation remains possible after the planning lease expires.
    let stopped = store
        .commands()
        .execute(TENANT, PROJECT, A, cancel(&store, e, "cancel").await)
        .await
        .unwrap();
    assert_eq!(stopped["receipt"]["data"]["stop_confirmed"], true);
    assert_eq!(stopped["receipt"]["data"]["state"], "cancelled");
    let before = snapshot(&admin).await;
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], one["receipt"]);
    assert_eq!(inspect(&store, e).await["state"], "cancelled");
    assert_eq!(snapshot(&admin).await, before);
    store.commands().execute(TENANT,PROJECT,A,command(&prepare(&store,A,"a").await,"release","claim.release",
        json!({"session_id":"session-a","expected_session_version":"1","claim_id":c["claim_id"],
            "expected_fence":c["fence"],"expected_lease_version":c["lease_version"]}))).await.unwrap();
}

#[tokio::test]
async fn concurrent_intents_and_exact_retries_create_one_execution() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let cmd = intent(&store, &c, "same").await;
    let s = store.commands();
    let (a, b) = tokio::join!(
        s.execute(TENANT, PROJECT, A, cmd.clone()),
        s.execute(TENANT, PROJECT, A, cmd.clone())
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a["receipt"], b["receipt"]);
    assert_ne!(a["replayed"], b["replayed"]);
    let mut changed = cmd;
    changed.args["input_digest"] = json!("b".repeat(64));
    assert!(matches!(
        s.execute(TENANT, PROJECT, A, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    let e = &a["receipt"]["data"];
    s.execute(TENANT, PROJECT, A, cancel(&store, e, "cancel").await)
        .await
        .unwrap();
    let first = intent(&store, &c, "next").await;
    let mut second = first.clone();
    second.request_id = "other".into();
    let (a, b) = tokio::join!(
        s.execute(TENANT, PROJECT, A, first),
        s.execute(TENANT, PROJECT, A, second)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(matches!(
        a.err().or(b.err()),
        Some(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        snapshot(&admin).await["execution"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn scope_and_version_guards_reject_forged_identity_without_writes() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let cmd = intent(&store, &c, "intent").await;
    let before = snapshot(&admin).await;
    for p in [
        "/src",
        "src/../outside",
        "src//file",
        "src/./file",
        "src\\file",
        "C:/src",
        "src\nfile",
    ] {
        let mut r = cmd.clone();
        r.args["declared_scope"] = json!([p]);
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, r).await,
            Err(PgError::Protocol(_))
        ));
    }
    for (field, v) in [
        ("actor_id", json!("reviewer")),
        ("fencing_class", json!("hard_fence")),
        ("receipt_kind", json!("trusted_executor")),
        ("input_digest", json!("invented")),
        ("expected_work_version", json!("01")),
        ("declared_scope", json!(vec!["src"; 129])),
    ] {
        let mut r = cmd.clone();
        r.args[field] = v;
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, r).await,
            Err(PgError::Protocol(_))
        ));
    }
    let mut outside = cmd.clone();
    outside.args["declared_scope"] = json!(["outside"]);
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, outside).await,
        Err(PgError::ScopeExceeded)
    ));
    for field in [
        "expected_work_version",
        "expected_session_version",
        "expected_lease_version",
    ] {
        let mut r = cmd.clone();
        r.args[field] = json!("9");
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, r).await,
            Err(PgError::PreconditionsChanged)
        ));
    }
    let mut r = cmd.clone();
    r.args["expected_fence"] = json!("9");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, r).await,
        Err(PgError::StaleFence)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, cmd).await,
        Err(PgError::LeaseExpired)
    ));
}

#[tokio::test]
async fn same_actor_other_client_may_read_granted_metadata_but_cannot_cancel() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let cmd = intent(&store, &c, "intent").await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    let e = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let mut q = query("execution.inspect");
    q.work_id = Some("a".into());
    q.execution_id = Some(e["execution_id"].as_str().unwrap().into());
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q.clone()).await,
        Err(PgError::Forbidden)
    ));
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'agent','cli-b',$3,1,true,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    assert_eq!(
        store.query(TENANT, PROJECT, B, q.clone()).await.unwrap()["data"]["owned_by_client"],
        false
    );
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, cancel(&store, &e, "forged").await)
            .await,
        Err(PgError::Forbidden)
    ));
    for id in ["hidden", e["execution_id"].as_str().unwrap()] {
        q.work_id = Some("b-private".into());
        q.execution_id = Some(id.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, B, q.clone()).await,
            Err(PgError::Forbidden)
        ));
    }
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn cancel_of_exposed_or_unknown_execution_is_only_a_request_and_keeps_resources() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let mut e = store
        .commands()
        .execute(TENANT, PROJECT, A, intent(&store, &c, "prepare").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    admin.execute("INSERT INTO awr_team.outbox(tenant_id,project_id,id,state,payload_json,action_kind,aggregate_id)
        VALUES($1,$2,'delivery','sending','{}','execution.dispatch',$3)",&[&TENANT,&PROJECT,&e["execution_id"].as_str().unwrap()]).await.unwrap();
    admin.batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=true;
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','resource','a','named','shared','unknown')").await.unwrap();
    for state in ["prepared", "queued", "accepted", "running", "unknown"] {
        admin
            .execute("UPDATE awr_team.executions SET state=$1", &[&state])
            .await
            .unwrap();
        let result = store
            .commands()
            .execute(TENANT, PROJECT, A, cancel(&store, &e, state).await)
            .await
            .unwrap();
        let data = &result["receipt"]["data"];
        assert_eq!(data["state"], state);
        assert_eq!(data["stop_confirmed"], false);
        e["execution_version"] = data["execution_version"].clone();
        let snap = snapshot(&admin).await;
        assert_eq!(snap["outbox"][0]["state"], "sending");
        assert_eq!(snap["resources"][0]["state"], "unknown");
        assert_eq!(snap["work"][0]["recovery_blocked"], true);
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, A, intent(&store, &c, "retry").await)
                .await,
            Err(PgError::RecoveryBlocked)
        ));
    }
}

#[tokio::test]
async fn epoch_and_ownership_changes_do_not_adopt_old_execution_records() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let e = store
        .commands()
        .execute(TENANT, PROJECT, A, intent(&store, &c, "prepare").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET coordinator_epoch='new' WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    assert_eq!(inspect(&store, &e).await["epoch_matches_current"], false);
    assert_eq!(inspect(&store, &e).await["lease_live"], false);
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cancel(&store, &e, "new-epoch").await)
            .await,
        Err(PgError::EpochChanged)
    ));
    admin.batch_execute("UPDATE awr_team.projects SET coordinator_epoch='epoch-a' WHERE tenant_id='reader-tenant';
        UPDATE awr_team.executions SET ownership_version=2").await.unwrap();
    let mut q = query("execution.inspect");
    q.work_id = Some("a".into());
    q.execution_id = Some(e["execution_id"].as_str().unwrap().into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                cancel(&store, &e, "other-generation").await
            )
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn failed_events_roll_back_intent_and_cancel_with_revision_and_receipt() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let cmd = intent(&store, &c, "prepare").await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_execution_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type LIKE 'execution.%' THEN RAISE EXCEPTION 'synthetic failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER reject_execution_event BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_execution_event()").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute("ALTER TABLE awr_team.events DISABLE TRIGGER reject_execution_event")
        .await
        .unwrap();
    let e = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let cmd = cancel(&store, &e, "cancel").await;
    admin
        .batch_execute("ALTER TABLE awr_team.events ENABLE TRIGGER reject_execution_event")
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute("DROP TRIGGER reject_execution_event ON awr_team.events")
        .await
        .unwrap();
    store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
}

#[tokio::test]
async fn revoked_grant_blocks_waiting_preparation_before_any_intent_is_written() {
    let (_g, mut admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let cmd = intent(&store, &c, "prepare").await;
    let before = snapshot(&admin).await;
    let observer = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let revoke = admin.transaction().await.unwrap();
    revoke.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    let commands = store.commands();
    let write = tokio::spawn(async move { commands.execute(TENANT, PROJECT, A, cmd).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = observer
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND query LIKE '%ORDER BY workstream_id FOR SHARE%')",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("writer must wait on the live grant row");
    revoke.commit().await.unwrap();
    assert!(matches!(write.await.unwrap(), Err(PgError::Forbidden)));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn barriers_and_active_contract_checks_preserve_recovery_only_cancellation() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    admin
        .batch_execute(
            "UPDATE awr_team.work_contracts SET definition_state='archived' WHERE work_id='a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, intent(&store, &c, "archived").await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.work_contracts SET definition_state='enabled' WHERE work_id='a';
        INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state)
        VALUES('reader-tenant','reader-project','wait','session-a','a','Confirm the plan?','open')",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, intent(&store, &c, "waiting").await)
            .await,
        Err(PgError::WaitOpen)
    ));
    admin
        .batch_execute("UPDATE awr_team.wait_items SET state='replied'")
        .await
        .unwrap();
    let e = store
        .commands()
        .execute(TENANT, PROJECT, A, intent(&store, &c, "prepared").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET status='frozen' WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cancel(&store, &e, "frozen").await)
            .await,
        Err(PgError::ProjectNotAvailable)
    ));
    assert_eq!(inspect(&store, &e).await["state"], "prepared");
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET status='active' WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    let mut catalog: awr_core::WorkstreamCatalog = serde_json::from_value(
        admin
            .query_one(
                "SELECT catalog_json FROM awr_team.workstream_catalogs WHERE tenant_id=$1",
                &[&TENANT],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    catalog
        .workstreams
        .iter_mut()
        .find(|s| s.id == awr_core::Id::from(1))
        .unwrap()
        .state = awr_core::WorkstreamState::Paused;
    admin
        .execute(
            "UPDATE awr_team.workstream_catalogs SET catalog_json=$1 WHERE tenant_id=$2",
            &[&json!(catalog), &TENANT],
        )
        .await
        .unwrap();
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            cancel(&store, &e, "paused-cancel").await,
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                intent(&store, &c, "paused-prepare").await
            )
            .await,
        Err(PgError::Workstream(awr_core::WorkstreamError::Inactive))
    ));
}

#[tokio::test]
async fn schema_twelve_preserves_legacy_executions_and_failed_migration_is_atomic() {
    let (_g, admin, _) = common::fresh_team_schema().await;
    // Reconstruct the historical schema from its migrations, not a partial
    // downgrade of the current schema that leaves later tables and constraints.
    admin
        .batch_execute("DROP SCHEMA awr_team CASCADE")
        .await
        .unwrap();
    for ddl in [
        include_str!("../migrations/20260917000001_init.sql"),
        include_str!("../migrations/20260918000002_session_wait.sql"),
        include_str!("../migrations/20260918000003_graph_resources.sql"),
        include_str!("../migrations/20260918000004_execution_protocol.sql"),
        include_str!("../migrations/20260918000005_review_completion.sql"),
        include_str!("../migrations/20260918000006_import_restore.sql"),
        include_str!("../migrations/20260919000007_completion_integrity.sql"),
        include_str!("../migrations/20260920000008_execution_result_binding.sql"),
        include_str!("../migrations/20260920000009_import_integrity.sql"),
        include_str!("../migrations/20260921000010_workstreams.sql"),
        include_str!("../migrations/20260921000011_workstream_claims.sql"),
    ] {
        admin.batch_execute(ddl).await.unwrap();
    }
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('reader-tenant','Readers','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status) VALUES('reader-tenant','reader-project','p','team','epoch-a','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES('reader-tenant','reader-project','a','a');
        INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
        VALUES('reader-tenant','reader-project','legacy','a',4,'old-contract','old-runner','unknown')").await.unwrap();
    let ddl = include_str!("../migrations/20260921000012_workstream_executions.sql");
    assert!(
        admin
            .batch_execute(&ddl.replace(
                "UPDATE awr_team.schema_state",
                "SELECT 1/0; UPDATE awr_team.schema_state"
            ))
            .await
            .is_err()
    );
    admin.batch_execute("ROLLBACK").await.unwrap();
    let row=admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),(SELECT count(*) FROM information_schema.columns
        WHERE table_schema='awr_team' AND table_name='executions' AND column_name='workstream_id')",&[]).await.unwrap();
    assert_eq!(row.get::<_, i32>(0), 11);
    assert_eq!(row.get::<_, i64>(1), 0);
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::migrate(&admin).await.unwrap();
    let r=admin.query_one("SELECT state,fence,scope_id,workstream_id,ownership_version,executor_client_id,execution_version FROM awr_team.executions",&[]).await.unwrap();
    assert_eq!(r.get::<_, String>(0), "unknown");
    assert_eq!(r.get::<_, i64>(1), 4);
    assert_eq!(r.get::<_, String>(2), "main");
    assert_eq!(r.get::<_, Option<String>>(3), None);
    assert_eq!(r.get::<_, Option<i64>>(4), None);
    assert_eq!(r.get::<_, Option<String>>(5), None);
    assert_eq!(r.get::<_, i64>(6), 1);
    assert!(
        admin
            .batch_execute("UPDATE awr_team.executions SET ownership_version=1")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn planning_change_blocks_execution_start_until_confirmed() {
    let (_g, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let inv = SelectiveInvalidationStore::from_config(with_app_role(&test_config(), &db));

    let (c, e) = ready_intent(&store).await;
    assert!(
        inv.action_blocked(TENANT, PROJECT, "a")
            .await
            .unwrap()
            .is_none()
    );

    inv.record_planning_change(
        TENANT,
        PROJECT,
        &RecordPlanningChangeRequest {
            request_key: "plan-block-a".into(),
            change_id: "chg-a".into(),
            discovered_by: "agent".into(),
            old_graph_version: "g0".into(),
            new_graph_version: "g1".into(),
            old_acceptance_contract: "acc0".into(),
            new_acceptance_contract: "acc1".into(),
            affected_work_ids: vec!["a".into()],
            cancel_split_relations: vec![],
            continue_conditions: vec!["human_confirm".into()],
            all_project_work_ids: vec!["a".into(), "b-private".into(), "c".into()],
            now_ms: 50,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        inv.action_blocked(TENANT, PROJECT, "a")
            .await
            .unwrap()
            .as_deref(),
        Some("chg-a")
    );
    // Unrelated work remains unblocked.
    assert!(
        inv.action_blocked(TENANT, PROJECT, "c")
            .await
            .unwrap()
            .is_none()
    );

    let err = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c, &e, "start-blocked").await,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::ActionBlockedByInvalidation(ref id) if id == "chg-a"),
        "unexpected error: {err}"
    );

    inv.confirm_planning_change(
        TENANT,
        PROJECT,
        &awr_team_pg::DecidePlanningChangeRequest {
            request_key: "plan-confirm-a".into(),
            change_id: "chg-a".into(),
            actor_id: "reviewer".into(),
            now_ms: 60,
        },
    )
    .await
    .unwrap();
    assert!(
        inv.action_blocked(TENANT, PROJECT, "a")
            .await
            .unwrap()
            .is_none()
    );

    // After confirmation, the previously prepared intent may start.
    let started = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c, &e, "start-after-confirm").await,
        )
        .await
        .unwrap();
    assert_eq!(started["execution_authorized"], true);
    assert_eq!(started["receipt"]["data"]["admission"], "granted_at_commit");
}

#[tokio::test]
async fn planning_change_blocks_execution_prepare_for_affected_work() {
    let (_g, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let inv = SelectiveInvalidationStore::from_config(with_app_role(&test_config(), &db));
    inv.record_planning_change(
        TENANT,
        PROJECT,
        &RecordPlanningChangeRequest {
            request_key: "plan-prep-block".into(),
            change_id: "chg-prep".into(),
            discovered_by: "agent".into(),
            old_graph_version: "g0".into(),
            new_graph_version: "g1".into(),
            old_acceptance_contract: "acc0".into(),
            new_acceptance_contract: "acc1".into(),
            affected_work_ids: vec!["a".into()],
            cancel_split_relations: vec![],
            continue_conditions: vec!["human_confirm".into()],
            all_project_work_ids: vec!["a".into(), "b-private".into(), "c".into()],
            now_ms: 50,
        },
    )
    .await
    .unwrap();
    let c = claim(&store).await;
    let err = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            intent(&store, &c, "prepare-blocked").await,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::ActionBlockedByInvalidation(ref id) if id == "chg-prep"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn planning_change_blocks_work_complete_for_affected_work() {
    let (_g, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let inv = SelectiveInvalidationStore::from_config(with_app_role(&test_config(), &db));
    let _claim = claim(&store).await;
    inv.record_planning_change(
        TENANT,
        PROJECT,
        &RecordPlanningChangeRequest {
            request_key: "plan-complete-block".into(),
            change_id: "chg-complete".into(),
            discovered_by: "agent".into(),
            old_graph_version: "g0".into(),
            new_graph_version: "g1".into(),
            old_acceptance_contract: "acc0".into(),
            new_acceptance_contract: "acc1".into(),
            affected_work_ids: vec!["a".into()],
            cancel_split_relations: vec![],
            continue_conditions: vec!["human_confirm".into()],
            all_project_work_ids: vec!["a".into(), "b-private".into(), "c".into()],
            now_ms: 70,
        },
    )
    .await
    .unwrap();
    let prepared = prepare(&store, A, "a").await;
    let err = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "complete-blocked",
                "work.complete",
                json!({
                    "session_id": "session-a",
                    "expected_session_version": "1",
                    "evidence_id": "evidence-blocked",
                    "context_complete": true
                }),
            ),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::ActionBlockedByInvalidation(ref id) if id == "chg-complete"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn current_contract_invalidation_blocks_prepare_until_rebound() {
    let (_g, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.dependency_bindings(
                tenant_id, project_id, downstream_work_id, upstream_work_id, binding_hash, valid)
             VALUES ('reader-tenant','reader-project','a','b-private','bind-current', true)",
        )
        .await
        .unwrap();
    let c = claim(&store).await;
    let inv = SelectiveInvalidationStore::from_config(with_app_role(&test_config(), &db));
    let (plan, _) = inv
        .apply_selective_invalidation(
            TENANT,
            PROJECT,
            &SelectiveInvalidateRequest {
                request_key: "inv-prepare-block".into(),
                event_id: "evt-prepare-block".into(),
                provider_work_id: "b-private".into(),
                change: ProviderChangeKind::NewVersionOrProgress,
                consumers: vec![AdoptedConsumerEdge {
                    dependency_id: "dep-current".into(),
                    consumer_work_id: "a".into(),
                    provider_work_id: "b-private".into(),
                    policy: "current_contract".into(),
                    credential_status: "active".into(),
                    assessment_status: "satisfied".into(),
                }],
                all_project_work_ids: vec!["a".into(), "b-private".into(), "c".into()],
                now_ms: 80,
            },
        )
        .await
        .unwrap();
    assert_eq!(plan.reevaluate, vec!["a".to_string()]);
    let still_valid: bool = admin
        .query_one(
            "SELECT valid FROM awr_team.dependency_bindings
             WHERE tenant_id=$1 AND project_id=$2
               AND downstream_work_id='a' AND upstream_work_id='b-private'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!still_valid);

    let err = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            intent(&store, &c, "prepare-invalid-binding").await,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::ActionBlockedByInvalidation(ref id) if id == "invalid_dependency_binding"),
        "unexpected error: {err}"
    );
    let executions: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.executions
             WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(executions, 0);

    admin
        .batch_execute(
            "UPDATE awr_team.dependency_bindings SET valid=true
             WHERE tenant_id='reader-tenant' AND project_id='reader-project'
               AND downstream_work_id='a' AND upstream_work_id='b-private'",
        )
        .await
        .unwrap();
    let prepared = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            intent(&store, &c, "prepare-after-rebind").await,
        )
        .await
        .unwrap();
    assert_eq!(prepared["receipt"]["data"]["state"], "prepared");
}
