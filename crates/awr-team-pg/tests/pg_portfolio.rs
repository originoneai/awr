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
    // A fault that ignores the start cursor would loop forever; bound the
    // walk so it fails instead of hanging.
    let max_pages = projects.len() / 3 + 1;
    let mut pages = 0usize;
    loop {
        pages += 1;
        assert!(
            pages <= max_pages,
            "pagination walked {pages} pages for {} projects; the start cursor is ignored",
            projects.len()
        );
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
    // Every Parse frame's SQL text, to pin that the scope markers are bound
    // transaction-locally: a pooled connection must never keep them.
    let statements = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let counter = counter.clone();
        let statements = statements.clone();
        let host = upstream_host.clone();
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    return;
                };
                let Ok(upstream) =
                    tokio::net::TcpStream::connect((host.as_str(), upstream_port)).await
                else {
                    return;
                };
                let counter = counter.clone();
                let statements = statements.clone();
                tokio::spawn(async move {
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
                                if kind == b'P' {
                                    // Parse body: statement name, then the
                                    // query, both NUL-terminated.
                                    let body = &pending[offset + 5..offset + length];
                                    if let Some(name_end) = body.iter().position(|b| *b == 0) {
                                        let rest = &body[name_end + 1..];
                                        if let Some(query_end) = rest.iter().position(|b| *b == 0) {
                                            if let Ok(text) =
                                                std::str::from_utf8(&rest[..query_end])
                                            {
                                                statements.lock().unwrap().push(text.to_owned());
                                            }
                                        }
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
    // The scope markers must be transaction-local: if the statement ever
    // binds them session-wide, a pooled connection keeps them and every
    // later reuse of that connection reads with the portfolio marker on.
    let captured = statements.lock().unwrap().clone();
    let marker_statement = captured
        .iter()
        .find(|text| text.contains("awr.portfolio_read"))
        .expect("the marker statement crossed the proxy");
    assert!(
        marker_statement.contains("set_config('awr.tenant_id',$1,true)")
            && marker_statement.contains("set_config('awr.portfolio_read','on',true)"),
        "markers must be bound with is_local=true: {marker_statement}"
    );
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

/// One work may carry two open review rounds (the table forbids one open
/// wait, not one open round per bundle). The per-work aggregate must not
/// double-count the work, and the review wait carries both rounds.
#[tokio::test]
async fn two_open_review_rounds_on_one_work_stay_one_work() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    seed_review(&admin, &one, "a").await;
    admin
        .execute(
            "INSERT INTO awr_team.review_rounds(tenant_id,project_id,id,work_id,round_index,bundle_hash,contract_hash,author_actor_id,state)
             VALUES($1,$2,'review-second-open','a',2,'bundle-two','contract','agent','open')",
            &[&TENANT, &one],
        )
        .await
        .unwrap();
    let projects = all_projects(&admin).await;
    let result = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let item = by_project_id(&result, &one);
    // work.list over the visible stream counts each work once.
    let streams = read_project(&store, &one, query("workstreams.list")).await;
    let mut listed = 0usize;
    for stream in streams["items"].as_array().unwrap() {
        let mut list = query("work.list");
        list.workstream_id = stream["id"].as_str().and_then(|id| id.parse().ok());
        list.limit = Some(100);
        listed += read_project(&store, &one, list).await["data"]["items"]
            .as_array()
            .unwrap()
            .len();
    }
    assert_eq!(
        item["works"]["total"].as_i64().unwrap() as usize,
        listed,
        "a work with two open rounds is still one work (work.list says {listed})"
    );
    // The review wait counts the rounds, like review.rounds does.
    let mut rounds = query("review.rounds");
    rounds.work_id = Some("a".into());
    rounds.limit = Some(100);
    let open_rounds = read_project(&store, &one, rounds).await["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|round| round["state"] == "open")
        .count();
    assert_eq!(
        item["waits"]["review"].as_i64().unwrap() as usize,
        open_rounds
    );
    assert_eq!(open_rounds, 2);
}

/// A stored payload the reader can reach must never fail the whole read. The
/// acceptance flag may be corrupt text or a non-boolean value: it then reads
/// as "no stated flag"; with no `independence_kind` either, the completion
/// falls through to the policy and stays recorded and unverified. A stated
/// `independence_kind` still decides first, exactly as in the shared rule.
#[tokio::test]
async fn corrupt_completion_payloads_degrade_to_unverified_without_failing_the_read() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    let two = clone_project(&admin, 2).await;
    let three = clone_project(&admin, 3).await;
    let four = clone_project(&admin, 4).await;
    // Corrupt text flag, but the kind alone classifies this completion.
    seed_completion(
        &admin,
        &one,
        "a",
        json!({"receipt_id":"r-text-kind","independence_kind":"team_independent",
            "team_independent_acceptance":"maybe"}),
        11,
    )
    .await;
    // Corrupt text flag and no kind: no stated flag falls through to the
    // policy, which is "recorded, never verified".
    seed_completion(
        &admin,
        &two,
        "a",
        json!({"receipt_id":"r-text-nokind","team_independent_acceptance":"maybe"}),
        12,
    )
    .await;
    // A number flag used to cast to true; it must read as no stated flag too.
    seed_completion(
        &admin,
        &three,
        "a",
        json!({"receipt_id":"r-number-nokind","team_independent_acceptance":1}),
        13,
    )
    .await;
    let projects = all_projects(&admin).await;
    let result = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .expect("one corrupt payload in one project must not fail the whole portfolio");
    let kind_wins = by_project_id(&result, &one);
    assert_eq!(kind_wins["read"]["status"], "complete");
    assert_eq!(kind_wins["works"]["verified"], json!(1));
    for project in [&two, &three] {
        let item = by_project_id(&result, project);
        assert_eq!(item["read"]["status"], "complete", "{project}");
        assert_eq!(
            item["works"]["completed_without_independent_review"],
            json!(1),
            "{project}: a stated flag that is not a boolean is no stated flag"
        );
        assert_eq!(item["works"]["verified"], json!(0), "{project}");
    }
    let untouched = by_project_id(&result, &four);
    assert_eq!(untouched["read"]["status"], "complete");
    assert_eq!(untouched["works"]["not_started"], json!(2));
}

/// Find one project's item in a portfolio reply.
fn by_project_id(result: &Value, project: &str) -> Value {
    result["data"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["project_id"] == project)
        .unwrap_or_else(|| panic!("project {project} absent from portfolio"))
        .clone()
}

/// Project-level disclosure: the directory grouping follows project entry
/// (owner decision of 2026-11-19), so every reader whose item is `complete`
/// sees it; `latest_update` is present exactly when `updates.list` succeeds
/// for the same reader. Five reader kinds, one project, same data.
#[tokio::test]
async fn project_level_fields_follow_their_per_project_reads() {
    use awr_core::{
        AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
        ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
    };
    let (_guard, admin, db, store) = fixture::setup().await;
    // A period record exists, so a reader with update authority receives it.
    seed_update(&admin, PROJECT, "2026-W47", "risk").await;
    // Person + binding for the Agent delegations below.
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('{TENANT}','{PROJECT}','parity-person','Parity Owner','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('{TENANT}','{PROJECT}','parity-binding','parity-person','agent','active');"
        ))
        .await
        .unwrap();
    let projects = all_projects(&admin).await;
    let issuer = awr_team_pg::AuthorizationStore::from_config(common::with_app_role(
        &common::test_config(),
        &db,
    ));
    let issue = |id: &str, scope: AuthorizationScope| {
        let issuer = &issuer;
        let request_key = format!("issue-{id}");
        let authorization = AgentAuthorization {
            id: id.into(),
            authorizer_person_id: PersonId::new("parity-person").unwrap(),
            responsible_person_id: PersonId::new("parity-person").unwrap(),
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
            binding_id: Some("parity-binding".into()),
        };
        async move {
            issuer
                .issue(
                    TENANT,
                    PROJECT,
                    &IssueAuthorizationRequest {
                        request_key,
                        authorization,
                    },
                )
                .await
                .unwrap();
        }
    };
    let stream_of = |n: i64| format!("{:026}", n);
    async fn check(store: &WorkstreamReadStore, projects: &[String], name: &str) -> bool {
        {
            let portfolio = store
                .portfolio_overview(TENANT, A, &registered(projects), portfolio(None, None))
                .await
                .unwrap();
            let item = by_project_id(&portfolio, PROJECT);
            assert_eq!(item["read"]["status"], "complete", "{name}");
            let updates_read = read_project(store, PROJECT, query("updates.list")).await;
            let updates_ok = updates_read
                .get("data")
                .and_then(|d| d.get("items"))
                .is_some();
            // The grouping labels follow project entry, whatever the entry
            // probe of directory.read itself would say for this reader.
            assert!(
                item["directory"].is_object(),
                "{name}: every complete item carries the directory grouping"
            );
            assert_eq!(
                item["latest_update"].is_null(),
                !updates_ok,
                "{name}: latest_update presence must equal updates.list"
            );
            if updates_ok {
                assert_eq!(
                    item["latest_update"]["summary"], updates_read["data"]["items"][0]["summary"],
                    "{name}: the supervisor's sentence travels with the record"
                );
            }
            updates_ok
        }
    }

    // 1. Partial human: stream 1 only. directory.read itself refuses, but
    //    the grouping labels follow project entry; updates.list succeeds.
    assert!(store.directory().read(TENANT, PROJECT, A).await.is_err());
    let updates = check(&store, &projects, "partial human").await;
    assert!(updates);
    // 2. Full-scope human: every stream of the catalog readable.
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,active,grant_version)
             VALUES($1,$2,'agent','cli-a',$3,1,true,true,1)",
            &[&TENANT, &PROJECT, &stream_of(2)],
        )
        .await
        .unwrap();
    let updates = check(&store, &projects, "full human").await;
    assert!(updates);
    admin
        .execute(
            "DELETE FROM awr_team.workstream_grants WHERE tenant_id=$1 AND project_id=$2
             AND actor_id='agent' AND client_id='cli-a' AND workstream_id=$3",
            &[&TENANT, &PROJECT, &stream_of(2)],
        )
        .await
        .unwrap();
    // Agent readers keep the full-catalog grants for cases 3-5.
    admin
        .batch_execute(&format!(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='{TENANT}' AND id='agent';
             INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,active,grant_version)
             VALUES('{TENANT}','{PROJECT}','agent','cli-a','{stream}',1,true,true,1);",
            stream = stream_of(2)
        ))
        .await
        .unwrap();
    // 3. Project-scope Agent: directory and updates both readable.
    issue(
        "parity-project",
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
    )
    .await;
    let updates = check(&store, &projects, "project agent").await;
    assert!(updates);
    // 4. Workstream-scope Agent: updates yes, directory no (no Project scope).
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{status}','\"revoked\"')
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    issue(
        "parity-stream",
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: stream_of(1),
        },
    )
    .await;
    let updates = check(&store, &projects, "workstream agent").await;
    assert!(updates);
    // 5. Task-only Agent: the project reads through its task; neither
    //    project-level field is disclosed.
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{status}','\"revoked\"')
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    issue(
        "parity-task",
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
    )
    .await;
    let portfolio = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let item = by_project_id(&portfolio, PROJECT);
    assert_eq!(item["read"]["status"], "complete");
    // A Task-only reader still sees the grouping labels (owner decision),
    // but no period update: updates.list refuses for it.
    assert!(
        item["directory"].is_object(),
        "task agent: the grouping labels follow project entry"
    );
    assert_eq!(item["latest_update"], Value::Null, "task agent: no update");
    assert_eq!(item["works"]["total"], json!(1), "only the task counts");
}

/// A credential id with the wrong secret is refused: the secret hash is part
/// of admission, not only the id. The token keeps the valid id and format.
#[tokio::test]
async fn a_wrong_secret_for_a_known_credential_id_is_refused() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    clone_project(&admin, 1).await;
    let wrong_secret = format!("awr1.reader-a.{}", "d".repeat(64));
    assert!(matches!(
        store
            .portfolio_overview(
                TENANT,
                &wrong_secret,
                &registered(&all_projects(&admin).await),
                portfolio(None, None)
            )
            .await,
        Err(PgError::Forbidden)
    ));
    // The same call with the real secret still succeeds.
    store
        .portfolio_overview(
            TENANT,
            A,
            &registered(&all_projects(&admin).await),
            portfolio(None, None),
        )
        .await
        .unwrap();
}

/// Credential, actor and tenant level states refuse the whole portfolio
/// exactly as they refuse the per-project entry.
#[tokio::test]
async fn credential_actor_and_tenant_states_refuse_the_whole_portfolio() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    clone_project(&admin, 1).await;
    let projects = all_projects(&admin).await;
    let steps: &[(&str, &str, &str)] = &[
        (
            "revoked credential",
            "UPDATE awr_team.credentials SET revoked_at=now() WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'",
        ),
        (
            "expired credential",
            "UPDATE awr_team.credentials SET expires_at=now()-interval '1 hour' WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='reader-a'",
        ),
        (
            "disabled actor",
            "UPDATE awr_team.actors SET status='disabled' WHERE tenant_id='reader-tenant' AND id='agent'",
            "UPDATE awr_team.actors SET status='active' WHERE tenant_id='reader-tenant' AND id='agent'",
        ),
        (
            "disabled tenant",
            "UPDATE awr_team.tenants SET status='disabled' WHERE id='reader-tenant'",
            "UPDATE awr_team.tenants SET status='active' WHERE id='reader-tenant'",
        ),
        (
            "membership removed everywhere",
            "DELETE FROM awr_team.workstream_grants WHERE actor_id='agent'; DELETE FROM awr_team.project_memberships WHERE actor_id='agent'",
            "INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
             SELECT tenant_id,id,'agent','admin' FROM awr_team.projects WHERE tenant_id='reader-tenant';
             INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,active,grant_version)
             SELECT c.tenant_id,c.project_id,'agent','cli-a',ws->>'id',1,true,true,1
             FROM awr_team.workstream_catalogs c
             JOIN LATERAL jsonb_array_elements(c.catalog_json->'workstreams') ws ON true
             WHERE c.tenant_id='reader-tenant'
               AND c.snapshot_id=(SELECT active_snapshot_id FROM awr_team.projects p
                                  WHERE p.tenant_id=c.tenant_id AND p.id=c.project_id);",
        ),
    ];
    for (name, apply, restore) in steps {
        admin.batch_execute(apply).await.unwrap();
        let entry = store.authorize_project_entry(TENANT, PROJECT, A).await;
        let whole = store
            .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
            .await;
        assert!(
            whole.is_err(),
            "{name}: the portfolio must refuse like the entry does ({entry:?})"
        );
        admin.batch_execute(restore).await.unwrap();
    }
    // Restored: the call works again.
    store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
}

/// The registered-project filter is the outer bound: an unregistered member
/// project stays absent and out of the totals.
#[tokio::test]
async fn an_unregistered_member_project_stays_absent() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    let two = clone_project(&admin, 2).await;
    let subset = vec![PROJECT.to_owned(), one];
    let result = store
        .portfolio_overview(TENANT, A, &registered(&subset), portfolio(None, None))
        .await
        .unwrap();
    let ids: Vec<&str> = result["data"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["project_id"].as_str().unwrap())
        .collect();
    assert!(
        !result.to_string().contains(&two),
        "unregistered project leaked"
    );
    assert_eq!(ids.len(), 2);
    assert_eq!(result["data"]["total_projects"], json!(2));
}

/// The verified classification follows the selected completion, not the
/// newest event: an older verified event with a newer selected unverified
/// completion reads unverified, and the reverse reads verified.
#[tokio::test]
async fn the_selected_completion_decides_not_the_newest_event() {
    let (_guard, admin, _db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    let two = clone_project(&admin, 2).await;
    // Project one: an older verified event, a newer selected unverified one.
    seed_completion(
        &admin,
        &one,
        "a",
        json!({"receipt_id":"older-verified","independence_kind":"team_independent"}),
        10,
    )
    .await;
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,
                contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,
                policy,approved_by_json,independence_kind,author_actor_id,final_submitter_actor_id)
             VALUES('{TENANT}','{one}','newer-plain','a','main','contract','result','deps','evidence',
                'review','{{}}','ordinary_confirm','agent','agent');
             INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json)
             VALUES('{TENANT}','{one}','event-newer-plain',11,0,'work.completed','agent','a',
                '{{\"receipt_id\":\"newer-plain\",\"independence_kind\":\"ordinary_confirm\"}}'::jsonb);"
        ))
        .await
        .unwrap();
    seed_runtime(&admin, &one, "a", "completed", Some("newer-plain")).await;
    // Project two: the selected receipt is the older verified one, and a
    // newer unselected plain event also exists — newest-event-wins would
    // misread this work as unverified.
    seed_completion(
        &admin,
        &two,
        "a",
        json!({"receipt_id":"older-verified","independence_kind":"team_independent"}),
        10,
    )
    .await;
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,
                contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,
                policy,approved_by_json,independence_kind,author_actor_id,final_submitter_actor_id)
             VALUES('{TENANT}','{two}','newer-plain','a','main','contract','result','deps','evidence',
                'review','{{}}','ordinary_confirm','agent','agent');
             INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json)
             VALUES('{TENANT}','{two}','event-newer-plain',11,0,'work.completed','agent','a',
                '{{\"receipt_id\":\"newer-plain\",\"independence_kind\":\"ordinary_confirm\"}}'::jsonb);"
        ))
        .await
        .unwrap();
    // Project three: the selected receipt has no matching event at all; an
    // older verified event exists but belongs to another receipt. With no
    // basis for the selected completion the work stays unverified, and
    // any-event-wins would misread it.
    let three = clone_project(&admin, 3).await;
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,
                contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,
                policy,approved_by_json,independence_kind,author_actor_id,final_submitter_actor_id)
             VALUES('{TENANT}','{three}','orphan-receipt','a','main','contract','result','deps','evidence',
                'review','{{}}',NULL,'agent','agent');
             INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json)
             VALUES('{TENANT}','{three}','event-other-receipt',10,0,'work.completed','agent','a',
                '{{\"receipt_id\":\"other-receipt\",\"independence_kind\":\"team_independent\"}}'::jsonb);
             INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,selected_completion_id)
             VALUES('{TENANT}','{three}','main','a','completed',1,0,'orphan-receipt');"
        ))
        .await
        .unwrap();
    let projects = all_projects(&admin).await;
    let result = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let one_item = by_project_id(&result, &one);
    assert_eq!(
        one_item["works"]["verified"],
        json!(0),
        "selected plain wins"
    );
    assert_eq!(
        one_item["works"]["completed_without_independent_review"],
        json!(1)
    );
    let two_item = by_project_id(&result, &two);
    assert_eq!(
        two_item["works"]["verified"],
        json!(1),
        "selected verified wins"
    );
    assert_eq!(
        two_item["works"]["completed_without_independent_review"],
        json!(0)
    );
    let three_item = by_project_id(&result, &three);
    assert_eq!(
        three_item["works"]["completed_without_independent_review"],
        json!(1),
        "a selected receipt with no event is recorded, never verified"
    );
    assert_eq!(three_item["works"]["verified"], json!(0));
}

/// An Agent's outstanding cursor expires when its delegation scope changes,
/// not only when grant versions move: the cursor binds the admitted streams
/// and task allowances.
#[tokio::test]
async fn an_agent_cursor_expires_when_its_delegation_scope_changes() {
    use awr_core::{
        AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
        ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
    };
    let (_guard, admin, db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    admin
        .batch_execute(&format!(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='{TENANT}' AND id='agent';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('{TENANT}','{one}','scope-person','Scope Owner','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('{TENANT}','{one}','scope-binding','scope-person','agent','active');"
        ))
        .await
        .unwrap();
    let issuer = awr_team_pg::AuthorizationStore::from_config(common::with_app_role(
        &common::test_config(),
        &db,
    ));
    async fn issue_stream(
        issuer: &awr_team_pg::AuthorizationStore,
        project: &str,
        scope: AuthorizationScope,
    ) {
        issuer
            .issue(
                TENANT,
                project,
                &IssueAuthorizationRequest {
                    request_key: format!("issue-scope-{project}"),
                    authorization: AgentAuthorization {
                        id: format!("scope-{project}"),
                        authorizer_person_id: PersonId::new("scope-person").unwrap(),
                        responsible_person_id: PersonId::new("scope-person").unwrap(),
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
                        binding_id: Some("scope-binding".into()),
                    },
                },
            )
            .await
            .unwrap();
    }
    issue_stream(
        &issuer,
        &one,
        AuthorizationScope::Workstream {
            project_id: one.clone(),
            workstream_id: format!("{:026}", 1),
        },
    )
    .await;
    let projects = all_projects(&admin).await;
    let first = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, Some(1)))
        .await
        .unwrap();
    let cursor = first["data"]["next_cursor"].as_str().unwrap().to_owned();
    store
        .portfolio_overview(
            TENANT,
            A,
            &registered(&projects),
            portfolio(Some(cursor.clone()), Some(1)),
        )
        .await
        .unwrap();
    // Reshape the only read delegation from stream scope to task scope and
    // back off the stream: the admitted streams, tasks and (through the
    // early denial) grant facts all move, and the outstanding cursor must
    // expire on that alone — no grant version moved.
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(jsonb_set(body_json,'{scope}',
                $3::jsonb),'{status}','\"revoked\"')
             WHERE tenant_id=$1 AND project_id=$2",
            &[
                &TENANT,
                &one,
                &json!({"kind":"task","project_id":one.clone(),"work_item_id":"a"}),
            ],
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .portfolio_overview(
                TENANT,
                A,
                &registered(&projects),
                portfolio(Some(cursor), Some(1))
            )
            .await,
        Err(PgError::CursorExpired)
    ));
}

/// The statements filter by tenant themselves, not only through row-level
/// policies: a second tenant holding a project, a credential and a snapshot
/// with the same ids cannot appear in this tenant's portfolio.
#[tokio::test]
async fn statement_tenant_filters_hold_beyond_row_level_policies() {
    let (_guard, admin, db, store) = fixture::setup().await;
    let one = clone_project(&admin, 1).await;
    // A whole second tenant with colliding ids: project, credential id and
    // snapshot id all match the first tenant's.
    let hash = awr_team_pg::workstream_credential_hash(A).unwrap();
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES('mirror-tenant','Mirror','active');
             INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
             VALUES('mirror-tenant','agent','human','Mirror Worker','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status,active_snapshot_id,project_revision)
             VALUES('mirror-tenant','{PROJECT}','p','team','epoch-mirror','active','snapshot-portfolio-001',3);
             UPDATE awr_team.workstream_modes SET enabled=true WHERE tenant_id='mirror-tenant' AND project_id='{PROJECT}';
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
             VALUES('mirror-tenant','{PROJECT}','agent','admin');
             INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash,project_id)
             VALUES('mirror-tenant','reader-a','agent','cli-a','{hash}','{PROJECT}');
             INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,active,grant_version)
             SELECT 'mirror-tenant','{PROJECT}','agent','cli-a',workstream_id,authority_version,true,true,1
             FROM awr_team.workstream_grants WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}';
             INSERT INTO awr_team.source_snapshots(tenant_id,project_id,id,manifest_digest,source_ref_json,parser_version,created_by)
             SELECT 'mirror-tenant','{PROJECT}','snapshot-portfolio-001',manifest_digest,source_ref_json,parser_version,created_by
             FROM awr_team.source_snapshots WHERE tenant_id='{TENANT}' AND project_id='{one}';
             INSERT INTO awr_team.workstream_catalogs(tenant_id,project_id,snapshot_id,catalog_json,projection_hash)
             SELECT 'mirror-tenant','{PROJECT}','snapshot-portfolio-001',
                jsonb_set(jsonb_set(catalog_json,'{{project_id}}',to_jsonb('{PROJECT}'::text)),
                    '{{workstreams}}',
                    (SELECT jsonb_agg(jsonb_set(ws,'{{project_id}}',to_jsonb('{PROJECT}'::text)))
                     FROM jsonb_array_elements(catalog_json->'workstreams') ws)),
                projection_hash
             FROM awr_team.workstream_catalogs WHERE tenant_id='{TENANT}' AND project_id='{one}'
               AND snapshot_id='snapshot-portfolio-001';"
        ))
        .await
        .unwrap();
    let projects = all_projects(&admin).await;
    assert_eq!(
        projects.len(),
        2,
        "the mirror project is not in this tenant's list"
    );
    let result = store
        .portfolio_overview(TENANT, A, &registered(&projects), portfolio(None, None))
        .await
        .unwrap();
    let text = result.to_string();
    assert!(
        !text.contains("mirror-tenant"),
        "the mirror tenant must stay absent"
    );
    assert_eq!(result["data"]["total_projects"], json!(2));
    // And the mirror credential, same id and secret in the other tenant,
    // reads only the mirror tenant through its own transport binding.
    let mirror_store =
        WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mirror = mirror_store
        .portfolio_overview(
            "mirror-tenant",
            A,
            &vec![PROJECT.to_owned()],
            portfolio(None, None),
        )
        .await
        .unwrap();
    assert_eq!(mirror["data"]["total_projects"], json!(1));
}

/// The batched admission agrees with `authorize_project_entry` across a
/// matrix of delegation and admission states: every disagreement between the
/// re-implemented rule and the entry probe is a divergence.
#[tokio::test]
async fn batched_admission_matches_the_entry_probe_across_states() {
    use awr_core::{
        AgentAuthorization, AuthorizationScope, AuthorizedAction, DelegateAuthorizationRequest,
        IssueAuthorizationRequest, PersonId,
    };
    let (_guard, admin, db, store) = fixture::setup().await;
    // Human states first: the actor turns into an Agent only afterwards.
    let human_states = [
        ("human_control", "SELECT 1"),
        (
            "human_mode_disabled",
            "UPDATE awr_team.workstream_modes SET enabled=false WHERE tenant_id='{TENANT}' AND project_id='{project}'",
        ),
        (
            "human_grants_cannot_read",
            "UPDATE awr_team.workstream_grants SET can_read=false WHERE tenant_id='{TENANT}' AND project_id='{project}'",
        ),
        (
            "human_grants_inactive",
            "UPDATE awr_team.workstream_grants SET active=false WHERE tenant_id='{TENANT}' AND project_id='{project}'",
        ),
        (
            "human_no_grants",
            "DELETE FROM awr_team.workstream_grants WHERE tenant_id='{TENANT}' AND project_id='{project}'",
        ),
        (
            "human_snapshot_null",
            "UPDATE awr_team.projects SET active_snapshot_id=NULL WHERE tenant_id='{TENANT}' AND id='{project}'",
        ),
    ];
    let mut scenarios: Vec<(String, String)> = Vec::new();
    for (index, (name, sql)) in human_states.iter().enumerate() {
        let project = clone_project(&admin, index + 1).await;
        admin
            .batch_execute(
                &sql.replace("{TENANT}", TENANT)
                    .replace("{project}", &project),
            )
            .await
            .unwrap();
        scenarios.push(((*name).to_owned(), project));
    }
    // Compare the human states while the actor is still human: the batched
    // admission and the entry probe must agree on every project.
    {
        let all = all_projects(&admin).await;
        let result = store
            .portfolio_overview(TENANT, A, &registered(&all), portfolio(None, None))
            .await
            .unwrap();
        let mut mismatches = Vec::new();
        for (name, project) in &scenarios {
            let entry_ok = store
                .authorize_project_entry(TENANT, project, A)
                .await
                .is_ok();
            let item = by_project_id(&result, project);
            let complete = item["read"]["status"] == "complete";
            if entry_ok != complete {
                mismatches.push(format!(
                    "{name}: entry passes={entry_ok}, portfolio={}",
                    item["read"]
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "human states diverged: {mismatches:#?}"
        );
    }
    // Agent states: one cloned project per delegation shape.
    admin
        .batch_execute(&format!(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='{TENANT}' AND id='agent';"
        ))
        .await
        .unwrap();
    let mut agent_scenarios: Vec<(String, String)> = Vec::new();
    let agent_states = [
        "agent_project_scope",
        "agent_workstream_1",
        "agent_workstream_2_ungranted",
        "agent_task_a_stream1",
        "agent_task_b_private_ungranted",
        "agent_expired_in_body",
        "agent_revoked_in_body",
        "agent_binding_disabled",
        "agent_person_disabled",
        "agent_person_departed",
        "agent_wrong_client",
        "agent_session_bound",
        "agent_claim_only_no_read",
        "agent_narrowed_parent_child_unreadable_task",
        "agent_ws2_plus_task_a",
        "agent_responsible_person_not_bound",
        "agent_no_binding_id",
        "agent_binding_other_agent",
        "agent_workstream_1_plus_task_b_private",
    ];
    let stream_of = |n: i64| format!("{:026}", n);
    let issuer = awr_team_pg::AuthorizationStore::from_config(common::with_app_role(
        &common::test_config(),
        &db,
    ));
    for (offset, name) in agent_states.iter().enumerate() {
        let project = clone_project(&admin, human_states.len() + offset + 1).await;
        admin
            .batch_execute(&format!(
                "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
                 VALUES('{TENANT}','{project}','matrix-person','Matrix Owner','active'),
                       ('{TENANT}','{project}','matrix-other','Other Owner','active');
                 INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
                 VALUES('{TENANT}','{project}','matrix-binding','matrix-person','agent','active');"
            ))
            .await
            .unwrap();
        let id = format!("matrix-{offset}");
        let pscope = AuthorizationScope::Project {
            project_id: project.clone(),
        };
        let ws = |n: i64| AuthorizationScope::Workstream {
            project_id: project.clone(),
            workstream_id: stream_of(n),
        };
        let task = |work: &str| AuthorizationScope::Task {
            project_id: project.clone(),
            work_item_id: work.into(),
        };
        let inspect = [AuthorizedAction::Inspect];
        let issue = |auth: AgentAuthorization| {
            let issuer = &issuer;
            let project = project.clone();
            async move {
                let auth_id = auth.id.clone();
                issuer
                    .issue(
                        TENANT,
                        &project,
                        &IssueAuthorizationRequest {
                            request_key: format!("issue-{}", auth.id),
                            authorization: auth,
                        },
                    )
                    .await
                    .unwrap_or_else(|error| panic!("issue {auth_id} failed: {error:?}"));
            }
        };
        match *name {
            "agent_project_scope" => {
                let auth = grant_for(&id, pscope.clone(), &inspect);
                issue(auth).await;
            }
            "agent_workstream_1" => {
                let auth = grant_for(&id, ws(1), &inspect);
                issue(auth).await;
            }
            "agent_workstream_2_ungranted" => {
                let auth = grant_for(&id, ws(2), &inspect);
                issue(auth).await;
            }
            "agent_task_a_stream1" => {
                let auth = grant_for(&id, task("a"), &inspect);
                issue(auth).await;
            }
            "agent_task_b_private_ungranted" => {
                let auth = grant_for(&id, task("b-private"), &inspect);
                issue(auth).await;
            }
            "agent_expired_in_body" => {
                let auth = grant_for(&id, pscope.clone(), &inspect);
                issue(auth).await;
                admin
                    .execute(
                        "UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(jsonb_set(body_json,'{expires_at_ms}','2'),'{created_at_ms}','1')
                         WHERE tenant_id=$1 AND project_id=$2",
                        &[&TENANT, &project],
                    )
                    .await
                    .unwrap();
            }
            "agent_revoked_in_body" => {
                let auth = grant_for(&id, pscope.clone(), &inspect);
                issue(auth).await;
                admin
                    .execute(
                        "UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{status}','\"revoked\"')
                         WHERE tenant_id=$1 AND project_id=$2",
                        &[&TENANT, &project],
                    )
                    .await
                    .unwrap();
            }
            "agent_binding_disabled" => {
                let auth = grant_for(&id, pscope.clone(), &inspect);
                issue(auth).await;
                admin
                    .execute(
                        "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE tenant_id=$1 AND project_id=$2",
                        &[&TENANT, &project],
                    )
                    .await
                    .unwrap();
            }
            "agent_person_disabled" => {
                let auth = grant_for(&id, pscope.clone(), &inspect);
                issue(auth).await;
                admin
                    .execute(
                        "UPDATE awr_team.persons SET status='disabled' WHERE tenant_id=$1 AND project_id=$2 AND id='matrix-person'",
                        &[&TENANT, &project],
                    )
                    .await
                    .unwrap();
            }
            "agent_person_departed" => {
                let auth = grant_for(&id, pscope.clone(), &inspect);
                issue(auth).await;
                admin
                    .execute(
                        "UPDATE awr_team.persons SET status='departed' WHERE tenant_id=$1 AND project_id=$2 AND id='matrix-person'",
                        &[&TENANT, &project],
                    )
                    .await
                    .unwrap();
            }
            "agent_wrong_client" => {
                let mut auth = grant_for(&id, pscope.clone(), &inspect);
                auth.client_id = "cli-b".into();
                issue(auth).await;
            }
            "agent_session_bound" => {
                let mut auth = grant_for(&id, pscope.clone(), &inspect);
                auth.session_id = Some("some-session".into());
                issue(auth).await;
            }
            "agent_claim_only_no_read" => {
                let auth = grant_for(
                    &id,
                    pscope.clone(),
                    &[AuthorizedAction::AcceptResponsibility],
                );
                issue(auth).await;
            }
            "agent_narrowed_parent_child_unreadable_task" => {
                let parent = grant_for(
                    &format!("{id}-parent"),
                    pscope.clone(),
                    &[AuthorizedAction::Inspect, AuthorizedAction::StartWork],
                );
                issue(parent).await;
                let child = grant_for(&id, task("b-private"), &inspect);
                issuer
                    .delegate(
                        TENANT,
                        &project,
                        &DelegateAuthorizationRequest {
                            request_key: format!("delegate-{id}"),
                            parent_authorization_id: format!("{id}-parent"),
                            child,
                        },
                        10,
                    )
                    .await
                    .unwrap();
            }
            "agent_ws2_plus_task_a" => {
                let ws2 = grant_for(&format!("{id}-ws2"), ws(2), &inspect);
                issue(ws2).await;
                let ta = grant_for(&format!("{id}-ta"), task("a"), &inspect);
                issue(ta).await;
            }
            "agent_responsible_person_not_bound" => {
                let mut auth = grant_for(&id, pscope.clone(), &inspect);
                auth.responsible_person_id = PersonId::new("matrix-other").unwrap();
                auth.authorizer_person_id = PersonId::new("matrix-other").unwrap();
                issue(auth).await;
            }
            "agent_no_binding_id" => {
                // The store refuses to issue an authorization without a
                // person binding; seed the state directly, as a legacy row.
                let auth = grant_for(&id, pscope.clone(), &inspect);
                issue(auth).await;
                admin
                    .execute(
                        "UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{binding_id}','null'::jsonb)
                         WHERE tenant_id=$1 AND project_id=$2",
                        &[&TENANT, &project],
                    )
                    .await
                    .unwrap();
            }
            "agent_binding_other_agent" => {
                let auth = grant_for(&id, pscope.clone(), &inspect);
                issue(auth).await;
                admin
                    .batch_execute(&format!(
                        "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='{TENANT}' AND id='reviewer';
                         UPDATE awr_team.person_agent_bindings SET agent_id='reviewer' WHERE tenant_id='{TENANT}' AND project_id='{project}';"
                    ))
                    .await
                    .unwrap();
            }
            "agent_workstream_1_plus_task_b_private" => {
                let ws1 = grant_for(&format!("{id}-ws1"), ws(1), &inspect);
                issue(ws1).await;
                let tb = grant_for(&format!("{id}-tb"), task("b-private"), &inspect);
                issue(tb).await;
            }
            other => panic!("unknown scenario {other}"),
        }
        agent_scenarios.push(((*name).to_owned(), project));
    }
    let all = all_projects(&admin).await;
    let result = store
        .portfolio_overview(TENANT, A, &registered(&all), portfolio(None, None))
        .await
        .unwrap();
    let mut mismatches = Vec::new();
    for (name, project) in &agent_scenarios {
        let entry_ok = store
            .authorize_project_entry(TENANT, project, A)
            .await
            .is_ok();
        let item = by_project_id(&result, project);
        let complete = item["read"]["status"] == "complete";
        if entry_ok != complete {
            mismatches.push(format!(
                "{name}: entry passes={entry_ok}, portfolio={}",
                item["read"]
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "admission diverged from the entry probe: {mismatches:#?}"
    );
    // Spot checks on the states the entry and the portfolio must both refuse.
    // The human states keep their human meaning only before the actor turned
    // into an Agent, so re-probe them through the human-phase assertions
    // above; these names come from both phases.
    for refused in [
        "agent_task_b_private_ungranted",
        "agent_expired_in_body",
        "agent_binding_disabled",
        "agent_wrong_client",
        "agent_claim_only_no_read",
        "agent_narrowed_parent_child_unreadable_task",
        "human_mode_disabled",
        "human_grants_cannot_read",
        "human_grants_inactive",
        "human_no_grants",
        "human_snapshot_null",
    ] {
        let project = &scenarios
            .iter()
            .chain(agent_scenarios.iter())
            .find(|(name, _)| name == refused)
            .unwrap()
            .1;
        let item = by_project_id(&result, project);
        assert_eq!(
            item["read"]["status"], "not_read",
            "{refused} must be not_read"
        );
    }
}

/// One delegation body for the matrix test.
fn grant_for(
    id: &str,
    scope: awr_core::AuthorizationScope,
    actions: &[awr_core::AuthorizedAction],
) -> awr_core::AgentAuthorization {
    use awr_core::{AgentAuthorization, AuthorizationStatus, ExecutionSubjectKind, PersonId};
    AgentAuthorization {
        id: id.into(),
        authorizer_person_id: PersonId::new("matrix-person").unwrap(),
        responsible_person_id: PersonId::new("matrix-person").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope,
        actions: actions.iter().copied().collect(),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1,
        binding_id: Some("matrix-binding".into()),
    }
}

/// The permissive SELECT policies of schema 66 at the SQL level, as the
/// application role without bypass: no marker or tenant-only marker sees
/// nothing; the portfolio marker sees only its own tenant; writes stay
/// pinned; nothing survives the transaction.
#[tokio::test]
async fn portfolio_policies_release_only_their_tenant_and_no_writes() {
    let (_guard, admin, db, _store) = fixture::setup().await;
    clone_project(&admin, 1).await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES('foreign-tenant','Foreign','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status,project_revision)
             VALUES('foreign-tenant','foreign-project','fp','team','e','active',1);",
        )
        .await
        .unwrap();
    let mut app = common::connect_config(&common::with_app_role(&common::test_config(), &db)).await;
    let tenant_counts = |rows: Vec<tokio_postgres::Row>| -> Vec<(String, i64)> {
        rows.iter()
            .map(|row| (row.get::<_, String>(0), row.get::<_, i64>(1)))
            .collect()
    };
    // No markers at all: the isolation policy hides everything.
    let rows = app
        .query(
            "SELECT tenant_id,count(*) FROM awr_team.projects GROUP BY 1",
            &[],
        )
        .await
        .unwrap();
    assert!(
        tenant_counts(rows).is_empty(),
        "without markers nothing is visible"
    );
    // The tenant marker alone (what every other path sets) releases nothing.
    {
        let tx = app.transaction().await.unwrap();
        tx.execute(
            "SELECT set_config('awr.tenant_id','reader-tenant',true)",
            &[],
        )
        .await
        .unwrap();
        let rows = tx
            .query(
                "SELECT tenant_id,count(*) FROM awr_team.projects GROUP BY 1",
                &[],
            )
            .await
            .unwrap();
        assert!(
            tenant_counts(rows).is_empty(),
            "the tenant marker alone must not release rows"
        );
        tx.rollback().await.unwrap();
    }
    // Both markers: own tenant only, and writes stay pinned.
    {
        let tx = app.transaction().await.unwrap();
        tx.execute(
            "SELECT set_config('awr.tenant_id','reader-tenant',true),
                set_config('awr.portfolio_read','on',true)",
            &[],
        )
        .await
        .unwrap();
        let seen = tenant_counts(
            tx.query(
                "SELECT tenant_id,count(*) FROM awr_team.projects GROUP BY 1",
                &[],
            )
            .await
            .unwrap(),
        );
        assert_eq!(seen.len(), 1, "the foreign tenant stays hidden: {seen:?}");
        assert_eq!(seen[0].0, "reader-tenant");
        assert!(seen[0].1 >= 2);
        let updated = tx
            .execute(
                "UPDATE awr_team.projects SET key=key WHERE tenant_id='reader-tenant'",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(updated, 0, "a SELECT policy must not widen UPDATE");
        let deleted = tx
            .execute(
                "DELETE FROM awr_team.work_contracts WHERE tenant_id='reader-tenant'",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(deleted, 0, "a SELECT policy must not widen DELETE");
        tx.rollback().await.unwrap();
    }
    // Another tenant string sees only that tenant.
    {
        let tx = app.transaction().await.unwrap();
        tx.execute(
            "SELECT set_config('awr.tenant_id','foreign-tenant',true),
                set_config('awr.portfolio_read','on',true)",
            &[],
        )
        .await
        .unwrap();
        let seen = tenant_counts(
            tx.query(
                "SELECT tenant_id,count(*) FROM awr_team.projects GROUP BY 1",
                &[],
            )
            .await
            .unwrap(),
        );
        assert_eq!(seen, vec![("foreign-tenant".to_owned(), 1)]);
        tx.rollback().await.unwrap();
    }
    // Nothing survives the transaction on the pooled connection.
    let rows = app
        .query(
            "SELECT tenant_id,count(*) FROM awr_team.projects GROUP BY 1",
            &[],
        )
        .await
        .unwrap();
    assert!(
        tenant_counts(rows).is_empty(),
        "markers must not leak past their transaction"
    );
}
