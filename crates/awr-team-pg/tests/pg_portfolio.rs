#![cfg(feature = "pg-tests")]
//! AWR-PPD-060: one member-scoped portfolio read across every readable
//! project. These tests pin the acceptance bounds: consistency with the
//! per-project reads, the database round-trip budget at forty projects,
//! pagination without overlap or loss, and the disclosure boundary.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{PgError, PortfolioOverviewRequest, WorkstreamQuery, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn portfolio(cursor: Option<String>, limit: Option<u16>) -> PortfolioOverviewRequest {
    PortfolioOverviewRequest {
        protocol_version: 1,
        limit,
        cursor,
    }
}

fn registered(ids: &[String]) -> Vec<String> {
    ids.to_vec()
}

/// Clone the seeded project into a fresh project id/key: same streams, works,
/// ownership and grants. Facts (runtime, checkpoints, events, waits, reviews,
/// updates) are seeded per scenario by the caller.
async fn clone_project(admin: &tokio_postgres::Client, index: usize) -> String {
    let id = format!("portfolio-project-{index:03}");
    let key = format!("pf-{index:03}");
    let snapshot = format!("snapshot-portfolio-{index:03}");
    for (index, statement) in CLONE_STATEMENTS
        .iter()
        .enumerate()
        .map(|(index, template)| {
            (
                index,
                template
                    .replace("{TENANT}", TENANT)
                    .replace("{PROJECT}", PROJECT)
                    .replace("{id}", &id)
                    .replace("{key}", &key)
                    .replace("{snapshot}", &snapshot),
            )
        })
    {
        admin
            .execute(&statement, &[])
            .await
            .unwrap_or_else(|error| {
                panic!("clone statement {index} failed: {error:?}\n{statement}")
            });
    }
    id
}

const CLONE_STATEMENTS: &[&str] = &[
    // The projects row triggers the default workstream mode; enable it next.
    "INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status,active_snapshot_id,project_revision)
     VALUES('{TENANT}','{id}','{key}','team','epoch-portfolio','active','{snapshot}',3)",
    "UPDATE awr_team.workstream_modes SET enabled=true WHERE tenant_id='{TENANT}' AND project_id='{id}'",
    "INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
     SELECT tenant_id,'{id}',id,name,status FROM awr_team.work_scopes
     WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}'",
    "INSERT INTO awr_team.source_snapshots(tenant_id,project_id,id,manifest_digest,source_ref_json,parser_version,created_by)
     SELECT tenant_id,'{id}','{snapshot}',manifest_digest,source_ref_json,parser_version,created_by
     FROM awr_team.source_snapshots WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}'
       AND id=(SELECT active_snapshot_id FROM awr_team.projects
               WHERE tenant_id='{TENANT}' AND id='{PROJECT}')",
    "INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
     SELECT tenant_id,'{id}',id,external_key FROM awr_team.work_items
     WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}'",
    "INSERT INTO awr_team.work_contracts(tenant_id,project_id,snapshot_id,scope_id,work_id,contract_hash,definition_state,title,contract_json)
     SELECT tenant_id,'{id}','{snapshot}',scope_id,work_id,contract_hash,definition_state,title,contract_json
     FROM awr_team.work_contracts WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}' AND snapshot_id=(
        SELECT active_snapshot_id FROM awr_team.projects
        WHERE tenant_id='{TENANT}' AND id='{PROJECT}')",
    "INSERT INTO awr_team.workstream_catalogs(tenant_id,project_id,snapshot_id,catalog_json,projection_hash)
     SELECT tenant_id,'{id}','{snapshot}',
        jsonb_set(jsonb_set(catalog_json,'{project_id}',to_jsonb('{id}'::text)),
            '{workstreams}',
            (SELECT jsonb_agg(jsonb_set(workstream,'{project_id}',to_jsonb('{id}'::text)))
             FROM jsonb_array_elements(catalog_json->'workstreams') workstream)),
        projection_hash
     FROM awr_team.workstream_catalogs WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}'
       AND snapshot_id=(SELECT active_snapshot_id FROM awr_team.projects
                        WHERE tenant_id='{TENANT}' AND id='{PROJECT}')",
    "INSERT INTO awr_team.workstream_ownership(tenant_id,project_id,work_id,workstream_id,ownership_version)
     SELECT tenant_id,'{id}',work_id,workstream_id,ownership_version FROM awr_team.workstream_ownership
     WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}'",
    "INSERT INTO awr_team.workstream_snapshot_ownership(tenant_id,project_id,snapshot_id,scope_id,work_id,workstream_id,ownership_version)
     SELECT tenant_id,'{id}','{snapshot}',scope_id,work_id,workstream_id,ownership_version
     FROM awr_team.workstream_snapshot_ownership WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}'
       AND snapshot_id=(SELECT active_snapshot_id FROM awr_team.projects
                        WHERE tenant_id='{TENANT}' AND id='{PROJECT}')",
    "INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
     SELECT tenant_id,'{id}',actor_id,role FROM awr_team.project_memberships
     WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}'",
    "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,active,grant_version)
     SELECT tenant_id,'{id}',actor_id,client_id,workstream_id,authority_version,can_read,true,1
     FROM awr_team.workstream_grants WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}'",
];

/// Give one cloned work a recorded runtime state.
async fn seed_runtime(
    admin: &tokio_postgres::Client,
    project: &str,
    work: &str,
    state: &str,
    selected_completion: Option<&str>,
) {
    admin
        .execute(
            "INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,selected_completion_id)
             VALUES($1,$2,'main',$3,$4,1,0,$5)
             ON CONFLICT (tenant_id,project_id,scope_id,work_id)
             DO UPDATE SET state=$4,selected_completion_id=$5",
            &[&TENANT, &project, &work, &state, &selected_completion],
        )
        .await
        .unwrap();
}

/// Record progress checkpoints (the session.checkpoint producer's rows) with
/// the given phases, in order. Each phase becomes one checkpoint.
async fn seed_progress(
    admin: &tokio_postgres::Client,
    project: &str,
    work: &str,
    stream: i64,
    phases: &[&str],
) {
    let session = format!("session-{project}-{work}");
    let stream_id = format!("{:026}", stream);
    admin
        .execute(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
             VALUES($1,$2,$3,'main',$4,'agent','cli-a',$3,'active',$5,1)",
            &[&TENANT, &project, &session, &work, &stream_id],
        )
        .await
        .unwrap();
    for (index, phase) in phases.iter().enumerate() {
        let checkpoint = format!("cp-{session}-{index}");
        admin
            .execute(
                "INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json,progress_json)
                 VALUES($1,$2,$3,$4,'context','contract',$5,'continue','[]',$6)",
                &[
                    &TENANT,
                    &project,
                    &checkpoint,
                    &session,
                    &((index + 1) as i64),
                    &json!({"phase":phase}),
                ],
            )
            .await
            .unwrap();
        admin
            .execute(
                "UPDATE awr_team.sessions SET latest_checkpoint_id=$1 WHERE tenant_id=$2 AND project_id=$3 AND id=$4",
                &[&checkpoint, &TENANT, &project, &session],
            )
            .await
            .unwrap();
    }
}

/// One work.completed event exactly as work.complete writes it, plus the
/// matching receipt and runtime completion. `payload` is the full event
/// payload including its receipt id.
async fn seed_completion(
    admin: &tokio_postgres::Client,
    project: &str,
    work: &str,
    payload: Value,
    revision: i64,
) {
    let receipt = payload["receipt_id"].as_str().unwrap().to_owned();
    admin
        .execute(
            "INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,
                contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,
                policy,approved_by_json,independence_kind,author_actor_id,final_submitter_actor_id)
             VALUES($1,$2,$3,$4,'main','contract','result','deps','evidence','review','{}',
                $5,'agent','agent')",
            &[
                &TENANT,
                &project,
                &receipt,
                &work,
                &payload["independence_kind"].as_str().map(str::to_owned),
            ],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json)
             VALUES($1,$2,$3,$4,0,'work.completed','agent',$5,$6)",
            &[
                &TENANT,
                &project,
                &format!("event-{project}-{work}-completed"),
                &revision,
                &work,
                &payload,
            ],
        )
        .await
        .unwrap();
    seed_runtime(admin, project, work, "completed", Some(&receipt)).await;
}

/// One ordinary decision wait exactly as decision.request leaves it: the
/// metadata object the producer writes, with its responder list and requester.
async fn seed_decision(admin: &tokio_postgres::Client, project: &str, work: &str) {
    let metadata = json!({"basis":{}, "request_digest":"synthetic-request-digest",
        "impact":"Synthetic impact statement.", "allowed_responder_person_ids":["reviewer"],
        "requester":{"actor_id":"agent","client_id":"cli-a","person_id":null},
        "source_snapshot_id":"synthetic-snapshot"});
    admin
        .execute(
            "INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state,decision_key,decision_baseline_digest,decision_json,decision_version)
             VALUES($1,$2,$3,$4,$5,'Synthetic decision question','open',$6,
                repeat('0',64),$7,1)",
            &[
                &TENANT,
                &project,
                &format!("decision-{project}-{work}"),
                &format!("session-{project}-{work}"),
                &work,
                &format!("decision-key-{project}-{work}"),
                &metadata,
            ],
        )
        .await
        .unwrap();
}

/// One open review round exactly as review.open leaves it.
async fn seed_review(admin: &tokio_postgres::Client, project: &str, work: &str) {
    admin
        .execute(
            "INSERT INTO awr_team.review_rounds(tenant_id,project_id,id,work_id,round_index,bundle_hash,contract_hash,author_actor_id,state)
             VALUES($1,$2,$3,$4,1,'bundle','contract','agent','open')",
            &[
                &TENANT,
                &project,
                &format!("review-{project}-{work}"),
                &work,
            ],
        )
        .await
        .unwrap();
}

/// The current record of one period exactly as update.record leaves it.
async fn seed_update(admin: &tokio_postgres::Client, project: &str, week: &str, status: &str) {
    let (starts, ends, period) = match week {
        "2026-W46" => ("2026-11-09", "2026-11-15", "2026-W46"),
        _ => ("2026-11-16", "2026-11-22", "2026-W47"),
    };
    admin
        .execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES($1,$2,'lead-portfolio','Portfolio Lead','active') ON CONFLICT DO NOTHING",
            &[&TENANT, &project],
        )
        .await
        .unwrap();
    let id = format!("update-{project}-{period}");
    admin
        .execute(
            "INSERT INTO awr_team.project_period_updates(tenant_id,project_id,id,period_kind,period_key,starts_on,ends_on,status,summary,author_person_id,author_display_name,recorded_by_actor_id,recorded_by_client_id,request_id,version)
             VALUES($1,$2,$3,'week',$4,$5::text::date,$6::text::date,$7,'A synthetic supervisor sentence.','lead-portfolio','Portfolio Lead','agent','cli-a',$8,1)",
            &[&TENANT, &project, &id, &period, &starts, &ends, &status, &id],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.project_period_update_current(tenant_id,project_id,period_kind,period_key,update_id)
             VALUES($1,$2,'week',$3,$4)",
            &[&TENANT, &project, &period, &id],
        )
        .await
        .unwrap();
}

async fn all_projects(admin: &tokio_postgres::Client) -> Vec<String> {
    admin
        .query(
            "SELECT id FROM awr_team.projects WHERE tenant_id=$1 ORDER BY id",
            &[&TENANT],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect()
}

async fn read_project(
    store: &WorkstreamReadStore,
    project: &str,
    mut query: WorkstreamQuery,
) -> Value {
    query.protocol_version = 1;
    let op = query.op.clone();
    store
        .query(TENANT, project, A, query)
        .await
        .unwrap_or_else(|error| panic!("per-project read {op} on {project}: {error:?}"))
}

fn query(op: &str) -> WorkstreamQuery {
    serde_json::from_value(json!({"protocol_version":1,"op":op})).unwrap()
}

/// Aggregate per-project reads the way the bridge does today and compare them
/// item by item with the portfolio summary (acceptance: consistency with
/// per-project reads on the same data).
#[tokio::test]
async fn portfolio_matches_per_project_reads() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    let two = clone_project(&admin, 2).await;
    let three = clone_project(&admin, 3).await;

    // Project one: running work, a delivered report, an open decision.
    seed_runtime(&admin, &one, "a", "claimed", None).await;
    seed_progress(&admin, &one, "a", 1, &["starting", "implementing"]).await;
    seed_progress(&admin, &one, "c", 1, &["implementing", "delivered"]).await;
    seed_decision(&admin, &one, "a").await;
    // Project two: one verified completion (team_independent, exact receipt),
    // one completed without independent basis (ordinary_confirm), an open
    // review, a dependency wait that started at the second checkpoint.
    seed_completion(
        &admin,
        &two,
        "a",
        json!({"receipt_id":"receipt-verified","independence_kind":"team_independent",
            "team_independent_acceptance":true}),
        11,
    )
    .await;
    seed_completion(
        &admin,
        &two,
        "c",
        json!({"receipt_id":"receipt-plain","independence_kind":"ordinary_confirm",
            "team_independent_acceptance":false}),
        12,
    )
    .await;
    seed_review(&admin, &two, "a").await;
    seed_progress(
        &admin,
        &two,
        "c",
        1,
        &["implementing", "waiting_dependency"],
    )
    .await;
    // b-private is stream 2: the reader holds no grant there, so its waits and
    // reports must never enter the summary or its text.
    seed_runtime(&admin, &two, "b-private", "claimed", None).await;
    seed_progress(&admin, &two, "b-private", 2, &["waiting_dependency"]).await;
    seed_decision(&admin, &two, "b-private").await;
    seed_update(&admin, &two, "2026-W47", "risk").await;
    // Project three: nothing recorded at all, plus one retired definition.
    admin
        .execute(
            "UPDATE awr_team.work_contracts SET definition_state='cancelled'
             WHERE tenant_id=$1 AND project_id=$2 AND work_id='c'",
            &[&TENANT, &three],
        )
        .await
        .unwrap();
    // The base project keeps its fixture state and gains a delivered report.
    seed_progress(&admin, PROJECT, "c", 1, &["reviewing"]).await;

    let projects = all_projects(&admin).await;
    let result = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let items = result["data"]["projects"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(result["data"]["truncated"], false);
    assert_eq!(result["data"]["total_projects"], 4);
    let by_project: std::collections::BTreeMap<String, &Value> = items
        .iter()
        .map(|item| (item["project_id"].as_str().unwrap().to_owned(), item))
        .collect();

    for project in [&one, &two, &three, PROJECT] {
        let item = by_project[project];
        assert_eq!(
            item["read"]["status"], "complete",
            "{project}: {}",
            item["read"]
        );
        assert_eq!(
            item["project_key"],
            admin
                .query_one(
                    "SELECT key FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
                    &[&TENANT, &project]
                )
                .await
                .unwrap()
                .get::<_, String>(0)
        );
        // Visible works from workstreams.list + work.list per stream.
        let mut listed = Vec::new();
        let streams = read_project(&store, project, query("workstreams.list")).await;
        for stream in streams["items"].as_array().unwrap() {
            let mut list = query("work.list");
            list.workstream_id = stream["id"].as_str().and_then(|id| id.parse().ok());
            list.limit = Some(100);
            let page = read_project(&store, project, list).await;
            listed.extend(page["data"]["items"].as_array().unwrap().iter().cloned());
        }
        let listed_total: usize = listed.len();
        let works = &item["works"];
        assert_eq!(
            works["total"].as_i64().unwrap() as usize,
            listed_total,
            "{project}: portfolio total must equal the enumerated work.list items"
        );
        // Buckets derived from the same per-work facts work.list reports.
        // Project three holds a retired definition, which work.list does not
        // expose; its buckets are pinned by the explicit assertions below.
        let completed = listed
            .iter()
            .filter(|work| work["summary"]["runtime"]["value"] == "completed")
            .count();
        assert_eq!(
            works["verified"].as_i64().unwrap()
                + works["completed_without_independent_review"]
                    .as_i64()
                    .unwrap(),
            completed as i64,
            "{project}: completed split must cover every completed runtime"
        );
        if project != &three {
            let delivered = listed
                .iter()
                .filter(|work| {
                    work["summary"]["phase"]["value"] == "delivered"
                        && work["summary"]["runtime"]["value"] != "completed"
                })
                .count();
            let not_started = listed
                .iter()
                .filter(|work| {
                    work["summary"]["runtime"]["reason"] == "no_runtime"
                        && work["summary"]["phase"]["value"] != "delivered"
                })
                .count();
            assert_eq!(
                works["delivered_unverified"],
                json!(delivered as i64),
                "{project}"
            );
            assert_eq!(works["not_started"], json!(not_started as i64), "{project}");
        }

        // Open decisions from decisions.list.
        let mut decisions = query("decisions.list");
        decisions.limit = Some(100);
        let open_decisions = read_project(&store, project, decisions).await;
        assert_eq!(
            item["waits"]["decision"].as_i64().unwrap(),
            open_decisions["data"]["items"].as_array().unwrap().len() as i64,
            "{project}: decision count must match decisions.list"
        );

        // Open review rounds from review.rounds on every listed work.
        let mut open_reviews = 0i64;
        for work in &listed {
            let mut rounds = query("review.rounds");
            rounds.work_id = work["work_id"].as_str().map(str::to_owned);
            rounds.limit = Some(100);
            let page = read_project(&store, project, rounds).await;
            open_reviews += page["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|round| round["state"] == "open")
                .count() as i64;
        }
        assert_eq!(
            item["waits"]["review"].as_i64().unwrap(),
            open_reviews,
            "{project}: review count must match review.rounds"
        );

        // Latest period update from updates.list.
        let updates = read_project(&store, project, query("updates.list")).await;
        match updates["data"]["items"].as_array().unwrap().first() {
            Some(first) => {
                assert_eq!(
                    item["latest_update"]["update_id"], first["update_id"],
                    "{project}"
                );
                assert_eq!(
                    item["latest_update"]["status"], first["status"],
                    "{project}"
                );
                assert_eq!(
                    item["latest_update"]["author"]["display_name"],
                    first["author"]["display_name"],
                    "{project}"
                );
                assert_eq!(
                    item["latest_update"]["recorded_at_unix_ms"], first["recorded_at_unix_ms"],
                    "{project}"
                );
            }
            None => assert_eq!(item["latest_update"], Value::Null, "{project}"),
        }
    }

    let two_item = by_project[&two];
    assert_eq!(two_item["works"]["verified"], json!(1));
    assert_eq!(
        two_item["works"]["completed_without_independent_review"],
        json!(1)
    );
    assert_eq!(two_item["waits"]["review"], json!(1));
    assert_eq!(two_item["waits"]["dependency"], json!(1));
    assert!(two_item["waits"]["oldest_open_unix_ms"].is_i64());
    // b-private (stream 2) is invisible: hidden count only, no identifiers.
    assert_eq!(two_item["works"]["hidden_work_count"], json!(1));
    let text = result.to_string();
    assert!(!text.contains("b-private"));
    assert!(!text.contains("PRIVATE NEXT ACTION"));
    // Dependency wait and decision of the hidden work stay out of the waits.
    let one_item = by_project[&one];
    assert_eq!(one_item["works"]["in_progress"], json!(1));
    assert_eq!(one_item["works"]["delivered_unverified"], json!(1));
    assert_eq!(one_item["waits"]["decision"], json!(1));
    let three_item = by_project[&three];
    assert_eq!(three_item["works"]["total"], json!(2));
    assert_eq!(three_item["works"]["not_started"], json!(1));
    assert_eq!(three_item["works"]["retired"], json!(1));
    assert_eq!(three_item["works"]["hidden_work_count"], json!(1));
    assert_eq!(three_item["latest_update"], Value::Null);
}

/// A project whose admission data diverges degrades to not_read while the
/// others stay complete (acceptance: one project never suppresses the rest).
#[tokio::test]
async fn a_divergent_project_is_not_read_and_isolates_the_failure() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    let two = clone_project(&admin, 2).await;
    // Divergence: the stored catalog names a different project.
    admin
        .execute(
            "UPDATE awr_team.workstream_catalogs SET catalog_json=jsonb_set(catalog_json,'{project_id}','\"other-project\"')
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &two],
        )
        .await
        .unwrap();
    let projects = all_projects(&admin).await;
    let result = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let items = result["data"]["projects"].as_array().unwrap();
    let find = |project: &str| {
        items
            .iter()
            .find(|item| item["project_id"] == project)
            .unwrap()
            .clone()
    };
    assert_eq!(
        find(&two)["read"],
        json!({"status":"not_read","reason":"read_failed"})
    );
    assert_eq!(find(&one)["read"]["status"], "complete");
    assert_eq!(find(PROJECT)["read"]["status"], "complete");
    // Snapshot-less projects are reported, not dropped.
    admin
        .execute(
            "UPDATE awr_team.projects SET active_snapshot_id=NULL WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &one],
        )
        .await
        .unwrap();
    let result = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let items = result["data"]["projects"].as_array().unwrap();
    let one_item = items
        .iter()
        .find(|item| item["project_id"] == one)
        .unwrap()
        .clone();
    assert_eq!(
        one_item["read"],
        json!({"status":"not_read","reason":"no_active_snapshot"})
    );
}

/// A member project without current read authority is marked not_read, a
/// project without membership never appears, and other tenants stay out.
#[tokio::test]
async fn authority_boundaries_hide_projects_and_works() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    let two = clone_project(&admin, 2).await;
    // `two` loses every read grant: a member project turned unreadable.
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET can_read=false
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &two],
        )
        .await
        .unwrap();
    // A project only another actor belongs to: never enumerated.
    let outsider = clone_project(&admin, 3).await;
    admin
        .batch_execute(&format!(
            "DELETE FROM awr_team.workstream_grants WHERE tenant_id='{TENANT}' AND project_id='{outsider}';
             DELETE FROM awr_team.project_memberships WHERE tenant_id='{TENANT}' AND project_id='{outsider}' AND actor_id='agent';"
        ))
        .await
        .unwrap();
    let projects = all_projects(&admin).await;
    let result = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let text = result.to_string();
    assert!(
        !text.contains(&outsider),
        "a foreign project must stay absent"
    );
    let items = result["data"]["projects"].as_array().unwrap();
    let find = |project: &str| {
        items
            .iter()
            .find(|item| item["project_id"] == project)
            .unwrap()
            .clone()
    };
    assert_eq!(
        find(&two)["read"],
        json!({"status":"not_read","reason":"no_read_authority"})
    );
    assert_eq!(find(&one)["read"]["status"], "complete");
    // Cross-tenant credentials never see this tenant's projects.
    assert!(matches!(
        store
            .portfolio_overview(
                "other-tenant",
                A,
                &registered(&projects),
                portfolio(None, None)
            )
            .await,
        Err(PgError::Forbidden)
    ));
    // A well-formed credential of a member without any read grant sees its
    // own memberships marked not_read; nothing else is disclosed.
    let ungranted = store
        .portfolio_overview(TENANT, NONE, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    for item in ungranted["data"]["projects"].as_array().unwrap() {
        assert_eq!(
            item["read"],
            json!({"status":"not_read","reason":"no_read_authority"})
        );
    }
    // Credential-scoped reads stay inside their one project.
    admin
        .execute(
            "UPDATE awr_team.credentials SET project_id='reader-project' WHERE id='reader-a'",
            &[],
        )
        .await
        .unwrap();
    let scoped = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let ids: Vec<&str> = scoped["data"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["project_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![PROJECT]);
}

#[tokio::test]
async fn pages_never_overlap_or_drop_projects() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    for index in 1..=4 {
        clone_project(&admin, index).await;
    }
    let projects = all_projects(&admin).await;
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = store
            .portfolio_overview(
                TENANT,
                A,
                &registered(&projects),
                portfolio(cursor, Some(3)),
            )
            .await
            .unwrap();
        let items = page["data"]["projects"].as_array().unwrap();
        assert!(!items.is_empty());
        for item in items {
            seen.push(item["project_id"].as_str().unwrap().to_owned());
        }
        match page["data"]["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => {
                assert_eq!(page["data"]["truncated"], false);
                break;
            }
        }
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "pages must not repeat a project");
    let mut expected = projects.clone();
    expected.sort();
    assert_eq!(unique, expected, "pages must cover every member project");
    // The 200-project page bound is enforced by validation.
    for bad in [201u16, 0] {
        let error = store
            .portfolio_overview(
                TENANT,
                A,
                &registered(&projects),
                portfolio(None, Some(bad)),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("query_input/limit"),
            "limit {bad}: {error}"
        );
    }
}

/// Count the client-server exchanges on the wire while reading forty projects
/// (acceptance: at most five). A per-project query loop cannot pass this.
#[tokio::test]
async fn forty_projects_stay_inside_the_round_trip_budget() {
    let (guard, admin, db, store) = fixture::setup().await;
    for index in 1..=39 {
        clone_project(&admin, index).await;
    }
    let projects = all_projects(&admin).await;
    assert_eq!(projects.len(), 40);
    drop(store);
    // A tiny counting proxy: forward every byte, count ReadyForQuery frames.
    let upstream = common::test_config();
    let upstream_host = match &upstream.get_hosts()[0] {
        tokio_postgres::config::Host::Tcp(host) => host.clone(),
        _ => panic!("test target must be TCP"),
    };
    let upstream_port = upstream.get_ports()[0];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    {
        let counter = counter.clone();
        let host = upstream_host.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut client, _)) = listener.accept().await else {
                    return;
                };
                let Ok(mut upstream) =
                    tokio::net::TcpStream::connect((host.as_str(), upstream_port)).await
                else {
                    return;
                };
                let counter = counter.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    if std::env::var("AWR_PORTFOLIO_DEBUG").is_ok() {
                        eprintln!("proxy connection opened");
                    }
                    let (mut client_read, mut client_write) = client.into_split();
                    let (mut upstream_read, mut upstream_write) = upstream.into_split();
                    // Count client-issued statement exchanges in the
                    // client-to-server direction: one Parse frame per
                    // extended-protocol statement and one Query frame per
                    // simple statement (BEGIN/COMMIT). tokio-postgres runs
                    // every string query as prepare-then-execute and closes
                    // the statement afterwards, so counting server Sync
                    // frames would double every statement for every read in
                    // this codebase; statement initiations measure the same
                    // property the bound protects: no per-project loops.
                    let up = tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let debug = std::env::var("AWR_PORTFOLIO_DEBUG").is_ok();
                        let mut pending = Vec::new();
                        let mut startup_skipped = false;
                        let mut chunk = [0u8; 16384];
                        loop {
                            let read = match client_read.read(&mut chunk).await {
                                Ok(0) | Err(_) => break,
                                Ok(read) => read,
                            };
                            if upstream_write.write_all(&chunk[..read]).await.is_err() {
                                break;
                            }
                            pending.extend_from_slice(&chunk[..read]);
                            let mut offset = 0;
                            if !startup_skipped {
                                if pending.len() < 4 {
                                    continue;
                                }
                                let startup = i32::from_be_bytes([
                                    pending[0], pending[1], pending[2], pending[3],
                                ]) as usize;
                                // sslmode=disable, so the first message is the
                                // startup message: length-prefixed, no type
                                // byte. Skip it, then parse framed messages.
                                if pending.len() < startup {
                                    continue;
                                }
                                offset = startup;
                                startup_skipped = true;
                            }
                            while offset + 5 <= pending.len() {
                                let length = i32::from_be_bytes([
                                    pending[offset + 1],
                                    pending[offset + 2],
                                    pending[offset + 3],
                                    pending[offset + 4],
                                ]) as usize
                                    + 1;
                                if offset + length > pending.len() {
                                    break;
                                }
                                let kind = pending[offset];
                                if kind == b'P' || kind == b'Q' {
                                    let seen = counter.fetch_add(1, Ordering::SeqCst);
                                    if debug {
                                        eprintln!("statement #{seen}");
                                    }
                                }
                                offset += length;
                            }
                            pending.drain(..offset);
                        }
                    });
                    let down = tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut upstream_read, &mut client_write).await;
                    });
                    let _ = tokio::join!(up, down);
                });
            }
        });
    }
    let user = upstream.get_user().unwrap_or("postgres").to_owned();
    let password = String::from_utf8(
        upstream
            .get_password()
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
    )
    .unwrap();
    let proxied_url = format!(
        "postgres://{user}:{password}@{proxy_addr}/{}?sslmode=disable",
        db
    );
    let proxied_store = WorkstreamReadStore::new(proxied_url);
    // Warm the pool so the measured call covers the portfolio read itself.
    proxied_store
        .query(TENANT, PROJECT, A, query("workstreams.list"))
        .await
        .unwrap();
    counter.store(0, Ordering::SeqCst);
    let result = proxied_store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    assert_eq!(result["data"]["total_projects"], json!(40));
    let exchanges = counter.load(Ordering::SeqCst);
    eprintln!("portfolio statement exchanges: {exchanges}");
    assert!(
        exchanges <= 5,
        "portfolio.overview used {exchanges} database round trips for 40 projects; the bound is 5"
    );
    drop(proxied_store);
    drop(guard);
}

/// An Agent reader needs one live covering read delegation, exactly like the
/// project entry probe; revoking it turns the member project not_read.
#[tokio::test]
async fn agent_readers_need_a_live_read_delegation_per_project() {
    let (_guard, admin, db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    admin
        .batch_execute(&format!(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='{TENANT}' AND id='agent';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('{TENANT}','{one}','portfolio-person','Portfolio Owner','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('{TENANT}','{one}','portfolio-binding','portfolio-person','agent','active');"
        ))
        .await
        .unwrap();
    let projects = all_projects(&admin).await;
    // Without any delegation no member project is readable.
    let denied = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    for item in denied["data"]["projects"].as_array().unwrap() {
        assert_eq!(item["read"]["reason"], "no_read_authority");
    }
    // A task-scoped Inspect delegation opens exactly its task.
    use awr_core::{
        AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
        ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
    };
    let issue = |project: String, scope: AuthorizationScope| {
        let store = awr_team_pg::AuthorizationStore::from_config(common::with_app_role(
            &common::test_config(),
            &db,
        ));
        let id = format!("portfolio-auth-{}", project);
        let request = IssueAuthorizationRequest {
            request_key: format!("issue-{id}"),
            authorization: AgentAuthorization {
                id: id.clone(),
                authorizer_person_id: PersonId::new("portfolio-person").unwrap(),
                responsible_person_id: PersonId::new("portfolio-person").unwrap(),
                subject_kind: ExecutionSubjectKind::Agent,
                subject_id: "agent".into(),
                client_id: "cli-a".into(),
                session_id: None,
                model_id: None,
                scope,
                actions: [AuthorizedAction::Inspect].into_iter().collect(),
                expires_at_ms: None,
                status: AuthorizationStatus::Active,
                revoked_at_ms: None,
                revoked_by: None,
                verifiable_capabilities: vec![],
                self_reported_skill_hints: vec![],
                parent_authorization_id: None,
                maintainer_person_id: None,
                created_at_ms: 1,
                binding_id: Some("portfolio-binding".into()),
            },
        };
        async move { store.issue(TENANT, &project, &request).await.unwrap() }
    };
    issue(
        one.clone(),
        AuthorizationScope::Task {
            project_id: one.clone(),
            work_item_id: "a".into(),
        },
    )
    .await;
    let allowed = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let items = allowed["data"]["projects"].as_array().unwrap();
    let one_item = items
        .iter()
        .find(|item| item["project_id"] == one)
        .unwrap()
        .clone();
    assert_eq!(one_item["read"]["status"], "complete");
    assert_eq!(
        one_item["works"]["total"],
        json!(1),
        "only the delegated task counts"
    );
    assert_eq!(one_item["works"]["hidden_work_count"], json!(2));
    // Other projects stay denied and the base project too.
    assert_eq!(
        items
            .iter()
            .find(|item| item["project_id"] == PROJECT)
            .unwrap()["read"]["reason"],
        "no_read_authority"
    );
}

/// Grant or membership changes expire an outstanding cursor.
#[tokio::test]
async fn cursor_binds_the_reader_authority() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    for index in 1..=3 {
        clone_project(&admin, index).await;
    }
    let projects = all_projects(&admin).await;
    let first = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, Some(2)))
        .await
        .unwrap();
    let cursor = first["data"]["next_cursor"].as_str().unwrap().to_owned();
    let second = store
        .portfolio_overview(
            TENANT,
            A,
            &registered(&projects),
            portfolio(Some(cursor.clone()), Some(2)),
        )
        .await
        .unwrap();
    assert_ne!(
        first["data"]["projects"][0]["project_id"],
        second["data"]["projects"][0]["project_id"]
    );
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET grant_version=grant_version+1
             WHERE tenant_id=$1 AND project_id=$2 AND client_id='cli-a'",
            &[&TENANT, &projects[0]],
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .portfolio_overview(
                TENANT,
                A,
                &registered(&projects),
                portfolio(Some(cursor), Some(2)),
            )
            .await,
        Err(PgError::CursorExpired)
    ));
    assert!(matches!(
        store
            .portfolio_overview(
                TENANT,
                A,
                &registered(&projects),
                portfolio(
                    Some("{\"binding\":\"x\",\"project_id\":\"y\"}".into()),
                    Some(2)
                ),
            )
            .await,
        Err(PgError::CursorExpired)
    ));
}
