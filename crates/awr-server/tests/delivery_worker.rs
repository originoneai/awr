#![cfg(feature = "pg-tests")]
//! Synthetic server/PG lifecycle regressions; not native team business acceptance.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "../../awr-team-pg/tests/fixtures/delivery_publication.rs"]
mod publication;

use awr_core::Id;
use awr_server::{
    config::DeliveryWorkerConfig,
    service::{
        ProjectBinding, ServiceConfig,
        delivery_sync::{DeliveryWorkerMonitor, DeliveryWorkerRuntime, WorkerFailure, WorkerState},
    },
};
use awr_team_pg::{ClaimDeliverySyncIntent, DeliverySyncLease, DeliverySyncStore};
use fixture::*;
use publication::*;
use serde_json::{Value, json};
use std::time::Duration;

fn service() -> ServiceConfig {
    ServiceConfig {
        version: 1,
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_hosts: vec![],
        allowed_web_origins: vec![],
        oauth: None,
        projects: vec![ProjectBinding {
            key: "one".into(),
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
        }],
    }
}

fn config(worker: &str) -> DeliveryWorkerConfig {
    toml::from_str(&format!(
        r#"version = 1
[[workers]]
project = "one"
worker_id = "{worker}"
workstreams = ["00000000000000000000000001"]
credential_env = "SYNTHETIC_WORKER_CREDENTIAL"
poll_interval_ms = 100
lease_seconds = 10
operation_timeout_ms = 5000
max_backoff_ms = 800
page_size = 1
max_pages_per_poll = 1
max_jobs_per_poll = 1
"#
    ))
    .unwrap()
}

async fn start(f: &Fixture, config: DeliveryWorkerConfig) -> DeliveryWorkerRuntime {
    DeliveryWorkerRuntime::start(&service(), Some(config), f.restarted(), |_| Some(A.into()))
        .await
        .unwrap()
}

async fn wait(
    monitor: &DeliveryWorkerMonitor,
    condition: impl Fn(&awr_server::service::delivery_sync::DeliveryWorkerSnapshot) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if monitor.snapshots().iter().all(&condition) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("worker state: {:?}", monitor.snapshots()));
}

async fn count(f: &Fixture, sql: &str) -> i64 {
    f.admin.query_one(sql, &[]).await.unwrap().get(0)
}

async fn queue(f: &Fixture) -> Value {
    f.store
        .sync_intents(TENANT, PROJECT, A, Id::from(1), 64, None)
        .await
        .unwrap()
}

async fn source_lease(f: &Fixture, seconds: i32) -> DeliverySyncLease {
    let rows = queue(f).await;
    let i = rows["intents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "source" && i["state"] == "pending")
        .unwrap();
    f.store
        .claim_sync_intent(
            TENANT,
            PROJECT,
            A,
            ClaimDeliverySyncIntent {
                request_id: "synthetic-worker-lost-claim".into(),
                read_set: f.set.clone(),
                intent_id: i["intent_id"].as_str().unwrap().into(),
                worker_id: "before-restart".into(),
                expected_fence: i["fence"].as_str().unwrap().into(),
                lease_seconds: seconds,
            },
        )
        .await
        .unwrap()
        .unwrap()
}

async fn select_c(f: &Fixture, session: &str) -> awr_team_pg::SelectDeliveryCandidate {
    f.admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        VALUES($1,$2,$3,'main','c','agent','cli-a',$3,'active','00000000000000000000000001',1)",
        &[&TENANT, &PROJECT, &session]).await.unwrap();
    select(&f.reads, &f.store, "c", session).await
}

fn no_secret(value: impl serde::Serialize) {
    let text = serde_json::to_string(&value).unwrap();
    for secret in [A, B, "SYNTHETIC_WORKER_CREDENTIAL"] {
        assert!(!text.contains(secret));
    }
}

#[tokio::test]
async fn absent_workers_do_not_read_credentials_or_connect_a_store() {
    for config in [
        None,
        Some(DeliveryWorkerConfig {
            version: 1,
            workers: vec![],
        }),
    ] {
        let runtime = DeliveryWorkerRuntime::start(
            &service(),
            config,
            DeliverySyncStore::new("deliberately-unconfigured"),
            |_| panic!("disabled workers must not request credentials"),
        )
        .await
        .unwrap();
        assert!(runtime.monitor().snapshots().is_empty());
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn complete_startup_admission_creates_no_grant_and_starts_no_partial_worker() {
    let f = setup_publisher().await;
    let before = f.bytes();
    let mut cfg = config("good");
    let mut bad = cfg.workers[0].clone();
    bad.worker_id = "bad".into();
    bad.credential_env = "INVALID_SYNTHETIC_IDENTITY".into();
    cfg.workers.push(bad);
    let result = DeliveryWorkerRuntime::start(&service(), Some(cfg), f.restarted(), |name| {
        Some(
            if name == "INVALID_SYNTHETIC_IDENTITY" {
                B
            } else {
                A
            }
            .into(),
        )
    })
    .await;
    assert!(result.is_err());
    no_secret(result.err().unwrap());
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(f.bytes(), before);
    assert_eq!(
        count(
            &f,
            "SELECT sum(attempts)::bigint FROM awr_team.delivery_sync_intents"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        0
    );
    f.admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=false WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(
        DeliveryWorkerRuntime::start(&service(), Some(config("denied")), f.restarted(), |_| Some(
            A.into()
        ))
        .await
        .is_err()
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.workstream_grants WHERE can_manage"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn bounded_loop_synchronizes_refresh_and_source_without_completion_or_secret_output() {
    let f = setup_publisher().await;
    let mut cfg = config("normal");
    cfg.workers[0].page_size = 64;
    let runtime = start(&f, cfg).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.synchronized == 2).await;
    runtime.shutdown().await;
    wait(&monitor, |s| s.state == WorkerState::Stopped).await;
    assert_eq!(f.status().await["source_synchronized"], true);
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        1
    );
    let text = String::from_utf8(f.bytes()).unwrap();
    assert_eq!(text.matches("status: planned").count(), 4);
    assert!(text.starts_with("# Preserve this comment"));
    let audit: Value = f
        .admin
        .query_one(
            "SELECT jsonb_agg(to_jsonb(a)) FROM awr_team.ops_audit_records a",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    no_secret(monitor.snapshots());
    no_secret(queue(&f).await);
    no_secret(audit);
    no_secret(text);
}

#[tokio::test]
async fn bounded_pages_advance_past_succeeded_history_and_visit_every_configured_scope() {
    let f = setup_publisher().await;
    let mut cfg = config("backlog");
    // Scope two is explicitly granted only in this isolated synthetic fixture.
    f.admin.batch_execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,grant_version,can_read,can_write,can_manage)
        SELECT tenant_id,project_id,actor_id,client_id,'00000000000000000000000002',1,1,true,false,true FROM awr_team.workstream_grants WHERE client_id='cli-a' LIMIT 1").await.unwrap();
    cfg.workers[0].workstreams.push(Id::from(2));
    let runtime = start(&f, cfg).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.synchronized == 2).await;
    let selected = select_c(&f, "session-c-backlog").await;
    f.observe(&selected, "backlog").await;
    wait(&monitor, |s| s.synchronized == 4).await;
    runtime.shutdown().await;
    assert!(monitor.snapshots()[0].polls >= 4);
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE state='succeeded'"
        )
        .await,
        4
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE fence<>1"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn competing_server_workers_have_one_effective_publication() {
    let f = setup_publisher().await;
    let (one, two) = tokio::join!(start(&f, config("one")), start(&f, config("two")));
    let m1 = one.monitor();
    let m2 = two.monitor();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if m1.snapshots()[0].synchronized + m2.snapshots()[0].synchronized == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    one.shutdown().await;
    two.shutdown().await;
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        1
    );
    assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE state='succeeded' AND fence=1").await, 2);
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn revocation_and_expiry_during_a_live_lease_stop_authorized_effects_and_backoff_is_cancellable()
 {
    for change in [
        "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'",
    ] {
        let f = setup_publisher().await;
        let lease = source_lease(&f, 60).await;
        f.store
            .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
            .await
            .unwrap();
        let before = f.bytes();
        let runtime = start(&f, config("revoked")).await;
        let monitor = runtime.monitor();
        wait(&monitor, |s| s.live_leases_observed > 0).await;
        f.admin.batch_execute(change).await.unwrap();
        wait(&monitor, |s| {
            s.failure_code == Some(WorkerFailure::AuthorizationUnavailable) && s.failures >= 3
        })
        .await;
        assert!(monitor.snapshots()[0].retry_delay_ms <= 800);
        if change.contains("expires_at") {
            // Restoring the same synthetic principal must clear its current failure,
            // even when a live lease leaves this clean poll with no effect to perform.
            f.admin
                .batch_execute("UPDATE awr_team.credentials SET expires_at=clock_timestamp()+interval '1 hour' WHERE id='reader-a'")
                .await
                .unwrap();
            wait(&monitor, |s| {
                s.state == WorkerState::Running
                    && s.failure_code.is_none()
                    && s.retry_delay_ms == 100
            })
            .await;
        }
        let instant = std::time::Instant::now();
        runtime.shutdown().await;
        assert!(instant.elapsed() < Duration::from_secs(1));
        assert_eq!(f.bytes(), before);
        assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE kind='source' AND state='succeeded'").await, 0);
        no_secret(monitor.snapshots());
    }
}

#[tokio::test]
async fn changed_source_is_reported_and_deferred_without_overwriting_authoritative_bytes() {
    let f = setup_publisher().await;
    let mut changed = f.bytes();
    changed.extend_from_slice(b"# synthetic concurrent source change\n");
    std::fs::write(f.root.join("ledger.yaml"), &changed).unwrap();
    let runtime = start(&f, config("drift")).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.deferred > 0 && s.failure_code == Some(WorkerFailure::PreconditionsChanged)
    })
    .await;
    runtime.shutdown().await;
    assert_eq!(f.bytes(), changed);
    assert_eq!(f.status().await["source_synchronized"], false);
    assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE kind='source' AND state='blocked' AND failure_code='preconditions_changed'").await, 1);
}

#[tokio::test]
async fn restart_queries_live_lease_then_recovers_a_real_landed_effect_without_rewriting() {
    let f = setup_publisher().await;
    let before = f.bytes();
    f.admin.batch_execute("CREATE FUNCTION awr_team.fail_worker_write() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.event_type='delivery.source.write' THEN RAISE EXCEPTION 'synthetic response loss'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER fail_worker_write BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.fail_worker_write()").await.unwrap();
    let mut cfg = config("restart");
    cfg.workers[0].lease_seconds = 5;
    cfg.workers[0].operation_timeout_ms = 2000;
    let runtime = start(&f, cfg.clone()).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.failure_code == Some(WorkerFailure::OutcomeUnknown)
    })
    .await;
    runtime.shutdown().await;
    let landed = f.bytes();
    assert_ne!(landed, before);
    let metadata = std::fs::metadata(f.root.join("ledger.yaml")).unwrap();
    let journal = f.status().await["history"][0].clone();
    assert_eq!(journal["phase"], "pending");
    f.admin
        .batch_execute("DROP TRIGGER fail_worker_write ON awr_team.events")
        .await
        .unwrap();
    let runtime = start(&f, cfg).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.live_leases_observed > 0).await;
    assert_eq!(monitor.snapshots()[0].attempts, 0);
    assert_eq!(f.bytes(), landed);
    wait(&monitor, |s| s.synchronized == 1).await;
    runtime.shutdown().await;
    assert_eq!(f.bytes(), landed);
    let recovered = std::fs::metadata(f.root.join("ledger.yaml")).unwrap();
    assert_eq!(metadata.modified().unwrap(), recovered.modified().unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata.ino(), recovered.ino());
    }
    assert_eq!(
        f.status().await["history"][0]["publication_id"],
        journal["publication_id"]
    );
    assert_eq!(f.status().await["source_synchronized"], true);
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        1
    );
    assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE kind='source' AND fence=2 AND state='succeeded'").await, 1);
}

#[tokio::test]
async fn stale_associated_publication_is_retained_and_never_rebound_by_the_loop() {
    let f = setup_publisher().await;
    let lease = source_lease(&f, 5).await;
    let journal = f
        .store
        .prepare_sync_source(TENANT, PROJECT, A, &lease, 5)
        .await
        .unwrap();
    f.admin.batch_execute("UPDATE awr_team.delivery_sync_intents SET expires_at=clock_timestamp()-interval '1 second';
        UPDATE awr_team.delivery_connectors SET enabled=false").await.unwrap();
    let before = f.bytes();
    let runtime = start(&f, config("stale")).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.outdated_bindings_observed > 0
            && s.failure_code == Some(WorkerFailure::PreconditionsChanged)
    })
    .await;
    runtime.shutdown().await;
    assert_eq!(f.bytes(), before);
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        1
    );
    let row = f.admin.query_one("SELECT publication_id,fence,state FROM awr_team.delivery_sync_intents WHERE kind='source'", &[]).await.unwrap();
    assert_eq!(row.get::<_, String>(0), journal.publication_id);
    assert_eq!(row.get::<_, i64>(1), 1);
    assert_eq!(row.get::<_, String>(2), "leased");
}

#[tokio::test]
async fn timeout_and_shutdown_during_a_source_effect_preserve_unknown_journal_for_recovery() {
    for cancel in [false, true] {
        let f = setup_publisher().await;
        let before = f.bytes();
        f.admin.batch_execute("CREATE FUNCTION awr_team.delay_worker_write() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN IF NEW.event_type='delivery.source.write' THEN PERFORM pg_sleep(3); END IF; RETURN NEW; END $$;
            CREATE TRIGGER delay_worker_write BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.delay_worker_write()").await.unwrap();
        let mut cfg = config("cancel-effect");
        cfg.workers[0].lease_seconds = 5;
        cfg.workers[0].operation_timeout_ms = if cancel { 2000 } else { 500 };
        let runtime = start(&f, cfg.clone()).await;
        let monitor = runtime.monitor();
        tokio::time::timeout(Duration::from_secs(10), async {
            while f.bytes() == before {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        if !cancel {
            wait(&monitor, |s| {
                s.failure_code == Some(WorkerFailure::OutcomeUnknown)
            })
            .await;
        }
        let stopped_at = std::time::Instant::now();
        runtime.shutdown().await;
        assert!(stopped_at.elapsed() < Duration::from_secs(1));
        wait(&monitor, |s| s.state == WorkerState::Stopped).await;
        let landed = f.bytes();
        let metadata = std::fs::metadata(f.root.join("ledger.yaml")).unwrap();
        // The canceled SQL transaction finishes rollback before removing the fixture trigger.
        f.admin
            .batch_execute("DROP TRIGGER delay_worker_write ON awr_team.events")
            .await
            .unwrap();
        let history = f.status().await;
        assert_eq!(history["history"][0]["phase"], "pending");
        assert_eq!(history["source_synchronized"], false);
        assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE kind='source' AND state='succeeded'").await, 0);
        cfg.workers[0].operation_timeout_ms = 2000;
        let runtime = start(&f, cfg).await;
        let monitor = runtime.monitor();
        wait(&monitor, |s| s.synchronized == 1).await;
        runtime.shutdown().await;
        assert_eq!(f.bytes(), landed);
        let recovered = std::fs::metadata(f.root.join("ledger.yaml")).unwrap();
        assert_eq!(metadata.modified().unwrap(), recovered.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(metadata.ino(), recovered.ino());
        }
        assert_eq!(
            count(
                &f,
                "SELECT count(*) FROM awr_team.delivery_source_publications"
            )
            .await,
            1
        );
        assert_eq!(f.status().await["source_synchronized"], true);
        assert_eq!(
            count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
            0
        );
    }
}

#[tokio::test]
async fn dropping_runtime_stops_polling_and_does_not_settle_future_intents() {
    let f = setup_publisher().await;
    let before = f.bytes();
    let immediately_dropped = start(&f, config("drop-before-first-poll")).await;
    let immediate_monitor = immediately_dropped.monitor();
    drop(immediately_dropped);
    wait(&immediate_monitor, |s| s.state == WorkerState::Stopped).await;
    assert_eq!(f.bytes(), before);
    assert_eq!(immediate_monitor.snapshots()[0].polls, 0);
    let runtime = start(&f, config("drop")).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.synchronized == 2).await;
    drop(runtime);
    wait(&monitor, |s| s.state == WorkerState::Stopped).await;
    let selected = select_c(&f, "session-c-after-drop").await;
    f.observe(&selected, "after-drop").await;
    let polls = monitor.snapshots()[0].polls;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(monitor.snapshots()[0].polls, polls);
    assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE work_id='c' AND state='pending'").await, 2);
}

#[tokio::test]
async fn loopback_http_and_workers_share_graceful_shutdown_and_abort_boundaries() {
    for abort in [false, true] {
        let f = setup_publisher().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = awr_server::service::router(
            service(),
            address,
            awr_team_pg::WorkstreamReadStore::from_config(f.config.clone()),
        )
        .unwrap();
        let runtime = start(&f, config("server")).await;
        let monitor = runtime.monitor();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(awr_server::service::serve_router_with_delivery_workers(
            listener,
            router,
            runtime,
            async {
                let _ = stopped.await;
            },
        ));
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let url = format!("http://{address}/v1/projects/one/query");
        let body = json!({"protocol_version":1,"op":"delivery.source.status","work_id":"a"});
        assert_eq!(
            http.post(&url).json(&body).send().await.unwrap().status(),
            reqwest::StatusCode::FORBIDDEN
        );
        wait(&monitor, |s| s.synchronized == 2).await;
        let response = http
            .post(&url)
            .bearer_auth(A)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let status: Value = response.json().await.unwrap();
        assert_eq!(status["data"]["source_synchronized"], true);
        assert_eq!(
            count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
            0
        );
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            stop.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        wait(&monitor, |s| s.state == WorkerState::Stopped).await;
        let polls = monitor.snapshots()[0].polls;
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(monitor.snapshots()[0].polls, polls);
        no_secret(monitor.snapshots());
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_binary_uses_optional_file_and_child_credentials_then_stops_on_sigterm() {
    use std::{
        io::BufRead,
        process::{Command, Stdio},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let f = setup_publisher().await;
    let raw = common::test_database_url_raw();
    let database = f.config.get_dbname().unwrap();
    let app = if raw.starts_with("postgres://") || raw.starts_with("postgresql://") {
        let mut url = reqwest::Url::parse(&raw).unwrap();
        url.set_path(&format!("/{database}"));
        let pairs = url
            .query_pairs()
            .filter(|(k, _)| !matches!(k.as_ref(), "dbname" | "user" | "password"))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        url.query_pairs_mut().extend_pairs(pairs);
        url.set_username("awr_app").unwrap();
        url.set_password(Some("app-test")).unwrap();
        url.to_string()
    } else {
        format!("{raw} dbname={database} user=awr_app password=app-test")
    };
    let path = f.root.join("service.toml");
    let workers = f.root.join("workers.toml");
    std::fs::write(&path, format!("version=1\nlisten='127.0.0.1:0'\n[[projects]]\nkey='one'\ntenant_id='{TENANT}'\nproject_id='{PROJECT}'\n")).unwrap();
    std::fs::write(
        &workers,
        r#"version=1
[[workers]]
project='one'
worker_id='binary-worker'
workstreams=['00000000000000000000000001']
credential_env='SYNTHETIC_WORKER_CREDENTIAL'
poll_interval_ms=100
lease_seconds=10
operation_timeout_ms=5000
"#,
    )
    .unwrap();
    // Inputs are isolated child environment values, never global process mutation or live credentials.
    let mut child = Child(
        Command::new(env!("CARGO_BIN_EXE_awr-server"))
            .args(["serve", "--config"])
            .arg(path)
            .env("AWR_TEAM_DATABASE_URL", app)
            .env("AWR_TEAM_DELIVERY_WORKER_CONFIG", workers)
            .env("SYNTHETIC_WORKER_CREDENTIAL", A)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (announcement, mut reader) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            (serde_json::from_str::<Value>(&line).unwrap(), reader)
        }),
    )
    .await
    .unwrap()
    .unwrap();
    no_secret(&announcement);
    let url = format!(
        "http://{}/v1/projects/one/query",
        announcement["listen"].as_str().unwrap()
    );
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let response: Value = http
                .post(&url)
                .bearer_auth(A)
                .json(&json!({"protocol_version":1,"op":"delivery.source.status","work_id":"a"}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if response["data"]["source_synchronized"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
    // The PID belongs to this test's held child; it cannot be a caller-selected process.
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.success());
    let mut output = String::new();
    std::io::Read::read_to_string(&mut reader, &mut output).unwrap();
    let mut errors = String::new();
    std::io::Read::read_to_string(&mut child.0.stderr.take().unwrap(), &mut errors).unwrap();
    no_secret(&output);
    no_secret(&errors);
    assert!(errors.contains("\"state\":\"stopped\""));
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE state='succeeded'"
        )
        .await,
        2
    );
}
