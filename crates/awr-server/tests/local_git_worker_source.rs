#![cfg(feature = "pg-tests")]
//! Physical source/real Git/PG mechanism proof; not native business acceptance.
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod access;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "fixtures/local_git.rs"]
mod git;
#[path = "../../awr-team-pg/tests/fixtures/delivery_integration.rs"]
mod integration_fixture;

use access::*;
use awr_server::{
    config::{DeliveryWorkerConfig, DeliveryWorkerSpec, LocalGitWorkerConfig, LocalGitWorkerSpec},
    service::{
        ProjectBinding, ServiceConfig, delivery_sync::WorkerFailure,
        delivery_workers::DeliveryWorkers,
    },
};
use awr_team::delivery::DeliveryCandidate;
use awr_team_pg::*;
use integration_fixture::{Fixture, REVIEWER, SUPERVISOR, WORKER};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

fn service() -> ServiceConfig {
    ServiceConfig {
        version: 1,
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_hosts: vec![],
        allowed_web_origins: vec![],
        oauth: None,
        projects: vec![ProjectBinding {
            key: "team".into(),
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
        }],
    }
}

async fn start(
    f: &Fixture,
    repo: &git::GitFixture,
    name: &str,
    source: bool,
    integrate: bool,
) -> DeliveryWorkers {
    let mut repository = repo.config.clone();
    repository.inspection_timeout_ms = 3000;
    let source = source.then(|| DeliveryWorkerConfig {
        version: 1,
        workers: vec![DeliveryWorkerSpec {
            project: "team".into(),
            worker_id: format!("source-{name}"),
            workstreams: vec![awr_core::Id::from(1)],
            credential_env: "AWR_SYNTHETIC_WORKER".into(),
            poll_interval_ms: 100,
            lease_seconds: 5,
            operation_timeout_ms: 2000,
            max_backoff_ms: 1000,
            page_size: 4,
            max_pages_per_poll: 2,
            max_jobs_per_poll: 4,
        }],
    });
    let git = LocalGitWorkerConfig {
        version: 1,
        workers: vec![LocalGitWorkerSpec {
            project: "team".into(),
            worker_id: format!("git-{name}"),
            credential_env: "AWR_SYNTHETIC_WORKER".into(),
            integration_enabled: integrate,
            repository,
            poll_interval_ms: 100,
            lease_seconds: 10,
            operation_timeout_ms: 5000,
            max_backoff_ms: 1000,
            page_size: 4,
            max_pages_per_poll: 2,
            max_jobs_per_poll: 4,
        }],
    };
    DeliveryWorkers::start(
        &service(),
        source,
        Some(git),
        DeliverySyncStore::from_config(f.config.clone()),
        |name| {
            assert_eq!(name, "AWR_SYNTHETIC_WORKER");
            Some(WORKER.into())
        },
    )
    .await
    .unwrap()
}

async fn wait(mut check: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(25), async {
        while !check().await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("source-first closure timed out");
}

async fn read(f: &Fixture, token: &str, op: &str, id: Option<&str>) -> Value {
    let mut q = query(op);
    q.work_id = Some("a".into());
    if op == "review.inspect" {
        q.review_round_id = id.map(str::to_owned);
    } else {
        q.request_id = id.map(str::to_owned);
    }
    f.reads.query(TENANT, PROJECT, token, q).await.unwrap()["data"].clone()
}

async fn status(f: &Fixture) -> Value {
    // Read a new snapshot after normal concurrent transaction contention.
    // Bound retries and refuse to conceal an unrelated authorization/source error.
    for _ in 0..40 {
        match f
            .store
            .source_publication_status(TENANT, PROJECT, SUPERVISOR, "a")
            .await
        {
            Ok(value) => return value,
            Err(error) if snapshot_contention(&error) => {
                tokio::time::sleep(Duration::from_millis(25)).await
            }
            Err(error) => panic!("source status failed: {error}"),
        }
    }
    panic!("source status contention exceeded the bounded retry budget")
}

fn snapshot_contention(error: &PgError) -> bool {
    matches!(error, PgError::Db(error) if error.code().is_some_and(|c| c.code() == "40001" || c.code() == "40P01"))
}

async fn count(f: &Fixture, sql: &str) -> i64 {
    f.admin.query_one(sql, &[]).await.unwrap().get(0)
}

async fn proof(f: &Fixture) -> Value {
    read(f, SUPERVISOR, "delivery.neutral.inspect", None).await["facts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|fact| {
            fact["current"] == true
                && fact["observation"]["kind"] == "verification"
                && fact["observation"]["check"] == "local_git.manifest"
                && fact["observation"]["outcome"] == "passed"
        })
        .expect("the actual observer must verify the Git manifest")
        .clone()
}

async fn observe_and_review(f: &mut Fixture, repo: &git::GitFixture) -> Value {
    assert!(f.request.review_decision_id.is_empty());
    assert_eq!(
        f.selection.candidate.binding.required_checks,
        ["local_git.manifest"]
    );
    assert_eq!(
        count(f, "SELECT count(*) FROM awr_team.review_decisions").await,
        0
    );
    assert_eq!(
        count(f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        0
    );
    let observer = start(f, repo, "observer", false, false).await;
    let monitor = observer.git_monitor();
    wait(async || {
        monitor.snapshots()[0].current_changed > 0 && monitor.snapshots()[0].current_unchanged >= 2
    })
    .await;
    observer.shutdown().await;
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(
        count(
            f,
            "SELECT count(*) FROM awr_team.delivery_integration_intents"
        )
        .await,
        0
    );
    let observed = proof(f).await;
    assert_eq!(
        observed["observation"]["provenance"]["source"],
        "adapter_observation"
    );
    assert_eq!(
        observed["observation"]["provenance"]["reference"]
            .as_str()
            .unwrap()
            .starts_with("awr-local-git-report:"),
        true
    );
    f.approve_source_review().await;
    let review = read(
        f,
        REVIEWER,
        "review.inspect",
        Some(&f.request.review_round_id),
    )
    .await;
    assert_eq!(
        review["review"]["decisions"][0]["decision_id"],
        f.request.review_decision_id
    );
    observed
}

/// All prerequisites come from the supervisor's public queries. Deliberately
/// discard the preparation reply and recover its original durable outcome.
async fn prepare_losing_reply(f: &Fixture, key: &str) -> String {
    let p = prepare(&f.reads, SUPERVISOR, "a").await;
    let neutral = read(f, SUPERVISOR, "delivery.neutral.inspect", None).await;
    let connector = neutral["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["enabled"] == true && c["provider"] == "local_git")
        .unwrap();
    let review = read(
        f,
        SUPERVISOR,
        "review.inspect",
        Some(&f.request.review_round_id),
    )
    .await;
    let candidate: DeliveryCandidate =
        serde_json::from_value(neutral["candidate"].clone()).unwrap();
    let command = command(
        &p,
        key,
        "delivery.integration.prepare",
        json!({
            "source_snapshot_id":p["source_snapshot_id"], "connector_id":connector["connector_id"],
            "connector_version":connector["connector_version"], "candidate_digest":candidate.binding.digest().unwrap(),
            "selection_version":neutral["selection_version"], "evidence_id":review["review"]["evidence_id"],
            "review_round_id":review["review"]["round_id"], "review_decision_id":review["review"]["decisions"][0]["decision_id"],
            "operation":"fast_forward",
        }),
    );
    let reply = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, SUPERVISOR, command)
        .await
        .unwrap();
    assert_eq!(reply["execution_authorized"], false);
    drop(reply);
    let outcome = read(f, SUPERVISOR, "delivery.neutral.outcome", Some(key)).await;
    assert_eq!(outcome["receipt"]["data"]["state"], "prepared");
    outcome["receipt"]["data"]["integration_id"]
        .as_str()
        .unwrap()
        .into()
}

async fn confirmed(f: &Fixture, id: &str) -> bool {
    match f
        .store
        .inspect_integration(TENANT, PROJECT, SUPERVISOR, "a", id)
        .await
    {
        Ok(value) => value["state"] == "confirmed",
        Err(error) if snapshot_contention(&error) => false,
        Err(error) => panic!("original integration query failed: {error}"),
    }
}

/// An integration receipt can precede the periodic target observer. Establish
/// the source baseline only after that observer has read the actual new target;
/// its legitimate observation must not be mistaken for restart write churn.
async fn target_observed(f: &Fixture, repo: &git::GitFixture) -> bool {
    read(f, SUPERVISOR, "delivery.neutral.inspect", None).await["facts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|fact| {
            let observation = &fact["observation"];
            fact["current"] == true
                && observation["kind"] == "integration_observation"
                && observation["external_reference"]
                    .as_str()
                    .is_some_and(|reference| reference.starts_with("local-git-target:"))
                && observation["result_revision"]["value"] == repo.source
                && observation["outcome"] == "applied"
                && observation["provenance"]["reference"]
                    .as_str()
                    .is_some_and(|reference| reference.starts_with("awr-local-git-report:"))
        })
}

async fn closure_diagnostics(f: &Fixture, repo: &git::GitFixture, root: &Path, id: &str) -> Value {
    let intents: Vec<Value> = f.admin.query("SELECT jsonb_build_object('id',id,'kind',kind,'state',state,
        'fence',fence,'attempts',attempts,'publication_id',publication_id,'failure_code',failure_code,
        'read_set',read_set_json,'generation',generation,'metadata',result_json) FROM awr_team.delivery_sync_intents ORDER BY created_at,id", &[])
        .await.unwrap().into_iter().map(|row| row.get(0)).collect();
    json!({"intents":intents,"source_status":status(f).await,
        "integration":f.store.inspect_integration(TENANT,PROJECT,SUPERVISOR,"a",id).await.unwrap(),
        "source":fs::read_to_string(root.join("ledger.yaml")).unwrap(),
        "target":repo.bare(&["rev-parse","refs/heads/main"])})
}

async fn wait_closed(f: &Fixture, repo: &git::GitFixture, root: &Path, id: &str) {
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        while !(confirmed(f, id).await
            && target_observed(f, repo).await
            && status(f).await["source_synchronized"] == true)
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    if result.is_err() {
        let diagnostics = tokio::time::timeout(
            Duration::from_secs(5),
            closure_diagnostics(f, repo, root, id),
        )
        .await;
        panic!("source-first closure timed out; synthetic diagnostics: {diagnostics:?}");
    }
}

async fn assert_closed(
    f: &Fixture,
    repo: &git::GitFixture,
    root: &Path,
    id: &str,
    before: &[u8],
    observed: &Value,
) {
    assert!(confirmed(f, id).await);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    assert_eq!(
        repo.bare(&["reflog", "show", "--format=%H", "refs/heads/main"])
            .lines()
            .count(),
        1
    );
    let current = status(f).await;
    for field in [
        "source_synchronized",
        "projection_current",
        "cursor_current",
    ] {
        assert_eq!(current[field], true, "{field}: {current}");
    }
    let bytes = fs::read(root.join("ledger.yaml")).unwrap();
    assert_ne!(bytes, before);
    assert_eq!(
        current["source_fingerprint"],
        awr_source::fingerprint(&bytes)
    );
    assert_eq!(
        current["confirmed_fingerprint"],
        current["source_fingerprint"]
    );
    let after = String::from_utf8(bytes).unwrap();
    let original = std::str::from_utf8(before).unwrap();
    assert_eq!(
        after.split_once("  - id: c\n").unwrap().1,
        original.split_once("  - id: c\n").unwrap().1
    );
    assert!(after.starts_with("# Preserve operator comments and unrelated work.\n"));
    let work = after
        .split_once("  - id: a\n")
        .unwrap()
        .1
        .split_once("  - id: c\n")
        .unwrap()
        .0;
    assert!(work.contains("    status: planned\n"));
    assert!(
        work.contains("delivery_sync:") || work.contains("\"delivery_sync\":"),
        "the exact work must contain its typed delivery reference"
    );
    assert_eq!(
        prepare(&f.reads, SUPERVISOR, "a").await["data"]["contract_hash"],
        f.set.contract_hash
    );
    assert_eq!(
        proof(f).await["fact_id"],
        observed["fact_id"],
        "target integration cannot invalidate unchanged source proof"
    );
    assert_eq!(
        count(
            f,
            "SELECT count(*) FROM awr_team.delivery_integration_intents"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            f,
            "SELECT count(*) FROM awr_team.events WHERE event_type='delivery.integration.dispatch'"
        )
        .await,
        1
    );
    assert_eq!(
        count(f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn physical_source_first_sha1_and_sha256_close_without_a_pr_and_restart_without_churn() {
    for sha256 in [false, true] {
        let repo = git::GitFixture::integration(sha256);
        repo.bare(&["config", "core.logAllRefUpdates", "true"]);
        let root = repo.source_contract();
        let before = fs::read(root.join("ledger.yaml")).unwrap();
        let mut f =
            integration_fixture::setup_source_integration(repo.integration_candidate(), &root)
                .await;
        let observed = observe_and_review(&mut f, &repo).await;
        let id = prepare_losing_reply(&f, "public-source-integration").await;
        let workers = start(&f, &repo, "closure", true, true).await;
        wait_closed(&f, &repo, &root, &id).await;
        workers.shutdown().await;
        assert_closed(&f, &repo, &root, &id, &before, &observed).await;
        let landed = fs::read(root.join("ledger.yaml")).unwrap();
        let modified = fs::metadata(root.join("ledger.yaml"))
            .unwrap()
            .modified()
            .unwrap();
        let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
        let publications = count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications",
        )
        .await;
        let workers = start(&f, &repo, "reconstructed", true, true).await;
        let monitor = workers.git_monitor();
        wait(async || {
            monitor.snapshots()[0].terminal_intents_observed >= 2
                && monitor.snapshots()[0].current_unchanged >= 2
        })
        .await;
        workers.shutdown().await;
        assert_eq!(fs::read(root.join("ledger.yaml")).unwrap(), landed);
        assert_eq!(
            fs::metadata(root.join("ledger.yaml"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        assert_eq!(
            count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
            facts
        );
        assert_eq!(
            count(
                &f,
                "SELECT count(*) FROM awr_team.delivery_source_publications"
            )
            .await,
            publications
        );
        assert_eq!(
            read(
                &f,
                SUPERVISOR,
                "delivery.neutral.outcome",
                Some("public-source-integration")
            )
            .await["receipt"]["data"]["integration_id"],
            id
        );
        assert_closed(&f, &repo, &root, &id, &before, &observed).await;
    }
}

#[tokio::test]
async fn concurrent_combined_services_share_one_git_effect_and_confirmed_source() {
    let repo = git::GitFixture::integration(false);
    repo.bare(&["config", "core.logAllRefUpdates", "true"]);
    let root = repo.source_contract();
    let before = fs::read(root.join("ledger.yaml")).unwrap();
    let mut f =
        integration_fixture::setup_source_integration(repo.integration_candidate(), &root).await;
    let observed = observe_and_review(&mut f, &repo).await;
    let id = prepare_losing_reply(&f, "concurrent-source-integration").await;
    let (one, two) = tokio::join!(
        start(&f, &repo, "one", true, true),
        start(&f, &repo, "two", true, true)
    );
    let source_one = one.source_monitor();
    let source_two = two.source_monitor();
    let git_one = one.git_monitor();
    let git_two = two.git_monitor();
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        while !(confirmed(&f, &id).await
            && target_observed(&f, &repo).await
            && status(&f).await["source_synchronized"] == true)
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    one.shutdown().await;
    two.shutdown().await;
    if result.is_err() {
        // Keep bounded, synthetic failure evidence before fixture teardown.
        // Read-only diagnostics must not retry, repair or supply prerequisites.
        let diagnostics = tokio::time::timeout(Duration::from_secs(5), async {
            json!({"closure":closure_diagnostics(&f, &repo, &root, &id).await,
                "source_one":source_one.snapshots(),"source_two":source_two.snapshots(),
                "git_one":git_one.snapshots(),"git_two":git_two.snapshots()})
        })
        .await;
        panic!("concurrent source-first closure timed out; synthetic diagnostics: {diagnostics:?}");
    }
    assert_closed(&f, &repo, &root, &id, &before, &observed).await;
}

#[tokio::test]
async fn source_conflict_preserves_concurrent_bytes_and_recovers_without_reintegrating_git() {
    let repo = git::GitFixture::integration(false);
    repo.bare(&["config", "core.logAllRefUpdates", "true"]);
    let root = repo.source_contract();
    let before = fs::read(root.join("ledger.yaml")).unwrap();
    let mut f =
        integration_fixture::setup_source_integration(repo.integration_candidate(), &root).await;
    let observed = observe_and_review(&mut f, &repo).await;
    let id = prepare_losing_reply(&f, "conflicted-source-integration").await;
    let mut changed = before.clone();
    changed.extend_from_slice(b"# Synthetic concurrent operator edit\n");
    fs::write(root.join("ledger.yaml"), &changed).unwrap();
    let workers = start(&f, &repo, "conflict", true, true).await;
    let monitor = workers.source_monitor();
    wait(async || confirmed(&f, &id).await && monitor.snapshots()[0].deferred > 0).await;
    workers.shutdown().await;
    assert_eq!(fs::read(root.join("ledger.yaml")).unwrap(), changed);
    assert_eq!(status(&f).await["source_synchronized"], false);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
    // The pump now supersedes the conflicted intent and keeps a pending one instead of
    // parking it as blocked; what matters is that no source write succeeded and one remains.
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE kind='source' AND state='succeeded'").await,
        0
    );
    assert!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE kind='source' AND state IN ('pending','leased','blocked')").await > 0);
    // Explicit fixture/operator resolution restores its baseline; the service
    // itself never deletes or overwrites the concurrent edit.
    fs::write(root.join("ledger.yaml"), &before).unwrap();
    let workers = start(&f, &repo, "resolved", true, true).await;
    wait_closed(&f, &repo, &root, &id).await;
    workers.shutdown().await;
    assert_closed(&f, &repo, &root, &id, &before, &observed).await;
}

#[tokio::test]
async fn landed_source_write_with_lost_receipt_is_confirmed_after_restart_without_rewriting() {
    let repo = git::GitFixture::integration(false);
    repo.bare(&["config", "core.logAllRefUpdates", "true"]);
    let root = repo.source_contract();
    let before = fs::read(root.join("ledger.yaml")).unwrap();
    let mut f =
        integration_fixture::setup_source_integration(repo.integration_candidate(), &root).await;
    let observed = observe_and_review(&mut f, &repo).await;
    let id = prepare_losing_reply(&f, "lost-source-receipt").await;
    let git_only = start(&f, &repo, "git-only", false, true).await;
    wait(async || confirmed(&f, &id).await && target_observed(&f, &repo).await).await;
    git_only.shutdown().await;
    assert_eq!(status(&f).await["source_synchronized"], false);
    // Failure injection supplies no prerequisite; it loses the acknowledgement
    // after a real confined filesystem write, retaining the pending journal.
    f.admin.batch_execute("CREATE FUNCTION awr_team.lose_source_write_reply() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.event_type='delivery.source.write' THEN RAISE EXCEPTION 'synthetic response loss'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER lose_source_write_reply BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.lose_source_write_reply()").await.unwrap();
    let workers = start(&f, &repo, "lost-reply", true, true).await;
    let monitor = workers.source_monitor();
    wait(async || monitor.snapshots()[0].failure_code == Some(WorkerFailure::OutcomeUnknown)).await;
    workers.shutdown().await;
    let landed = fs::read(root.join("ledger.yaml")).unwrap();
    assert_ne!(landed, before);
    let modified = fs::metadata(root.join("ledger.yaml"))
        .unwrap()
        .modified()
        .unwrap();
    let publication = status(&f).await["history"][0]["publication_id"].clone();
    f.admin
        .batch_execute("DROP TRIGGER lose_source_write_reply ON awr_team.events")
        .await
        .unwrap();
    let workers = start(&f, &repo, "recovered-reply", true, true).await;
    wait_closed(&f, &repo, &root, &id).await;
    workers.shutdown().await;
    assert_eq!(
        status(&f).await["history"][0]["publication_id"],
        publication
    );
    assert_eq!(fs::read(root.join("ledger.yaml")).unwrap(), landed);
    assert_eq!(
        fs::metadata(root.join("ledger.yaml"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert_closed(&f, &repo, &root, &id, &before, &observed).await;
}
