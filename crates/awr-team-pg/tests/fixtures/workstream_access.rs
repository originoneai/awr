#![allow(dead_code)]
use crate::common;
use awr_core::{Id, Workstream, WorkstreamCatalog, WorkstreamState};
use awr_team::{SourceActivationPlan, WorkContract, WorkId, WorkstreamBundle, WorkstreamContract};
use awr_team_pg::{
    GraphStore, IngestRequest, SourceFile, SourceStore, WorkstreamQuery, WorkstreamReadStore,
    workstream_credential_hash,
};
use std::sync::MutexGuard;
use tokio_postgres::Client;

pub const TENANT: &str = "reader-tenant";
pub const PROJECT: &str = "reader-project";
pub const A: &str =
    "awr1.reader-a.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const B: &str =
    "awr1.reader-b.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const NONE: &str =
    "awr1.no-grants.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

pub fn query(op: &str) -> WorkstreamQuery {
    serde_json::from_value(serde_json::json!({"protocol_version":1,"op":op})).unwrap()
}

pub fn command(
    prepared: &serde_json::Value,
    request: &str,
    op: &str,
    args: serde_json::Value,
) -> awr_team_pg::WorkstreamCommand {
    serde_json::from_value(serde_json::json!({"protocol_version":1,"request_id":request,"op":op,
        "workstream_id":prepared["workstream_id"],"work_id":prepared["data"]["work_id"],
        "coordinator_epoch":prepared["coordinator_epoch"],"expected_project_revision":prepared["project_revision"],
        "expected_authority_version":prepared["authority_version"],"expected_ownership_version":prepared["data"]["ownership_version"],
        "expected_contract_hash":prepared["data"]["contract_hash"],"args":args})).unwrap()
}

pub async fn prepare(store: &WorkstreamReadStore, token: &str, work: &str) -> serde_json::Value {
    let mut q = query("work.prepare");
    q.work_id = Some(work.into());
    store.query(TENANT, PROJECT, token, q).await.unwrap()
}

pub async fn enable_writes(admin: &Client) {
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
}

pub async fn setup() -> (MutexGuard<'static, ()>, Client, String, WorkstreamReadStore) {
    let (guard, admin, db, store, _) = setup_inner(false, None, false, false).await;
    (guard, admin, db, store)
}

pub async fn setup_with_three_streams()
-> (MutexGuard<'static, ()>, Client, String, WorkstreamReadStore) {
    let (guard, admin, db, store, _) = setup_inner(false, None, true, false).await;
    (guard, admin, db, store)
}

pub async fn setup_with_legacy_resource() -> (
    MutexGuard<'static, ()>,
    Client,
    String,
    WorkstreamReadStore,
    String,
) {
    let (guard, admin, db, store, legacy) = setup_inner(true, None, false, false).await;
    (guard, admin, db, store, legacy.expect("legacy resource"))
}

pub async fn setup_with_specs(
    files: Vec<SourceFile>,
) -> (MutexGuard<'static, ()>, Client, String, WorkstreamReadStore) {
    let (guard, admin, db, store, _) = setup_inner(false, Some(files), false, false).await;
    (guard, admin, db, store)
}

pub async fn setup_with_assigned_tasks()
-> (MutexGuard<'static, ()>, Client, String, WorkstreamReadStore) {
    let (guard, admin, db, store, _) = setup_inner(false, None, false, true).await;
    (guard, admin, db, store)
}

async fn setup_inner(
    legacy_resource: bool,
    spec_files: Option<Vec<SourceFile>>,
    third_stream: bool,
    assigned_tasks: bool,
) -> (
    MutexGuard<'static, ()>,
    Client,
    String,
    WorkstreamReadStore,
    Option<String>,
) {
    let (guard, admin, db) = common::fresh_team_schema().await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('reader-tenant','Readers','active'),('other-tenant','Other','active');
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
          ('reader-tenant','agent','human','Worker','active'),('reader-tenant','reviewer','human','Reviewer','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status) VALUES
          ('reader-tenant','reader-project','p','team','epoch-a','active'),('other-tenant','reader-project','p','team','epoch-b','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
          ('reader-tenant','reader-project','agent','admin'),('reader-tenant','reader-project','reviewer','reviewer');
        INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
          VALUES('reader-tenant','reader-project','main','Main','active');").await.unwrap();
    for (id, client, token) in [
        ("reader-a", "cli-a", A),
        ("reader-b", "cli-b", B),
        ("no-grants", "unscoped", NONE),
    ] {
        let hash = workstream_credential_hash(token).unwrap();
        admin
            .execute(
                "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
            VALUES($1,$2,'agent',$3,$4)",
                &[&TENANT, &id, &client, &hash],
            )
            .await
            .unwrap();
    }
    let legacy = if legacy_resource {
        admin
            .batch_execute(
                "INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                 VALUES('reader-tenant','reader-project','a','a')",
            )
            .await
            .unwrap();
        Some(
            GraphStore::from_config(common::with_db(&common::test_config(), &db))
                .reserve(TENANT, PROJECT, "a", "named", "legacy-resource")
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let mut stream_definitions = vec![(1, "alpha"), (2, "private-beta")];
    if third_stream {
        stream_definitions.push((3, "hidden-gamma"));
    }
    let definitions = stream_definitions
        .into_iter()
        .map(|(i, key)| Workstream {
            id: Id::from(i),
            project_id: PROJECT.into(),
            external_key: key.into(),
            title: key.into(),
            state: WorkstreamState::Active,
            authority_version: 1,
            goal_keys: vec![key.into()],
            acceptance_contracts: if spec_files.is_some() {
                vec![format!("docs/{key}.md")]
            } else {
                vec![]
            },
        })
        .collect();
    let mut work_definitions = vec![
        ("a", 1, vec![]),
        ("b-private", 2, vec![]),
        ("c", 1, vec!["b-private"]),
    ];
    if third_stream {
        work_definitions.push(("d-hidden", 3, vec![]));
    }
    if assigned_tasks {
        work_definitions.extend([("aa-assigned", 1, vec![]), ("ab-assigned", 1, vec![])]);
    }
    let contracts = work_definitions
        .into_iter()
        .map(|(id, stream, deps)| WorkstreamContract {
            workstream_id: Id::from(stream),
            contract: WorkContract {
                dependency_acceptance: Default::default(),
                execution_settlement: None,
                codec: WorkContract::CODEC.into(),
                work_id: WorkId::new(id).unwrap(),
                external_key: id.into(),
                goals: vec!["ship".into()],
                hard_rules: vec!["preserve compatibility".into()],
                scope_paths: vec!["src".into()],
                acceptance: vec!["verified".into()],
                required_dependencies: deps.into_iter().map(Into::into).collect(),
                completion_policy: "review".into(),
                verification_requirements: vec!["report".into()],
            },
        })
        .collect();
    let bundle = WorkstreamBundle {
        codec: WorkstreamBundle::CODEC.into(),
        catalog: WorkstreamCatalog {
            version: 1,
            project_id: PROJECT.into(),
            legacy_default: None,
            workstreams: definitions,
        },
        contracts,
    };
    let source = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let c = source
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: "workstreams/1".into(),
            files: {
                let mut files = spec_files.unwrap_or_default();
                files.push(SourceFile {
                    path: "workstreams.json".into(),
                    bytes: serde_json::to_vec(&bundle).unwrap(),
                });
                files
            },
        })
        .await
        .unwrap();
    source
        .approve(
            TENANT,
            PROJECT,
            &c.proposal_id,
            "reviewer",
            &c.manifest_digest,
        )
        .await
        .unwrap();
    source
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &c.proposal_id,
            &SourceActivationPlan {
                candidate_digest: c.manifest_digest.clone(),
                parser_version: c.parser_version.clone(),
                expected_authority_epoch: c.base_epoch.clone(),
                approved_candidate_digest: c.manifest_digest.clone(),
            },
        )
        .await
        .unwrap();
    for (client, stream) in [("cli-a", 1), ("cli-b", 2)] {
        admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
            VALUES($1,$2,'agent',$3,$4,1,true)", &[&TENANT,&PROJECT,&client,&Id::from(stream).to_string()]).await.unwrap();
    }
    for (index, stream, work, session, next, client) in [
        (2, 1, "a", "session-a", "continue alpha", "cli-a"),
        (
            3,
            2,
            "b-private",
            "session-b",
            "PRIVATE NEXT ACTION",
            "cli-b",
        ),
    ] {
        let stream = Id::from(stream).to_string();
        admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
            VALUES($1,$2,$3,'main',$4,'agent',$6,$3,'active',$5,1)", &[&TENANT,&PROJECT,&session,&work,&stream,&client]).await.unwrap();
        let checkpoint = format!("cp-{session}");
        admin.execute("INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json)
            VALUES($1,$2,$3,$4,'context','contract',1,$5,'[]')", &[&TENANT,&PROJECT,&checkpoint,&session,&next]).await.unwrap();
        admin
            .execute(
                "UPDATE awr_team.sessions SET latest_checkpoint_id=$1 WHERE id=$2",
                &[&checkpoint, &session],
            )
            .await
            .unwrap();
        admin.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json,workstream_id)
            VALUES($1,$2,$3,$4,0,'session.started','agent',$5,'{}',$6)", &[&TENANT,&PROJECT,&format!("ev-{session}"),&(index as i64),&work,&stream]).await.unwrap();
    }
    admin
        .execute(
            "UPDATE awr_team.projects SET project_revision=3 WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    let store =
        WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    (guard, admin, db, store, legacy)
}
