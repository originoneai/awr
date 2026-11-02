#![cfg(feature = "pg-tests")]
//! Deterministic state-machine check of one complete Team delivery order, driven through the
//! command handlers with member bearer tokens. No model, native client or network is involved:
//! the test pins which command the service accepts or refuses in which state, not what a member
//! would decide to do. Repository effects run against a synthetic local Git repository.
//!
//! The order: a manager assigns one item and leaves the other in the pool, two developers
//! implement and refine their items twice before any review decision, an independent reviewer
//! decides (and returns one item once), the manager delivers the first item, the second item's
//! stale base is rejected by the repository connector, its author rebuilds on the new target and
//! the follow-up is delivered. Afterwards the finalized items accept no follow-up work.
//!
//! This pins the service state machine and the order of commands. It does not pin what a model
//! or an operator does: whether a developer fetches the current target before building, which
//! tool a member picks, or whether a member asks the user a question.
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod access;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "fixtures/local_git.rs"]
mod git;

use access::{PROJECT, TENANT, command, prepare};
use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, Id, IssueAuthorizationRequest, PersonId, Workstream, WorkstreamCatalog,
    WorkstreamState,
};
use awr_server::delivery_adapter::{
    LocalGitAdapter, LocalGitConfig, LocalGitIntegrationConfig, LocalGitIntegrationPollRequest,
    LocalGitIntegrationSnapshot, LocalGitIntegrator,
};
use awr_team::delivery::*;
use awr_team::{
    ExecutionSettlementMode, ExecutionSettlementPolicy, OrdinaryPlanningSelfApprovePolicy,
    SourceActivationPlan, WorkContract, WorkId, WorkstreamBundle, WorkstreamContract,
};
use awr_team_pg::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, collections::BTreeSet, sync::MutexGuard};
use tokio_postgres::Client;

/// Assert that a command was refused with one named reason; `what` names the attempted step.
macro_rules! refused {
    ($result:expr, $reason:pat, $($what:tt)+) => {{
        let result: PgResult<Value> = $result;
        assert!(
            matches!(result, Err($reason)),
            "{}: expected {}, got {}",
            format!($($what)+),
            stringify!($reason),
            match &result {
                Ok(_) => "an accepted command".to_owned(),
                Err(error) => format!("{error:?}"),
            }
        );
    }};
}

const RESOURCE: &str = "fixture://team-repository";
const POLICY: &str = ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY;

#[derive(Clone, Copy)]
struct Member {
    /// The person a member acts for; assignments name this identity.
    person: &'static str,
    /// The authenticated agent that holds the credential.
    agent: &'static str,
    client: &'static str,
    token: &'static str,
}

const MANAGER: Member = Member {
    person: "manager",
    agent: "manager-agent",
    client: "cli-manager",
    token: "awr1.team-manager.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
};
const LIN: Member = Member {
    person: "lin",
    agent: "lin-agent",
    client: "cli-lin",
    token: "awr1.team-lin.1111111111111111111111111111111111111111111111111111111111111111",
};
const MEI: Member = Member {
    person: "mei",
    agent: "mei-agent",
    client: "cli-mei",
    token: "awr1.team-mei.2222222222222222222222222222222222222222222222222222222222222222",
};
const CHEN: Member = Member {
    person: "chen",
    agent: "chen-agent",
    client: "cli-chen",
    token: "awr1.team-chen.3333333333333333333333333333333333333333333333333333333333333333",
};
/// The repository connector's service principal.
const WORKER: Member = Member {
    person: "integrator",
    agent: "integrator",
    client: "cli-worker",
    token: "awr1.team-worker.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
};

#[derive(Clone, Copy)]
struct Work {
    id: &'static str,
    /// The single deliverable file of every candidate version.
    path: &'static str,
    scope: &'static str,
    workspace: &'static str,
    connector: &'static str,
}

const API: Work = Work {
    id: "issue-filter-api",
    path: "src/api/filter.js",
    scope: "src/api",
    workspace: "workspace-api",
    connector: "repository-api",
};
const PAGE: Work = Work {
    id: "issue-filter-page",
    path: "public/app.js",
    scope: "public",
    workspace: "workspace-page",
    connector: "repository-page",
};

fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn stream() -> String {
    Id::from(1).to_string()
}

/// One candidate version of one work, as the author submitted it for review.
#[derive(Clone)]
struct Version {
    number: u32,
    source: String,
    digest: String,
    selection: String,
    evidence_id: String,
    round_id: String,
}

struct Team {
    _guard: MutexGuard<'static, ()>,
    admin: Client,
    reads: WorkstreamReadStore,
    store: DeliverySyncStore,
    source: SourceStore,
    repo: git::GitFixture,
    requests: u32,
    sessions: BTreeMap<(&'static str, &'static str), String>,
    claims: BTreeMap<&'static str, Value>,
}

/// The work contract of one item. The bootstrap form (`delivery == false`) uses the codec the
/// workstream bundle accepts; the delivery form adds explicit workspace settlement and the
/// simulated member review policy that a source-first deployment declares.
fn contract(work: Work, delivery: bool) -> WorkContract {
    WorkContract {
        dependency_acceptance: Default::default(),
        execution_settlement: delivery.then(|| ExecutionSettlementPolicy {
            mode: ExecutionSettlementMode::IndependentWorkspaceV1,
            workspace_id: work.workspace.into(),
        }),
        codec: if delivery {
            WorkContract::CODEC_V4
        } else {
            WorkContract::CODEC
        }
        .into(),
        work_id: WorkId::new(work.id).unwrap(),
        external_key: work.id.into(),
        goals: vec!["support".into()],
        hard_rules: vec!["preserve the recorded issues".into()],
        scope_paths: vec![work.scope.into()],
        acceptance: vec!["filters combine and the result is verified".into()],
        required_dependencies: vec![],
        completion_policy: if delivery { POLICY } else { "review" }.into(),
        verification_requirements: vec![
            if delivery {
                "local_git.manifest"
            } else {
                "report"
            }
            .into(),
        ],
    }
}

fn read_set(prepared: &Value) -> DeliveryReadSet {
    serde_json::from_value(json!({"work_id":prepared["data"]["work_id"],
        "workstream_id":prepared["workstream_id"],"coordinator_epoch":prepared["coordinator_epoch"],
        "source_snapshot_id":prepared["source_snapshot_id"],"authority_version":prepared["authority_version"],
        "ownership_version":prepared["data"]["ownership_version"],
        "contract_hash":prepared["data"]["contract_hash"]}))
    .unwrap()
}

impl Team {
    /// Project, members, activated source with two works, connectors and a synthetic repository.
    #[expect(
        clippy::await_holding_lock,
        reason = "The guard serializes the shared PostgreSQL fixture for the whole scenario."
    )]
    async fn start() -> Self {
        let (guard, admin, db) = common::fresh_team_schema().await;
        admin
            .batch_execute(
                "INSERT INTO awr_team.tenants(id,name,status) VALUES('reader-tenant','Team','active');
                 INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
                   ('reader-tenant','source-author','human','Source author','active'),
                   ('reader-tenant','source-reviewer','human','Source reviewer','active'),
                   ('reader-tenant','integrator','system','Integrator','active'),
                   ('reader-tenant','manager','agent','Manager','active'),
                   ('reader-tenant','lin','agent','Lin','active'),
                   ('reader-tenant','mei','agent','Mei','active'),
                   ('reader-tenant','chen','agent','Chen','active'),
                   ('reader-tenant','manager-agent','agent','Manager agent','active'),
                   ('reader-tenant','lin-agent','agent','Lin agent','active'),
                   ('reader-tenant','mei-agent','agent','Mei agent','active'),
                   ('reader-tenant','chen-agent','agent','Chen agent','active');
                 INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                   VALUES('reader-tenant','reader-project','p','team','epoch-a','active');
                 INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
                   ('reader-tenant','reader-project','source-author','admin'),
                   ('reader-tenant','reader-project','source-reviewer','reviewer'),
                   ('reader-tenant','reader-project','integrator','admin'),
                   ('reader-tenant','reader-project','manager','developer'),
                   ('reader-tenant','reader-project','lin','developer'),
                   ('reader-tenant','reader-project','mei','developer'),
                   ('reader-tenant','reader-project','chen','developer');
                 INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role,business_roles,assignment_grant,agent_review) VALUES
                   ('reader-tenant','reader-project','manager-agent','maintainer','[\"supervisor\",\"deliverer\"]',true,false),
                   ('reader-tenant','reader-project','lin-agent','developer','[\"developer\"]',false,false),
                   ('reader-tenant','reader-project','mei-agent','developer','[\"developer\"]',false,false),
                   ('reader-tenant','reader-project','chen-agent','developer','[\"reviewer\"]',false,true);
                 INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                   VALUES('reader-tenant','reader-project','main','Main','active')",
            )
            .await
            .unwrap();
        let config = common::with_app_role(&common::test_config(), &db);
        // One activated source with both works in one workstream.
        let bundle = WorkstreamBundle {
            codec: WorkstreamBundle::CODEC.into(),
            catalog: WorkstreamCatalog {
                version: 1,
                project_id: PROJECT.into(),
                legacy_default: None,
                workstreams: vec![Workstream {
                    id: Id::from(1),
                    project_id: PROJECT.into(),
                    external_key: "support".into(),
                    title: "support".into(),
                    state: WorkstreamState::Active,
                    authority_version: 1,
                    goal_keys: vec!["support".into()],
                    acceptance_contracts: vec![],
                }],
            },
            contracts: [API, PAGE]
                .into_iter()
                .map(|work| WorkstreamContract {
                    workstream_id: Id::from(1),
                    contract: contract(work, false),
                })
                .collect(),
        };
        let source = SourceStore::from_config(config.clone());
        let candidate = source
            .ingest(IngestRequest {
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
                actor_id: "source-author".into(),
                parser_version: "workstreams/1".into(),
                files: vec![SourceFile {
                    path: "workstreams.json".into(),
                    bytes: serde_json::to_vec(&bundle).unwrap(),
                }],
            })
            .await
            .unwrap();
        source
            .approve(
                TENANT,
                PROJECT,
                &candidate.proposal_id,
                "source-reviewer",
                &candidate.manifest_digest,
            )
            .await
            .unwrap();
        source
            .activate_workstreams(
                TENANT,
                PROJECT,
                "source-author",
                &candidate.proposal_id,
                &SourceActivationPlan {
                    candidate_digest: candidate.manifest_digest.clone(),
                    parser_version: candidate.parser_version.clone(),
                    expected_authority_epoch: candidate.base_epoch.clone(),
                    approved_candidate_digest: candidate.manifest_digest.clone(),
                },
            )
            .await
            .unwrap();
        // The bootstrap codec carries no settlement fields; install the delivery contracts.
        for work in [API, PAGE] {
            let delivery = contract(work, true);
            admin
                .execute(
                    "UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id=$3",
                    &[&json!(delivery), &delivery.hash().unwrap(), &work.id],
                )
                .await
                .unwrap();
        }
        let members = [MANAGER, LIN, MEI, CHEN];
        for member in members {
            admin
                .execute(
                    "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity)
                     VALUES($1,$2,$3,$3,'active','{\"kind\":\"simulated_member\",\"controller_ref\":\"shared-test-controller\"}')",
                    &[&TENANT, &PROJECT, &member.person],
                )
                .await
                .unwrap();
            admin
                .execute(
                    "INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
                     VALUES($1,$2,$3,$4,$5,'active')",
                    &[&TENANT, &PROJECT, &format!("bind-{}", member.agent), &member.person, &member.agent],
                )
                .await
                .unwrap();
        }
        for member in members.into_iter().chain([WORKER]) {
            admin
                .execute(
                    "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,$2,$3,$4,$5)",
                    &[
                        &TENANT,
                        &member.token.split('.').nth(1).unwrap(),
                        &member.agent,
                        &member.client,
                        &workstream_credential_hash(member.token).unwrap(),
                    ],
                )
                .await
                .unwrap();
            admin
                .execute(
                    "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage)
                     VALUES($1,$2,$3,$4,$5,1,true,true,$6)",
                    &[&TENANT, &PROJECT, &member.agent, &member.client, &stream(), &(member.agent == "integrator")],
                )
                .await
                .unwrap();
        }
        let authorizations = AuthorizationStore::from_config(config.clone());
        for (member, actions) in [
            (
                MANAGER,
                vec![
                    AuthorizedAction::Inspect,
                    AuthorizedAction::AssignWork,
                    AuthorizedAction::FinalizeDelivery,
                ],
            ),
            (
                LIN,
                vec![
                    AuthorizedAction::Inspect,
                    AuthorizedAction::StartWork,
                    AuthorizedAction::ClaimCoordination,
                ],
            ),
            (
                MEI,
                vec![
                    AuthorizedAction::Inspect,
                    AuthorizedAction::StartWork,
                    AuthorizedAction::ClaimCoordination,
                ],
            ),
            (
                CHEN,
                vec![
                    AuthorizedAction::Inspect,
                    AuthorizedAction::StartWork,
                    AuthorizedAction::Review,
                ],
            ),
        ] {
            authorizations
                .issue(
                    TENANT,
                    PROJECT,
                    &IssueAuthorizationRequest {
                        request_key: format!("grant-{}", member.agent),
                        authorization: AgentAuthorization {
                            id: format!("grant-{}", member.agent),
                            authorizer_person_id: PersonId::new(member.person).unwrap(),
                            responsible_person_id: PersonId::new(member.person).unwrap(),
                            subject_kind: ExecutionSubjectKind::Agent,
                            subject_id: member.agent.into(),
                            client_id: member.client.into(),
                            session_id: None,
                            model_id: None,
                            scope: AuthorizationScope::Workstream {
                                project_id: PROJECT.into(),
                                workstream_id: stream(),
                            },
                            actions: actions.into_iter().collect::<BTreeSet<_>>(),
                            expires_at_ms: None,
                            status: AuthorizationStatus::Active,
                            revoked_at_ms: None,
                            revoked_by: None,
                            verifiable_capabilities: vec![],
                            self_reported_skill_hints: vec![],
                            parent_authorization_id: None,
                            maintainer_person_id: None,
                            created_at_ms: 1000,
                            binding_id: Some(format!("bind-{}", member.agent)),
                        },
                    },
                )
                .await
                .unwrap();
        }
        let mut repo = git::GitFixture::new(false);
        repo.config.tenant_id = TENANT.into();
        repo.config.project_id = PROJECT.into();
        repo.config.resource = RESOURCE.into();
        let team = Self {
            _guard: guard,
            admin,
            reads: WorkstreamReadStore::from_config(config.clone()),
            store: DeliverySyncStore::from_config(config),
            source,
            repo,
            requests: 0,
            sessions: BTreeMap::new(),
            claims: BTreeMap::new(),
        };
        for work in [API, PAGE] {
            let p = prepare(&team.reads, MANAGER.token, work.id).await;
            team.store
                .configure_connector(
                    TENANT,
                    PROJECT,
                    WORKER.token,
                    ConfigureDeliveryConnector {
                        request_id: format!("configure-{}", work.id),
                        read_set: read_set(&p),
                        expected_connector_version: "0".into(),
                        mapping: DeliveryConnectorMapping {
                            connector_id: work.connector.into(),
                            provider: "local_git".into(),
                            resource: RESOURCE.into(),
                            principal_actor_id: "integrator".into(),
                            principal_client_id: "cli-worker".into(),
                            fact_source: FactSource::AdapterObservation,
                            enabled: true,
                            source_locator: None,
                        },
                    },
                )
                .await
                .unwrap();
        }
        team
    }

    fn next_key(&mut self, label: &str) -> String {
        self.requests += 1;
        format!("{label}-{}", self.requests)
    }

    /// One command by one member against the freshly prepared work. Refusals are returned.
    async fn attempt(&mut self, who: Member, work: Work, op: &str, args: Value) -> PgResult<Value> {
        let prepared = prepare(&self.reads, who.token, work.id).await;
        let key = self.next_key(op);
        self.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                who.token,
                command(&prepared, &key, op, args),
            )
            .await
    }

    /// Like `attempt`, but returns the receipt data of an accepted command.
    async fn try_run(&mut self, who: Member, work: Work, op: &str, args: Value) -> PgResult<Value> {
        self.attempt(who, work, op, args)
            .await
            .map(|done| done["receipt"]["data"].clone())
    }

    async fn run(&mut self, who: Member, work: Work, op: &str, args: Value) -> Value {
        match self.try_run(who, work, op, args).await {
            Ok(data) => data,
            Err(error) => panic!(
                "{op} by {} on {} was refused: {error:?}",
                who.person, work.id
            ),
        }
    }

    async fn session(&mut self, who: Member, work: Work) -> Value {
        let id = match self.sessions.get(&(who.person, work.id)) {
            Some(id) => id.clone(),
            None => {
                let started = self
                    .run(
                        who,
                        work,
                        "session.start",
                        json!({"conversation_id":format!("{}-{}", who.person, work.id)}),
                    )
                    .await;
                let id = started["session_id"].as_str().unwrap().to_owned();
                self.sessions.insert((who.person, work.id), id.clone());
                id
            }
        };
        let version: i64 = self
            .admin
            .query_one(
                "SELECT session_version FROM awr_team.sessions WHERE id=$1",
                &[&id],
            )
            .await
            .unwrap()
            .get(0);
        json!({"session_id":id,"expected_session_version":version.to_string()})
    }

    /// Merge the session header into a command's arguments.
    async fn with_session(&mut self, who: Member, work: Work, mut args: Value) -> Value {
        let session = self.session(who, work).await;
        args.as_object_mut()
            .unwrap()
            .extend(session.as_object().unwrap().clone());
        args
    }

    fn claim_header(&self, work: Work) -> Value {
        let claim = &self.claims[work.id];
        json!({"claim_id":claim["claim_id"],"expected_fence":claim["fence"],
            "expected_lease_version":claim["lease_version"]})
    }

    async fn take(&mut self, who: Member, work: Work, accept: bool) -> PgResult<Value> {
        let p = prepare(&self.reads, who.token, work.id).await;
        let mut args = json!({
            "expected_responsibility_version":p["data"]["responsibility"]["version"],
            "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
            "ttl_seconds":3600});
        if accept {
            args["assignment_request_key"] =
                p["data"]["responsibility"]["pending"]["transfer_request_key"].clone();
        }
        let args = self.with_session(who, work, args).await;
        let op = if accept {
            "task.accept_assignment"
        } else {
            "task.claim_available"
        };
        let done = self.attempt(who, work, op, args).await?;
        let claim = done["receipt"]["data"].clone();
        self.claims.insert(work.id, claim.clone());
        Ok(claim)
    }

    /// Commit `content` on top of `base` and publish it as the author's branch.
    fn publish(&self, work: Work, number: u32, content: &str, base: &str) -> String {
        self.repo.git(&["checkout", "--detach", base]);
        self.repo.commit(work.path, content.as_bytes());
        let head = self.repo.git(&["rev-parse", "HEAD"]);
        self.repo.git(&[
            "push",
            "--force",
            self.repo.config.repository.to_str().unwrap(),
            &format!("HEAD:refs/heads/{}-v{number}", work.id),
        ]);
        head
    }

    fn candidate(
        &self,
        work: Work,
        number: u32,
        source: &str,
        base: &str,
        content: &str,
        prepared: &Value,
    ) -> DeliveryCandidate {
        let revision =
            |value: &str| json!({"resource":RESOURCE,"format":self.repo.format,"value":value});
        let mut candidate: DeliveryCandidate = serde_json::from_value(json!({"binding":{
            "tenant_id":TENANT,"project_id":PROJECT,"scope_id":"main",
            "workstream_id":prepared["workstream_id"],"work_id":work.id,
            "candidate_id":format!("{}-v{number}", work.id),"candidate_version":number.to_string(),
            "contract_hash":prepared["data"]["contract_hash"],"manifest_digest":"0".repeat(64),
            "source_revision":revision(source),"required_checks":["local_git.manifest"],
            "target":{"resource":RESOURCE,"reference":"refs/heads/main",
                "precondition":{"kind":"exact","revision":revision(base)}}},
            "manifest":{"entries":[{"artifact_id":"deliverable","sha256":sha(content),
                "byte_length":content.len().to_string(),"locator":format!("git-blob:{}", work.path)}]}}))
        .unwrap();
        git::rebind(&mut candidate);
        candidate
    }

    /// One author cycle: build a version on `base`, settle the run, select the candidate,
    /// submit the evidence and open a review round for it.
    async fn implement(
        &mut self,
        author: Member,
        work: Work,
        number: u32,
        content: &str,
        base: &str,
    ) -> Version {
        self.implement_declaring(author, work, number, content, base, base)
            .await
    }

    /// Like `implement`, but the candidate may declare a target base other than the commit's parent.
    async fn implement_declaring(
        &mut self,
        author: Member,
        work: Work,
        number: u32,
        content: &str,
        parent: &str,
        base: &str,
    ) -> Version {
        let source = self.publish(work, number, content, parent);
        let prepared = prepare(&self.reads, author.token, work.id).await;
        let candidate = self.candidate(work, number, &source, base, content, &prepared);
        let digest = candidate.binding.digest().unwrap();
        let input = sha(&format!("{}-input-{number}", work.id));
        let output = sha(content);
        let claim = self.claim_header(work);
        let args = self
            .with_session(
                author,
                work,
                json!({"claim_id":claim["claim_id"],"expected_fence":claim["expected_fence"],
                    "expected_lease_version":claim["expected_lease_version"],
                    "expected_work_version":prepared["data"]["runtime"]["work_version"],
                    "input_digest":input,"declared_scope":[work.scope]}),
            )
            .await;
        let execution = self.run(author, work, "execution.prepare", args).await;
        let prepared = prepare(&self.reads, author.token, work.id).await;
        let args = self
            .with_session(
                author,
                work,
                json!({"execution_id":execution["execution_id"],
                    "expected_execution_version":execution["execution_version"],
                    "claim_id":claim["claim_id"],"expected_fence":claim["expected_fence"],
                    "expected_lease_version":claim["expected_lease_version"],
                    "expected_work_version":prepared["data"]["runtime"]["work_version"],
                    "execution_mode":"caller_managed"}),
            )
            .await;
        let started = self.run(author, work, "execution.start", args).await;
        let args = self
            .with_session(
                author,
                work,
                json!({"execution_id":execution["execution_id"],
                    "expected_execution_version":started["execution_version"],
                    "outcome":"succeeded","output_digest":output,
                    "observed_paths":[work.path],"note":format!("version {number} is built and tested"),
                    "workspace_settlement":{"workspace_id":work.workspace,"input_digest":input,
                        "environment_digest":"c".repeat(64),"claim_id":claim["claim_id"],
                        "expected_fence":claim["expected_fence"],
                        "expected_lease_version":claim["expected_lease_version"],
                        "executor_stopped":true,"no_external_effects":true}}),
            )
            .await;
        self.run(author, work, "execution.report", args).await;
        let previous = self
            .admin
            .query_opt(
                "SELECT binding_digest FROM awr_team.delivery_selections WHERE work_id=$1",
                &[&work.id],
            )
            .await
            .unwrap()
            .map(|row| row.get::<_, String>(0));
        let prepared = prepare(&self.reads, author.token, work.id).await;
        // Selection names the session but takes no session version.
        let session = self.session(author, work).await;
        let selected = self
            .run(
                author,
                work,
                "delivery.candidate.select",
                json!({"source_snapshot_id":prepared["source_snapshot_id"],
                    "session_id":session["session_id"],
                    "claim_id":claim["claim_id"],"fence":claim["expected_fence"],
                    "lease_version":claim["expected_lease_version"],
                    "expected_selected_digest":previous,"candidate":candidate}),
            )
            .await;
        let args = self
            .with_session(
                author,
                work,
                json!({"artifact_text":content,"input_digest":input,
                    "execution_id":execution["execution_id"],"dirty_tree":false,
                    "payload":{"passed":true,"output_digest":output,"delivery_candidate_digest":digest}}),
            )
            .await;
        let evidence = self.run(author, work, "evidence.submit", args).await;
        let args = self
            .with_session(author, work, json!({"evidence_id":evidence["evidence_id"]}))
            .await;
        let round = self
            .run(author, work, "delivery.submit_and_request_review", args)
            .await;
        Version {
            number,
            source,
            digest,
            selection: selected["selection_version"].as_str().unwrap().into(),
            evidence_id: evidence["evidence_id"].as_str().unwrap().into(),
            round_id: round["round_id"].as_str().unwrap().into(),
        }
    }

    fn git_config(&self, work: Work) -> LocalGitConfig {
        let mut config = self.repo.config.clone();
        config.work_id = work.id.into();
        config.connector_id = work.connector.into();
        config.adapter_id = format!("local-{}", work.id);
        config
    }

    /// The service-owned repository observation of the work's selected candidate.
    async fn observe(&self, work: Work) {
        LocalGitAdapter::open(self.git_config(work))
            .await
            .unwrap()
            .reconcile_current(&self.store, WORKER.token)
            .await
            .unwrap();
    }

    /// The reviewer's decision on one round; returns the decision receipt data.
    async fn decide(
        &mut self,
        reviewer: Member,
        work: Work,
        round_id: &str,
        decision: &str,
    ) -> PgResult<Value> {
        let args = self
            .with_session(
                reviewer,
                work,
                json!({"round_id":round_id,"decision":decision,
                    "reason":format!("{decision}: checked the exact candidate bytes")}),
            )
            .await;
        self.try_run(reviewer, work, "review.decide", args).await
    }

    async fn prepare_integration(
        &mut self,
        manager: Member,
        work: Work,
        version: &Version,
        decision_id: &str,
    ) -> PgResult<Value> {
        let prepared = prepare(&self.reads, manager.token, work.id).await;
        self.try_run(
            manager,
            work,
            "delivery.integration.prepare",
            json!({"source_snapshot_id":prepared["source_snapshot_id"],
                "connector_id":work.connector,"connector_version":"1",
                "candidate_digest":version.digest,"selection_version":version.selection,
                "evidence_id":version.evidence_id,"review_round_id":version.round_id,
                "review_decision_id":decision_id,"operation":"fast_forward"}),
        )
        .await
    }

    async fn integrator(&self, work: Work) -> LocalGitIntegrator {
        LocalGitIntegrator::open(LocalGitIntegrationConfig {
            enabled: true,
            repository: self.git_config(work),
        })
        .await
        .unwrap()
    }

    /// The connector worker leases a prepared integration and performs the repository effect.
    async fn dispatch(&mut self, work: Work, integration_id: &str) -> LocalGitIntegrationSnapshot {
        let prepared = prepare(&self.reads, MANAGER.token, work.id).await;
        let set = read_set(&prepared);
        let lease_key = self.next_key("lease");
        let lease = self
            .store
            .lease_integration(
                TENANT,
                PROJECT,
                WORKER.token,
                LeaseDeliveryIntegration {
                    request_id: lease_key,
                    read_set: set.clone(),
                    integration_id: integration_id.into(),
                    lease_seconds: 60,
                },
            )
            .await
            .unwrap();
        let dispatch_key = self.next_key("dispatch");
        self.integrator(work)
            .await
            .execute(
                &self.store,
                WORKER.token,
                DispatchDeliveryIntegration {
                    request_id: dispatch_key,
                    read_set: set,
                    integration_id: integration_id.into(),
                    lease_id: lease["data"]["lease_id"].as_str().unwrap().into(),
                },
            )
            .await
            .unwrap()
    }

    /// The connector worker observes the original attempt and confirms it in the service.
    async fn confirm(&mut self, work: Work, integration_id: &str) -> Value {
        let prepared = prepare(&self.reads, MANAGER.token, work.id).await;
        let key = self.next_key("confirm");
        self.integrator(work)
            .await
            .reconcile(
                &self.store,
                WORKER.token,
                LocalGitIntegrationPollRequest {
                    request_id: key,
                    integration_id: integration_id.into(),
                    read_set: read_set(&prepared),
                    connector_version: "1".into(),
                },
            )
            .await
            .unwrap()
    }

    async fn inspect_integration(
        &mut self,
        who: Member,
        work: Work,
        integration_id: &str,
    ) -> Value {
        let done = self
            .reads
            .query(
                TENANT,
                PROJECT,
                who.token,
                serde_json::from_value(
                    json!({"protocol_version":1,"op":"delivery.integration.inspect",
                    "work_id":work.id,"request_id":integration_id}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        done["data"].clone()
    }

    /// Select a candidate version that names `expected` as the version it replaces.
    async fn select_replacing(
        &mut self,
        author: Member,
        work: Work,
        number: u32,
        content: &str,
        base: &str,
        expected: Option<&str>,
    ) -> PgResult<Value> {
        let source = self.publish(work, number, content, base);
        let prepared = prepare(&self.reads, author.token, work.id).await;
        let candidate = self.candidate(work, number, &source, base, content, &prepared);
        let session = self.session(author, work).await;
        let claim = self.claim_header(work);
        self.try_run(
            author,
            work,
            "delivery.candidate.select",
            json!({"source_snapshot_id":prepared["source_snapshot_id"],
                "session_id":session["session_id"],"claim_id":claim["claim_id"],
                "fence":claim["expected_fence"],"lease_version":claim["expected_lease_version"],
                "expected_selected_digest":expected,"candidate":candidate}),
        )
        .await
    }

    async fn rework(&mut self, who: Member, work: Work, round_id: &str) -> PgResult<Value> {
        let args = self
            .with_session(
                who,
                work,
                json!({"round_id":round_id,"note":"address the finding"}),
            )
            .await;
        self.try_run(who, work, "work.rework", args).await
    }

    async fn acquire(&mut self, who: Member, work: Work) -> PgResult<Value> {
        let prepared = prepare(&self.reads, who.token, work.id).await;
        let args = self
            .with_session(
                who,
                work,
                json!({"expected_work_version":prepared["data"]["runtime"]["work_version"],
                    "ttl_seconds":3600}),
            )
            .await;
        self.try_run(who, work, "claim.acquire", args).await
    }

    /// Prepare (the service accepts the request) and dispatch (the repository effect) one delivery.
    async fn deliver(
        &mut self,
        work: Work,
        version: &Version,
        decision_id: &str,
    ) -> (String, LocalGitIntegrationSnapshot) {
        let prepared = self
            .prepare_integration(MANAGER, work, version, decision_id)
            .await
            .unwrap_or_else(|error| panic!("integration of {} was refused: {error:?}", work.id));
        let id = prepared["integration_id"].as_str().unwrap().to_owned();
        let snapshot = self.dispatch(work, &id).await;
        (id, snapshot)
    }

    async fn finalize(&mut self, work: Work, version: &Version) -> Value {
        let args = self
            .with_session(
                MANAGER,
                work,
                json!({"evidence_id":version.evidence_id,"context_complete":true}),
            )
            .await;
        self.run(MANAGER, work, "delivery.finalize", args).await
    }

    fn main_revision(&self) -> String {
        self.repo.bare(&["rev-parse", "refs/heads/main"])
    }

    fn file_on_main(&self, path: &str) -> String {
        self.repo
            .bare(&["show", &format!("refs/heads/main:{path}")])
    }

    async fn round_state(&self, round_id: &str) -> String {
        self.admin
            .query_one(
                "SELECT state FROM awr_team.review_rounds WHERE id=$1",
                &[&round_id],
            )
            .await
            .unwrap()
            .get(0)
    }

    async fn runtime_state(&self, work: Work) -> String {
        self.admin
            .query_one(
                "SELECT state FROM awr_team.work_runtime WHERE work_id=$1",
                &[&work.id],
            )
            .await
            .unwrap()
            .get(0)
    }
}

const API_VERSIONS: [&str; 3] = [
    "filter by status and keyword\n",
    "filter by status, keyword, priority and owner\n",
    "filter by status, keyword, priority, owner and a date range\n",
];
const PAGE_VERSIONS: [&str; 6] = [
    "page filters by status and keyword\n",
    "page filters by status, keyword, priority and owner\n",
    "page filters by status, keyword, priority, owner and a date range\n",
    "page filters by status, keyword, priority, owner and a date range, boundary fixed\n",
    "page republished with the new target declared\n",
    "page rebuilt on the delivered api\n",
];

/// The versions each item went through before it was delivered, in submission order.
#[derive(Default)]
struct Order {
    api: Vec<Version>,
    page: Vec<Version>,
}

/// The manager arranges the work: one item is assigned, the other stays in the pool.
async fn arrange(team: &mut Team) {
    let offered = prepare(&team.reads, MANAGER.token, API.id).await;
    assert_eq!(
        offered["data"]["guidance"]["code"], "assignment",
        "the manager is offered the unassigned item"
    );
    let assign = json!({"assignee_person_id":"lin","expected_responsibility_version":"0"});
    refused!(
        team.try_run(LIN, API, "task.assign", assign.clone()).await,
        PgError::Forbidden,
        "a developer without an assignment grant assigns work"
    );
    let assigned = team.run(MANAGER, API, "task.assign", assign).await;
    assert_eq!(assigned["responsibility"]["pending"]["person_id"], "lin");
    assert_eq!(
        assigned["coordination_claim_acquired"], false,
        "assigning claims nothing"
    );
    refused!(
        team.take(MEI, API, false).await,
        PgError::ClaimHeld,
        "a pool claim on an item assigned to someone else"
    );
    refused!(
        team.take(MEI, API, true).await,
        PgError::Forbidden,
        "accepting an assignment made to someone else"
    );
    let accepted = team.take(LIN, API, true).await.unwrap();
    assert_eq!(accepted["coordination_claim_acquired"], true);
    let pool = prepare(&team.reads, MEI.token, PAGE.id).await;
    assert_eq!(pool["data"]["responsibility"]["relation"], "pool");
    let taken = team.take(MEI, PAGE, false).await.unwrap();
    assert_eq!(taken["responsibility"]["owner"], "mei");
}

/// Both developers submit a first version and refine it twice before any review decision.
/// A refinement is new evidence and a new review round for the same work; it supersedes the
/// previous round and names the selected version it replaces.
async fn refine(team: &mut Team, order: &mut Order) {
    let base = team.repo.base.clone();
    let (mut api, mut page) = (vec![], vec![]);
    for number in 1..=3u32 {
        let index = number as usize - 1;
        for (author, work, content, chain) in [
            (LIN, API, API_VERSIONS[index], &mut api),
            (MEI, PAGE, PAGE_VERSIONS[index], &mut page),
        ] {
            if number == 2 {
                refused!(
                    team.select_replacing(author, work, number, content, &base, None)
                        .await,
                    PgError::PreconditionsChanged,
                    "{} refines without naming the selected version",
                    work.id
                );
            }
            let version = team.implement(author, work, number, content, &base).await;
            assert_eq!(
                version.selection,
                number.to_string(),
                "{} selection",
                work.id
            );
            assert_eq!(
                team.round_state(&version.round_id).await,
                "open",
                "{} round",
                work.id
            );
            if let Some(previous) = chain.last() {
                let previous: &Version = previous;
                assert_eq!(
                    team.round_state(&previous.round_id).await,
                    "invalidated",
                    "{} version {} is superseded by its refinement",
                    work.id,
                    previous.number
                );
            }
            chain.push(version);
        }
    }
    // Nothing was returned, so there is nothing to rework yet.
    refused!(
        team.rework(LIN, API, &api[2].round_id).await,
        PgError::ReworkRequiresReturnedReview,
        "rework of a round that was never returned"
    );
    refused!(
        team.rework(MEI, PAGE, &page[2].round_id).await,
        PgError::ReworkRequiresReturnedReview,
        "rework of a round that was never returned"
    );
    order.api = api;
    order.page = page;
}

/// The independent reviewer approves the API item and returns the page item once. Returns the
/// approving decision ids of the final API and page versions.
async fn review(team: &mut Team, order: &mut Order) -> (String, String) {
    let api = order.api.last().unwrap().clone();
    let page = order.page.last().unwrap().clone();
    refused!(
        team.decide(LIN, API, &api.round_id, "approve").await,
        PgError::Forbidden,
        "the author decides the review of their own work"
    );
    let finalize = team
        .with_session(
            MANAGER,
            API,
            json!({"evidence_id":api.evidence_id,"context_complete":true}),
        )
        .await;
    refused!(
        team.try_run(MANAGER, API, "delivery.finalize", finalize)
            .await,
        PgError::ReviewRequired,
        "finalizing an item nobody has reviewed"
    );
    refused!(
        team.prepare_integration(MANAGER, API, &api, "no-decision")
            .await,
        PgError::ReviewRequired,
        "integrating an item nobody has reviewed"
    );
    team.observe(API).await;
    team.observe(PAGE).await;
    let approved = team
        .decide(CHEN, API, &api.round_id, "approve")
        .await
        .unwrap();
    assert_eq!(approved["state"], "approved");
    assert_eq!(
        approved["independence_kind"],
        "simulated_member_independent"
    );
    assert_eq!(approved["human_approval"], false);
    let returned = team
        .decide(CHEN, PAGE, &page.round_id, "reject")
        .await
        .unwrap();
    assert_eq!(returned["state"], "rejected");
    // Rework follows a returned review and nothing else.
    refused!(
        team.rework(LIN, API, &api.round_id).await,
        PgError::ReworkRequiresReturnedReview,
        "rework of an approved round"
    );
    let reworked = team.rework(MEI, PAGE, &page.round_id).await.unwrap();
    assert_eq!(reworked["rework_acknowledged"], true);
    let base = team.repo.base.clone();
    let page4 = team.implement(MEI, PAGE, 4, PAGE_VERSIONS[3], &base).await;
    assert_eq!(
        team.round_state(&page.round_id).await,
        "rejected",
        "the returned round stays as history"
    );
    assert_eq!(team.round_state(&page4.round_id).await, "open");
    team.observe(PAGE).await;
    let page_approved = team
        .decide(CHEN, PAGE, &page4.round_id, "approve")
        .await
        .unwrap();
    order.page.push(page4);
    (
        approved["decision_id"].as_str().unwrap().into(),
        page_approved["decision_id"].as_str().unwrap().into(),
    )
}

/// The manager integrates the API item with a fast-forward and finalizes it.
async fn deliver_api(team: &mut Team, order: &Order, decision: &str) {
    let api = order.api.last().unwrap();
    let (id, snapshot) = team.deliver(API, api, decision).await;
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Applied);
    assert_eq!(
        snapshot.report.content_witness,
        Some(IntegrationContentWitness::ExactRevision)
    );
    assert_eq!(
        team.main_revision(),
        api.source,
        "main fast-forwards to the approved candidate"
    );
    let confirmed = team.confirm(API, &id).await;
    assert_eq!(confirmed["integration"]["data"]["state"], "confirmed");
    let completed = team.finalize(API, api).await;
    assert_eq!(completed["task_complete"], true);
    assert_eq!(team.runtime_state(API).await, "completed");
}

/// Deliver the page item. Its approved version was built on the old target, so the service
/// accepts the request and the repository connector refuses it; the page is completed only
/// after its author rebuilt it on the new target and the rebuilt version was reviewed.
async fn deliver_page(team: &mut Team, order: &mut Order, decision: &str) {
    let new_target = team.main_revision();
    page_on_the_old_target(team, order, decision, &new_target).await;
    let declared = page_declaring_the_new_target(team, &new_target).await;
    let rebuilt = page_rebuilt_on_the_new_target(team, &new_target).await;
    order.page.extend([declared, rebuilt]);
}

/// The candidate declares the old target as its base and the target has moved since.
async fn page_on_the_old_target(team: &mut Team, order: &Order, decision: &str, new_target: &str) {
    let stale = order.page.last().unwrap();
    let (id, snapshot) = team.deliver(PAGE, stale, decision).await;
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(
        snapshot.report.content_witness,
        Some(IntegrationContentWitness::Unavailable {
            reason: ContentProofUnavailableReason::ContentChanged
        })
    );
    assert_eq!(
        team.main_revision(),
        new_target,
        "a rejected delivery leaves the target unchanged"
    );
    team.confirm(PAGE, &id).await;
    let rejected = team.inspect_integration(MANAGER, PAGE, &id).await;
    assert_eq!(rejected["state"], "rejected");
    assert_eq!(rejected["target_changed"], true);
    assert!(
        rejected["guidance"]["action"]
            .as_str()
            .unwrap()
            .contains("The repository target changed"),
        "{}",
        rejected["guidance"]
    );
    // The page is not finalized here: the guidance tells the deliverer not to finalize a
    // rejected version, and the service would not stop them.
    assert_ne!(team.runtime_state(PAGE).await, "completed");
}

/// A republication that declares the new target but was not rebuilt on it passes review,
/// because its bytes are right, and is refused as a changed base.
async fn page_declaring_the_new_target(team: &mut Team, new_target: &str) -> Version {
    let old_base = team.repo.base.clone();
    let declared = team
        .implement_declaring(MEI, PAGE, 5, PAGE_VERSIONS[4], &old_base, new_target)
        .await;
    team.observe(PAGE).await;
    let approved = team
        .decide(CHEN, PAGE, &declared.round_id, "approve")
        .await
        .unwrap();
    let (id, snapshot) = team
        .deliver(PAGE, &declared, approved["decision_id"].as_str().unwrap())
        .await;
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(
        snapshot.report.content_witness,
        Some(IntegrationContentWitness::Unavailable {
            reason: ContentProofUnavailableReason::BaseChanged
        })
    );
    assert_eq!(team.main_revision(), new_target);
    team.confirm(PAGE, &id).await;
    declared
}

/// The follow-up: rebuilt on the new target, reviewed again and delivered on top of the API.
async fn page_rebuilt_on_the_new_target(team: &mut Team, new_target: &str) -> Version {
    let rebuilt = team
        .implement(MEI, PAGE, 6, PAGE_VERSIONS[5], new_target)
        .await;
    team.observe(PAGE).await;
    let approved = team
        .decide(CHEN, PAGE, &rebuilt.round_id, "approve")
        .await
        .unwrap();
    let (id, snapshot) = team
        .deliver(PAGE, &rebuilt, approved["decision_id"].as_str().unwrap())
        .await;
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Applied);
    assert_eq!(team.main_revision(), rebuilt.source);
    assert_eq!(
        team.file_on_main(API.path),
        API_VERSIONS[2].trim_end(),
        "the page lands on top of the delivered api"
    );
    team.confirm(PAGE, &id).await;
    team.finalize(PAGE, &rebuilt).await;
    assert_eq!(team.runtime_state(PAGE).await, "completed");
    rebuilt
}

fn suggestion(request: &str, work: Work) -> PlanningSuggestRequest {
    serde_json::from_value(json!({"protocol_version":1,"request_id":request,
        "rationale":"customers asked for another filter","affected_work_keys":[work.id],
        "proposed_notes":{"summary":"add one more filter"}}))
    .unwrap()
}

fn follow_up_draft(request: &str) -> PlanningDraftRequest {
    serde_json::from_value(
        json!({"protocol_version":1,"request_id":request,"mode":"create",
        "changes":[{"op":"create_task","before":null,"after":{"work_id":"follow-up-filter",
            "external_key":"follow-up-filter","title":"Follow-up filter","goals":["support"],
            "scope_paths":["specs/api.md"],"acceptance":["the follow-up is verified"],
            "required_dependencies":[],"completion_policy":"independent_review",
            "definition_state":"draft","split_children":[]}}],
        "allowed_spec_roots":["specs"],"project_goal_keys":["support"],
        "self_approve_policy":OrdinaryPlanningSelfApprovePolicy::ordinary_default()}),
    )
    .unwrap()
}

/// A finalized item accepts no follow-up work from its developer: refinement belongs before
/// delivery. Every route a developer could take to change the item is refused.
async fn assert_closed_to_follow_up(team: &mut Team, who: Member, work: Work, approved: &Version) {
    assert_eq!(
        team.runtime_state(work).await,
        "completed",
        "{} is finalized",
        work.id
    );
    refused!(
        team.source
            .planning_draft(
                TENANT,
                PROJECT,
                who.token,
                &follow_up_draft(&format!("{}-draft", who.person))
            )
            .await,
        PgError::Forbidden,
        "{} drafts a follow-up task",
        who.person
    );
    refused!(
        team.source
            .planning_suggest(
                TENANT,
                PROJECT,
                who.token,
                &suggestion(&format!("{}-suggest", who.person), work)
            )
            .await,
        PgError::Forbidden,
        "{} suggests follow-up work",
        who.person
    );
    refused!(
        team.rework(who, work, &approved.round_id).await,
        PgError::ReworkRequiresReturnedReview,
        "{} reopens the finalized work",
        who.person
    );
    // Giving the claim up and asking for it again does not help either.
    let claim = team.claim_header(work);
    let args = team.with_session(who, work, claim).await;
    team.run(who, work, "claim.release", args).await;
    refused!(
        team.acquire(who, work).await,
        PgError::PreconditionsChanged,
        "{} claims the finalized work again",
        who.person
    );
    refused!(
        team.take(who, work, false).await,
        PgError::PreconditionsChanged,
        "{} takes the finalized work from the pool",
        who.person
    );
}

/// Run a scenario on a thread with a large stack. The futures of a full order are deep in a
/// debug build; the default stack of a test thread leaves too little margin across platforms.
fn on_large_stack<Body, Scenario>(body: Body)
where
    Body: FnOnce() -> Scenario + Send + 'static,
    Scenario: std::future::Future<Output = ()>,
{
    let thread = std::thread::Builder::new()
        .name("team-delivery-order".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(body())
        })
        .unwrap();
    if let Err(panic) = thread.join() {
        std::panic::resume_unwind(panic);
    }
}

/// The planned order: refinements happen before review and delivery, the stale page is
/// refused by the repository connector and delivered after a rebuild, and nothing can be added
/// to a finalized item.
#[test]
fn assignment_refinement_review_and_delivery_hold_in_the_planned_order() {
    on_large_stack(|| async {
        let mut team = Team::start().await;
        let mut order = Order::default();
        arrange(&mut team).await;
        refine(&mut team, &mut order).await;
        let (api_decision, page_decision) = review(&mut team, &mut order).await;
        deliver_api(&mut team, &order, &api_decision).await;
        deliver_page(&mut team, &mut order, &page_decision).await;
        let api = order.api.last().unwrap().clone();
        let page = order.page.last().unwrap().clone();
        assert_closed_to_follow_up(&mut team, LIN, API, &api).await;
        assert_closed_to_follow_up(&mut team, MEI, PAGE, &page).await;
    });
}

/// The unplanned order: delivering the first version closes the item, so a refinement can no
/// longer be started by anyone. This is why the refinements come before delivery.
#[test]
fn delivering_the_first_version_leaves_no_way_to_refine_the_item() {
    on_large_stack(|| async {
        let mut team = Team::start().await;
        arrange(&mut team).await;
        let base = team.repo.base.clone();
        let api = team.implement(LIN, API, 1, API_VERSIONS[0], &base).await;
        team.observe(API).await;
        let approved = team
            .decide(CHEN, API, &api.round_id, "approve")
            .await
            .unwrap();
        let (id, snapshot) = team
            .deliver(API, &api, approved["decision_id"].as_str().unwrap())
            .await;
        assert_eq!(snapshot.report.outcome, IntegrationOutcome::Applied);
        team.confirm(API, &id).await;
        team.finalize(API, &api).await;
        assert_closed_to_follow_up(&mut team, LIN, API, &api).await;
    });
}
