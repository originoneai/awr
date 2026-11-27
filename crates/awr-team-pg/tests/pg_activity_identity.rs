#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
};
use awr_team_pg::{AuthorizationStore, PgError, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn null_actor() -> Value {
    json!({"person_id":null,"display_name":null,"kind":null})
}

/// The disclosed identity: the person's name where one resolved, and the actor's
/// recorded kind either way.
fn actor(person_id: &str, display_name: &str, kind: &str) -> Value {
    json!({"person_id":person_id,"display_name":display_name,"kind":kind})
}

/// A resolved kind with no person to name (conflicting, stopped or unregistered).
fn kind_only(kind: &str) -> Value {
    json!({"person_id":null,"display_name":null,"kind":kind})
}

fn null_client() -> Value {
    json!({"display_name":null})
}

async fn full_directory_access(admin: &tokio_postgres::Client) {
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
             workstream_id,authority_version,can_read)
             SELECT tenant_id,project_id,actor_id,'cli-a',workstream_id,authority_version,can_read
             FROM awr_team.workstream_grants WHERE client_id='cli-b'",
            &[],
        )
        .await
        .unwrap();
}

async fn seed_request(admin: &tokio_postgres::Client, id: &str, actor: &str) {
    admin
        .execute(
            "INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,
             credential_id,action,result)
             VALUES($1,$2,$3,$4,'audit-client','audit-credential','work.prepare','succeeded')",
            &[&TENANT, &PROJECT, &id, &actor],
        )
        .await
        .unwrap();
}

fn activity_item<'a>(reply: &'a Value, id: &str) -> &'a Value {
    reply["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap_or_else(|| panic!("missing activity item {id}: {reply}"))
}

#[tokio::test]
async fn both_audit_reads_use_only_unambiguous_active_recorded_relationships() {
    let (_guard, admin, _, store) = setup().await;
    full_directory_access(&admin).await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
             ('reader-tenant','agent-operator','agent','Opaque agent','active'),
             ('reader-tenant','ambiguous-agent','agent','Opaque agent','active'),
             ('reader-tenant','disabled-binding','agent','Opaque agent','active'),
             ('reader-tenant','inactive-person','agent','Opaque agent','active'),
             ('reader-tenant','unbound-person','agent','Opaque agent','active'),
             ('reader-tenant','human-without-person','human','Opaque human','active'),
             ('reader-tenant','departed-human','human','Opaque human','active');
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
             ('reader-tenant','reader-project','person-one','Agent Owner','active'),
             ('reader-tenant','reader-project','person-two','Other Owner','active'),
             ('reader-tenant','reader-project','departed-person','Departed Owner','departed'),
             ('reader-tenant','reader-project','reviewer','Human Reviewer','active'),
             ('reader-tenant','reader-project','unbound-person','Must Not Be Inferred','active'),
             ('reader-tenant','reader-project','departed-human','Departed Human','departed');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
             ('reader-tenant','reader-project','binding-one','person-one','agent-operator','active'),
             ('reader-tenant','reader-project','binding-two','person-one','ambiguous-agent','active'),
             ('reader-tenant','reader-project','binding-three','person-two','ambiguous-agent','active'),
             ('reader-tenant','reader-project','binding-four','person-one','disabled-binding','disabled'),
             ('reader-tenant','reader-project','binding-five','departed-person','inactive-person','active');",
        )
        .await
        .unwrap();
    for (id, actor) in [
        ("request-agent", "agent-operator"),
        ("request-human", "reviewer"),
        ("request-ambiguous", "ambiguous-agent"),
        ("request-disabled", "disabled-binding"),
        ("request-inactive", "inactive-person"),
        ("request-unbound", "unbound-person"),
        ("request-missing", "human-without-person"),
        ("request-departed-human", "departed-human"),
        ("request-unknown-actor", "no-such-actor"),
    ] {
        seed_request(&admin, id, actor).await;
    }
    admin
        .batch_execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,
             event_type,actor_id,work_id,payload_json) VALUES
             ('reader-tenant','reader-project','development-agent',90,0,'session.start','agent-operator',NULL,'{}'),
             ('reader-tenant','reader-project','development-human',91,0,'session.start','reviewer',NULL,'{}')",
        )
        .await
        .unwrap();
    let persons_before: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.persons", &[])
        .await
        .unwrap()
        .get(0);

    let requests = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&requests, "request-agent")["actor"],
        actor("person-one", "Agent Owner", "agent")
    );
    assert_eq!(
        activity_item(&requests, "request-human")["actor"],
        actor("reviewer", "Human Reviewer", "human")
    );
    // An actor that resolves to nobody still discloses its recorded kind to a reader
    // who passed the gate; the kind is a property of the actor row, not of the person.
    for (id, kind) in [
        ("request-ambiguous", "agent"),
        ("request-disabled", "agent"),
        ("request-inactive", "agent"),
        ("request-unbound", "agent"),
        ("request-missing", "human"),
        ("request-departed-human", "human"),
    ] {
        assert_eq!(
            activity_item(&requests, id)["actor"],
            kind_only(kind),
            "{id}"
        );
    }
    // No recorded actor: nothing to say, not even a kind.
    assert_eq!(
        activity_item(&requests, "request-unknown-actor")["actor"],
        null_actor()
    );
    for item in requests["data"]["items"].as_array().unwrap() {
        assert_eq!(item["client"], null_client());
    }
    let persons_after: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.persons", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        persons_after, persons_before,
        "audit reads must not create people"
    );

    let development = store
        .query(TENANT, PROJECT, A, query("audit.development"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&development, "development-agent")["actor"],
        actor("person-one", "Agent Owner", "agent")
    );
    assert_eq!(
        activity_item(&development, "development-human")["actor"],
        actor("reviewer", "Human Reviewer", "human")
    );
    for item in development["data"]["items"].as_array().unwrap() {
        assert_eq!(item["client"], null_client());
    }
}

#[tokio::test]
async fn missing_directory_authority_redacts_names_without_rejecting_project_or_self_audit() {
    let (_guard, admin, _, store) = setup().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('reader-tenant','reader-project','agent','Directory Name','active')",
        )
        .await
        .unwrap();
    seed_request(&admin, "request-reader", "agent").await;

    assert!(matches!(
        store.directory().read(TENANT, PROJECT, A).await,
        Err(PgError::Forbidden)
    ));
    let partial = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(partial["data"]["scope"], "project");
    assert_eq!(
        activity_item(&partial, "request-reader")["actor"],
        null_actor()
    );
    assert_eq!(
        activity_item(&partial, "request-reader")["client"],
        null_client()
    );

    full_directory_access(&admin).await;
    let disclosed = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&disclosed, "request-reader")["actor"],
        actor("agent", "Directory Name", "human")
    );

    admin
        .execute(
            "UPDATE awr_team.project_memberships SET role='developer',membership_version=membership_version+1
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id='agent'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    let own = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(own["data"]["scope"], "self");
    assert_eq!(
        activity_item(&own, "request-reader")["actor"],
        actor("agent", "Directory Name", "human")
    );

    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET can_read=false
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id='agent' AND client_id='cli-a'
               AND workstream_id=$3",
            &[&TENANT, &PROJECT, &awr_core::Id::from(2).to_string()],
        )
        .await
        .unwrap();
    let redacted_self = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(redacted_self["data"]["scope"], "self");
    assert_eq!(
        activity_item(&redacted_self, "request-reader")["actor"],
        null_actor()
    );
}

async fn grant_agent_read(db: &str, id: &str, scope: AuthorizationScope, created_at_ms: u64) {
    let authorization = AgentAuthorization {
        id: id.into(),
        authorizer_person_id: PersonId::new("reader-person").unwrap(),
        responsible_person_id: PersonId::new("reader-person").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope,
        actions: BTreeSet::from([AuthorizedAction::Inspect]),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: created_at_ms as i64,
        binding_id: Some("reader-binding".into()),
    };
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: id.into(),
                authorization,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn agent_name_disclosure_requires_a_live_project_scoped_read_delegation() {
    let (_guard, admin, db, store) = setup().await;
    full_directory_access(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('reader-tenant','reader-project','reader-person','Project Reader','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('reader-tenant','reader-project','reader-binding','reader-person','agent','active')",
        )
        .await
        .unwrap();
    seed_request(&admin, "agent-own-request", "agent").await;
    // A client name follows exactly the same gate as the actor identity.
    store
        .connections()
        .record_client_name(TENANT, PROJECT, B, "Delegated Client")
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,
             credential_id,action,result)
             VALUES($1,$2,'agent-named-request','agent','cli-b','reader-b','work.prepare','succeeded')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    grant_agent_read(
        &db,
        "stream-read",
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: awr_core::Id::from(1).to_string(),
        },
        1000,
    )
    .await;

    let stream_scoped = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(stream_scoped["data"]["scope"], "self");
    assert_eq!(
        activity_item(&stream_scoped, "agent-own-request")["actor"],
        null_actor()
    );
    assert_eq!(
        activity_item(&stream_scoped, "agent-named-request")["client"],
        null_client()
    );

    grant_agent_read(
        &db,
        "project-read",
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        1001,
    )
    .await;
    let project_scoped = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(project_scoped["data"]["scope"], "self");
    assert_eq!(
        activity_item(&project_scoped, "agent-own-request")["actor"],
        actor("reader-person", "Project Reader", "agent")
    );
    assert_eq!(
        activity_item(&project_scoped, "agent-named-request")["client"],
        json!({"display_name":"Delegated Client"})
    );
}

trait PgWire: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> PgWire for T {}

// Count real Execute/simple-query frames; no product-code instrumentation or extension.
async fn counted_pg(
    upstream: tokio_postgres::Config,
    count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> (u16, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let upstream = upstream.clone();
            let count = count.clone();
            tokio::spawn(async move {
                socket.set_nodelay(true).unwrap();
                let port = upstream.get_ports().first().copied().unwrap_or(5432);
                let remote: Box<dyn PgWire> = match &upstream.get_hosts()[0] {
                    tokio_postgres::config::Host::Tcp(host) => {
                        let host = upstream
                            .get_hostaddrs()
                            .first()
                            .map(|ip| ip.to_string())
                            .unwrap_or_else(|| host.clone());
                        let stream = tokio::net::TcpStream::connect((host.as_str(), port))
                            .await
                            .unwrap();
                        stream.set_nodelay(true).unwrap();
                        Box::new(stream)
                    }
                    #[cfg(unix)]
                    tokio_postgres::config::Host::Unix(path) => Box::new(
                        tokio::net::UnixStream::connect(path.join(format!(".s.PGSQL.{port}")))
                            .await
                            .unwrap(),
                    ),
                };
                let (mut incoming, mut outgoing) = socket.into_split();
                let (mut responses, mut requests) = tokio::io::split(remote);
                let forward = async {
                    let length = incoming.read_u32().await?;
                    let mut startup = vec![0; length as usize - 4];
                    incoming.read_exact(&mut startup).await?;
                    requests.write_u32(length).await?;
                    requests.write_all(&startup).await?;
                    loop {
                        let kind = incoming.read_u8().await?;
                        let length = incoming.read_u32().await?;
                        assert!((4..=4 * 1024 * 1024).contains(&length));
                        let mut body = vec![0; length as usize - 4];
                        incoming.read_exact(&mut body).await?;
                        if kind == b'E' || kind == b'Q' {
                            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        requests.write_u8(kind).await?;
                        requests.write_u32(length).await?;
                        requests.write_all(&body).await?;
                        if kind == b'X' {
                            return Ok::<(), std::io::Error>(());
                        }
                    }
                };
                tokio::select! {_=forward=>{},_=tokio::io::copy(&mut responses,&mut outgoing)=>{}}
            });
        }
    });
    (port, task)
}

#[tokio::test]
async fn forty_audit_rows_add_at_most_three_statements_over_a_one_row_page() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_guard, admin, database, _) = setup().await;
    full_directory_access(&admin).await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
             SELECT 'reader-tenant','bulk-agent-'||g,'agent','Opaque agent','active'
             FROM generate_series(1,40) g;
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             SELECT 'reader-tenant','reader-project','bulk-person-'||g,'Bulk Person '||g,'active'
             FROM generate_series(1,40) g;
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             SELECT 'reader-tenant','reader-project','bulk-binding-'||g,'bulk-person-'||g,
                    'bulk-agent-'||g,'active' FROM generate_series(1,40) g;
             INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,
             credential_id,action,result)
             SELECT 'reader-tenant','reader-project','bulk-request-'||g,'bulk-agent-'||g,
                    'bulk-client-'||g,'bulk-credential','work.prepare','succeeded'
             FROM generate_series(1,40) g",
        )
        .await
        .unwrap();
    let upstream = common::with_app_role(&common::test_config(), &database);
    let count = Arc::new(AtomicUsize::new(0));
    let (port, proxy) = counted_pg(upstream.clone(), count.clone()).await;
    let mut counts = Vec::new();
    for limit in [1, 40] {
        let mut config = tokio_postgres::Config::new();
        config
            .host("127.0.0.1")
            .port(port)
            .user("awr_app")
            .password("app-test")
            .dbname(&database);
        let store = WorkstreamReadStore::from_config(config);
        let mut q = query("audit.requests");
        q.limit = Some(limit);
        count.store(0, Ordering::SeqCst);
        let result = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert_eq!(
            result["data"]["items"].as_array().unwrap().len(),
            limit as usize
        );
        assert!(
            result["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["actor"]["display_name"].is_string())
        );
        counts.push(count.load(Ordering::SeqCst));
        drop(store);
    }
    proxy.abort();
    println!("audit.requests statement counts for pages of 1 and 40: {counts:?}");
    assert!(
        counts[1] <= counts[0] + 3,
        "a 40-row page must not resolve actors one by one: {counts:?}"
    );
}
