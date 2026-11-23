#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{PgError, WorkstreamQuery, WorkstreamReadStore, workstream_credential_hash};
use fixture::*;
use serde_json::{Value, json};

const ADMIN_TOKEN: &str =
    "awr1.usage-admin.1111111111111111111111111111111111111111111111111111111111111111";
const ZEBRA_TOKEN: &str =
    "awr1.usage-zebra.2222222222222222222222222222222222222222222222222222222222222222";

async fn seed_usage_scenario(admin: &tokio_postgres::Client) {
    let admin_hash = workstream_credential_hash(ADMIN_TOKEN).unwrap();
    let zebra_hash = workstream_credential_hash(ZEBRA_TOKEN).unwrap();
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
             VALUES('{TENANT}','admin-auditor','human','Audit Lead','active'),
                   ('{TENANT}','member-zebra','human','Zebra Builder','active');
             INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
             VALUES('{TENANT}','usage-admin','admin-auditor','cli-admin','{admin_hash}'),
                   ('{TENANT}','usage-zebra','member-zebra','cli-zebra','{zebra_hash}');
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role,business_roles)
             VALUES('{TENANT}','{PROJECT}','admin-auditor','project_admin','[\"administrator\"]'::jsonb),
                   ('{TENANT}','{PROJECT}','member-zebra','developer','[\"developer\"]'::jsonb);
             UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]'::jsonb
             WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}' AND actor_id='agent';
             INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,active)
             VALUES('{TENANT}','{PROJECT}','admin-auditor','cli-admin','00000000000000000000000001',1,true,false,true),
                   ('{TENANT}','{PROJECT}','admin-auditor','cli-admin','00000000000000000000000002',1,true,false,true),
                   ('{TENANT}','{PROJECT}','member-zebra','cli-zebra','00000000000000000000000001',1,true,true,true),
                   ('{TENANT}','{PROJECT}','member-zebra','cli-zebra','00000000000000000000000002',1,true,true,true);
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('{TENANT}','{PROJECT}','admin-auditor','Audit Lead','active'),
                   ('{TENANT}','{PROJECT}','member-alpha','Alpha Builder','active'),
                   ('{TENANT}','{PROJECT}','member-zebra','Zebra Builder','active'),
                   ('{TENANT}','{PROJECT}','member-delivery','Delivery Member','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('{TENANT}','{PROJECT}','bind-alpha','member-alpha','agent','active');
             INSERT INTO awr_team.project_directories(tenant_id,project_id,version,plan_json,provenance)
             VALUES('{TENANT}','{PROJECT}',1,
                 '{{\"organization\":{{\"id\":\"org-1\",\"name\":\"Example Org\"}},
                    \"departments\":[
                      {{\"id\":\"dept-eng\",\"name\":\"Engineering\"}},
                      {{\"id\":\"grp-app\",\"name\":\"App Group\",\"parent_id\":\"dept-eng\"}},
                      {{\"id\":\"grp-platform\",\"name\":\"Platform Group\",\"parent_id\":\"dept-eng\"}},
                      {{\"id\":\"grp-delivery\",\"name\":\"Delivery Group\",\"parent_id\":\"dept-eng\"}}
                    ],
                    \"project_department_ids\":[\"grp-app\"],
                    \"members\":[
                      {{\"person_id\":\"member-alpha\",\"department_ids\":[\"grp-app\"]}},
                      {{\"person_id\":\"member-zebra\",\"department_ids\":[\"grp-platform\"]}},
                      {{\"person_id\":\"member-delivery\",\"department_ids\":[\"grp-delivery\"]}}
                    ]}}'::jsonb,
                 '{{\"actor_id\":\"admin-auditor\",\"client_id\":\"cli-admin\"}}'::jsonb);"
        ))
        .await
        .unwrap();

    // Update fixture session-a (actor 'agent' -> member-alpha in grp-app) to ended with
    // exact 3_000 ms duration and two counter_ids (explicit counter reset) where
    // cached_input_tokens is intentionally omitted (must stay null, never 0).
    admin
        .batch_execute(&format!(
            "UPDATE awr_team.sessions SET state='ended'
             WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}' AND id='session-a';
             INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,workstream_id,payload_json,created_at)
             VALUES('{TENANT}','{PROJECT}','ev-a-start',20,0,'session.start','agent','a','00000000000000000000000001',
                    '{{\"session_id\":\"session-a\"}}'::jsonb,'2026-11-19T10:00:00.000Z'::timestamptz),
                   ('{TENANT}','{PROJECT}','ev-a-end',21,0,'session.end','agent','a','00000000000000000000000001',
                    '{{\"session_id\":\"session-a\"}}'::jsonb,'2026-11-19T10:00:03.000Z'::timestamptz);
             INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json,usage_json)
             VALUES('{TENANT}','{PROJECT}','cp-a-1','session-a','ctx','ctr',20,'step 1','[]',
                    '{{\"source\":\"native_host_event\",\"source_ref\":\"log#1\",\"counter_id\":\"c-1\",\"scope\":\"host_session\",\"coverage\":\"complete\",\"observed_at_unix_ms\":1795082401000,\"input_tokens\":1000,\"output_tokens\":200,\"cached_input_tokens\":null}}'::jsonb),
                   ('{TENANT}','{PROJECT}','cp-a-2','session-a','ctx','ctr',21,'step 2','[]',
                    '{{\"source\":\"native_host_event\",\"source_ref\":\"log#2\",\"counter_id\":\"c-1\",\"scope\":\"host_session\",\"coverage\":\"complete\",\"observed_at_unix_ms\":1795082402000,\"input_tokens\":4000,\"output_tokens\":500,\"cached_input_tokens\":null}}'::jsonb),
                   ('{TENANT}','{PROJECT}','cp-a-3','session-a','ctx','ctr',22,'step 3','[]',
                    '{{\"source\":\"native_host_event\",\"source_ref\":\"log#3\",\"counter_id\":\"c-2\",\"scope\":\"host_session\",\"coverage\":\"complete\",\"observed_at_unix_ms\":1795082403000,\"input_tokens\":1200,\"output_tokens\":200,\"cached_input_tokens\":null}}'::jsonb);"
        ))
        .await
        .unwrap();

    // Add a session for member-zebra (grp-platform) on work 'a' with NO usage_json
    // and NO start/end events, plus a second session on stream 2 work 'b-private' (hidden from A).
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
             VALUES('{TENANT}','{PROJECT}','session-zebra-uncollected','main','a','member-zebra','cli-zebra','conv-z1','interrupted','00000000000000000000000001',1),
                   ('{TENANT}','{PROJECT}','session-stream2','main','b-private','member-zebra','cli-zebra','conv-z2','ended','00000000000000000000000002',1);
             INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,workstream_id,payload_json,created_at)
             VALUES('{TENANT}','{PROJECT}','ev-z2-start',23,0,'session.start','member-zebra','b-private','00000000000000000000000002',
                    '{{\"session_id\":\"session-stream2\"}}'::jsonb,'2026-11-19T10:00:01.000Z'::timestamptz),
                   ('{TENANT}','{PROJECT}','ev-z2-end',24,0,'session.end','member-zebra','b-private','00000000000000000000000002',
                    '{{\"session_id\":\"session-stream2\"}}'::jsonb,'2026-11-19T10:01:01.000Z'::timestamptz);"
        ))
        .await
        .unwrap();
}

async fn read_usage(store: &WorkstreamReadStore, token: &str, q: WorkstreamQuery) -> Value {
    store.query(TENANT, PROJECT, token, q).await.unwrap()
}

#[tokio::test]
async fn usage_summary_aggregates_duration_tokens_and_reports_not_collected_never_zero() {
    let (_guard, admin, _db, store) = setup().await;
    seed_usage_scenario(&admin).await;

    let reply = read_usage(&store, ADMIN_TOKEN, query("usage.summary")).await;
    assert_eq!(reply["protocol_version"], 1);
    let data = &reply["data"];
    assert_eq!(data["execution_authorized"], false);
    assert_eq!(data["collection"]["personal_ledger_collected"], false);
    assert_eq!(data["collection"]["billing_collected"], false);
    assert_eq!(data["collection"]["cost_status"], "not_collected");
    assert_eq!(data["collection"]["is_historical_observation"], true);
    assert_eq!(data["collection"]["is_not_estimated_remaining_time"], true);

    // Project totals (fixture seeds session-a on 'a' and session-b on 'b-private'):
    // - session-a: [10:00:00, 10:00:03] (3000 ms), tokens 4000+1200=5200 in, 500+200=700 out, cached=null
    // - session-stream2: [10:00:01, 10:01:01] (60000 ms), no usage
    // - session-b + session-zebra-uncollected: no timestamps, no usage
    // Union of [0, 3000] and [1000, 61000] is [0, 61000] => 61000 ms wall clock, 63000 ms summed.
    assert_eq!(data["project"]["session_count"], 4);
    assert_eq!(data["project"]["duration"]["status"], "partial");
    assert_eq!(data["project"]["duration"]["summed_ms"], 63_000);
    assert_eq!(data["project"]["duration"]["wall_clock_ms"], 61_000);
    assert_eq!(data["project"]["duration"]["sessions_with_duration"], 2);
    assert_eq!(data["project"]["duration"]["sessions_without_duration"], 2);

    assert_eq!(data["project"]["tokens"]["status"], "partial");
    assert_eq!(data["project"]["tokens"]["input_tokens"], 5_200);
    assert_eq!(data["project"]["tokens"]["output_tokens"], 700);
    assert!(
        data["project"]["tokens"]["cached_input_tokens"].is_null(),
        "unreported token dimension must be null, never 0"
    );
    assert_eq!(data["project"]["tokens"]["sessions_with_usage"], 1);
    assert_eq!(data["project"]["tokens"]["sessions_without_usage"], 3);

    assert_eq!(data["project"]["cost"]["status"], "not_collected");
    assert!(data["project"]["cost"]["amount_micros"].is_null());
    assert!(data["project"]["cost"]["currency"].is_null());

    // Work 'b-private' has one timed session (60000 ms) + fixture session-b (no timestamps)
    // and NO token usage: duration is partial (60000 ms), tokens must be not_collected/null, never 0.
    let tasks = data["by_task"].as_array().unwrap();
    let task_b = tasks.iter().find(|t| t["work_id"] == "b-private").unwrap();
    assert_eq!(task_b["duration"]["status"], "partial");
    assert_eq!(task_b["duration"]["summed_ms"], 60_000);
    assert_eq!(task_b["tokens"]["status"], "not_collected");
    assert!(task_b["tokens"]["input_tokens"].is_null());
    assert!(task_b["tokens"]["output_tokens"].is_null());
    assert_eq!(task_b["cost"]["status"], "not_collected");
    assert!(task_b["cost"]["amount_micros"].is_null());

    // Work 'c' has zero sessions: both duration and tokens must be not_collected/null, never 0.
    let task_c = tasks.iter().find(|t| t["work_id"] == "c").unwrap();
    assert_eq!(task_c["session_count"], 0);
    assert_eq!(task_c["duration"]["status"], "not_collected");
    assert!(task_c["duration"]["summed_ms"].is_null());
    assert_eq!(task_c["tokens"]["status"], "not_collected");
    assert!(task_c["tokens"]["input_tokens"].is_null());

    // Coverage must list collected group (App Group) AND every missing group (Delivery + Platform).
    assert_eq!(data["coverage"]["status"], "partial");
    assert_eq!(data["coverage"]["cost_status"], "not_collected");
    assert_eq!(data["coverage"]["groups"]["registered"], true);
    assert_eq!(
        data["coverage"]["groups"]["collected"],
        json!([{"id":"grp-app","name":"App Group"}])
    );
    assert_eq!(
        data["coverage"]["groups"]["missing"],
        json!([
            {"id":"grp-delivery","name":"Delivery Group"},
            {"id":"grp-platform","name":"Platform Group"}
        ])
    );
}

#[tokio::test]
async fn usage_summary_enforces_member_privacy_and_never_ranks_by_personal_usage() {
    let (_guard, admin, _db, store) = setup().await;
    seed_usage_scenario(&admin).await;

    // Give member-zebra 10x larger token usage than member-alpha so any ranking by
    // usage would put member-zebra ahead of member-alpha.
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json,usage_json)
             VALUES('{TENANT}','{PROJECT}','cp-z-1','session-stream2','ctx','ctr',24,'done','[]',
                    '{{\"source\":\"native_host_event\",\"source_ref\":\"log#z\",\"counter_id\":\"cz-1\",\"scope\":\"host_session\",\"coverage\":\"complete\",\"observed_at_unix_ms\":1795082460000,\"input_tokens\":90000,\"output_tokens\":9000,\"cached_input_tokens\":1000}}'::jsonb);"
        ))
        .await
        .unwrap();

    // 1. Audit-authorized caller sees all members, strictly sorted by member_id ASC (NOT ranked by usage).
    let audited = read_usage(&store, ADMIN_TOKEN, query("usage.summary")).await;
    let data = &audited["data"];
    assert_eq!(data["member_scope"], "project");
    assert_eq!(data["member_ordering"], "identifier_asc");
    assert_eq!(data["personal_comparison_ranking"], false);
    assert_eq!(data["coverage"]["members"]["scope"], "project");
    assert_eq!(data["coverage"]["members"]["hidden_member_count"], 0);

    let member_ids: Vec<&str> = data["by_member"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["member_id"].as_str().unwrap())
        .collect();
    let mut sorted_ids = member_ids.clone();
    sorted_ids.sort();
    assert_eq!(
        member_ids, sorted_ids,
        "by_member must be strictly ordered by identifier, never ranked by usage"
    );
    let alpha_pos = member_ids
        .iter()
        .position(|id| *id == "member-alpha")
        .unwrap();
    let zebra_pos = member_ids
        .iter()
        .position(|id| *id == "member-zebra")
        .unwrap();
    assert!(
        alpha_pos < zebra_pos,
        "member-alpha (5200 tokens) must stay before member-zebra (90000 tokens)"
    );
    for entry in data["by_member"].as_array().unwrap() {
        let obj = entry.as_object().unwrap();
        assert!(!obj.contains_key("rank"));
        assert!(!obj.contains_key("percentile"));
    }

    // 2. Non-audit member (A -> agent -> member-alpha) sees ONLY self, never member-zebra.
    let self_only = read_usage(&store, A, query("usage.summary")).await;
    let self_data = &self_only["data"];
    assert_eq!(self_data["member_scope"], "self");
    assert_eq!(self_data["coverage"]["members"]["scope"], "self");
    assert!(
        self_data["coverage"]["members"]["hidden_member_count"]
            .as_u64()
            .unwrap()
            >= 2
    );
    let visible_members = self_data["by_member"].as_array().unwrap();
    assert_eq!(visible_members.len(), 1);
    assert_eq!(visible_members[0]["member_id"], "member-alpha");
    let serialized = self_only.to_string();
    assert!(
        !serialized.contains("member-zebra"),
        "non-audit caller must not see other member identities or usage"
    );
    assert!(
        !serialized.contains("90000"),
        "non-audit caller must not see other member token totals"
    );

    // 3. Non-audit member explicitly filtering by another member_actor_id is Forbidden.
    let mut other_query = query("usage.summary");
    other_query.member_actor_id = Some("member-zebra".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, other_query).await,
        Err(PgError::Forbidden)
    ));

    // 4. Non-audit member filtering by own actor_id succeeds.
    let mut own_query = query("usage.summary");
    own_query.member_actor_id = Some("agent".into());
    let own_reply = read_usage(&store, A, own_query).await;
    assert_eq!(own_reply["data"]["by_member"].as_array().unwrap().len(), 1);
    assert_eq!(
        own_reply["data"]["by_member"][0]["member_id"],
        "member-alpha"
    );

    // 5. Audit-authorized caller filtering by member-zebra succeeds.
    let mut audited_target = query("usage.summary");
    audited_target.member_actor_id = Some("member-zebra".into());
    let target_reply = read_usage(&store, ADMIN_TOKEN, audited_target).await;
    assert_eq!(
        target_reply["data"]["by_member"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        target_reply["data"]["by_member"][0]["member_id"],
        "member-zebra"
    );
    assert_eq!(
        target_reply["data"]["by_member"][0]["tokens"]["input_tokens"],
        90_000
    );
}

#[tokio::test]
async fn usage_summary_respects_workstream_scope_cross_project_and_rejects_invalid_selectors() {
    let (_guard, admin, _db, store) = setup().await;
    seed_usage_scenario(&admin).await;

    // Reader A holds stream 1 only (works 'a' and 'c'); work 'b-private' (stream 2) is hidden.
    let scoped = read_usage(&store, A, query("usage.summary")).await;
    assert_eq!(scoped["data"]["coverage"]["hidden_work_count"], 1);
    let task_ids: Vec<&str> = scoped["data"]["by_task"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["work_id"].as_str().unwrap())
        .collect();
    assert!(task_ids.contains(&"a"));
    assert!(!task_ids.contains(&"b-private"));

    // Explicit query for unauthorized work 'b-private' or stream 2 is refused.
    let mut hidden_work = query("usage.summary");
    hidden_work.work_id = Some("b-private".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, hidden_work).await,
        Err(PgError::Forbidden)
    ));
    let mut hidden_stream = query("usage.summary");
    hidden_stream.workstream_id = Some("00000000000000000000000002".parse().unwrap());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, hidden_stream).await,
        Err(PgError::Forbidden)
    ));

    // Cross-tenant / wrong-project / no-grant reads are refused.
    assert!(matches!(
        store
            .query("other-tenant", PROJECT, A, query("usage.summary"))
            .await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store
            .query(TENANT, "other-project", A, query("usage.summary"))
            .await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store
            .query(TENANT, PROJECT, NONE, query("usage.summary"))
            .await,
        Err(PgError::Forbidden)
    ));

    // Unsupported selectors (session_id, limit, cursor) and unknown sort/ranking fields fail validation.
    for bad in [
        json!({"protocol_version":1,"op":"usage.summary","session_id":"session-a"}),
        json!({"protocol_version":1,"op":"usage.summary","limit":10}),
        json!({"protocol_version":1,"op":"usage.summary","cursor":"opaque"}),
    ] {
        let q: WorkstreamQuery = serde_json::from_value(bad).unwrap();
        assert!(q.validate().is_err());
    }
    for unknown in [
        json!({"protocol_version":1,"op":"usage.summary","sort_by":"tokens_desc"}),
        json!({"protocol_version":1,"op":"usage.summary","rank":true}),
    ] {
        assert!(serde_json::from_value::<WorkstreamQuery>(unknown).is_err());
    }

    // Portfolio overview exposes matching group usage coverage without leaking individual usage.
    let portfolio = store
        .portfolio_overview(
            TENANT,
            ADMIN_TOKEN,
            &[PROJECT.to_owned()],
            awr_team_pg::PortfolioOverviewRequest {
                protocol_version: 1,
                limit: None,
                cursor: None,
            },
        )
        .await
        .unwrap();
    let item = &portfolio["data"]["projects"][0];
    assert_eq!(item["usage"]["status"], "partial");
    assert_eq!(item["usage"]["cost_status"], "not_collected");
    assert_eq!(
        item["usage"]["groups"]["collected"],
        json!([{"id":"grp-app","name":"App Group"}])
    );
    assert_eq!(
        item["usage"]["groups"]["missing"],
        json!([
            {"id":"grp-delivery","name":"Delivery Group"},
            {"id":"grp-platform","name":"Platform Group"}
        ])
    );
    assert!(
        item["usage"].get("by_member").is_none(),
        "portfolio overview must never leak per-member usage"
    );
}
