#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod access;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "fixtures/oauth_http.rs"]
mod transport;
use access::*;
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use transport::*;

async fn code(server: &Server, credential: &str) -> (String, String) {
    let pending = begin(server, "Synthetic native SDK client").await;
    let res = consent(server, &pending, "allow", credential)
        .send()
        .await
        .unwrap();
    let url = completion_callback(res).await;
    assert!(!url.as_str().contains(credential));
    let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs["state"], "client state & original");
    (pending.client_id, pairs["code"].clone())
}
async fn tokens(server: &Server, credential: &str) -> (String, Value) {
    let (client, code) = code(server, credential).await;
    let body = token_body(&client, &code, RESOURCE, VERIFIER);
    let res = exchange(server, body.clone()).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-store");
    let text = res.text().await.unwrap();
    assert!(!text.contains(credential));
    assert!(!text.contains(&code));
    let token: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(token["expires_in"], 3600);
    assert_eq!(token["scope"], "awr.project");
    assert!(
        token["refresh_token"]
            .as_str()
            .unwrap()
            .starts_with("awr_refresh_")
    );
    assert_eq!(exchange(server, body).send().await.unwrap().status(), 400);
    (client, token)
}
async fn issue(server: &Server, credential: &str) -> String {
    tokens(server, credential).await.1["access_token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn sdk_client(
    server: &Server,
    access_token: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let transport = StreamableHttpClientTransport::with_client(
        http(),
        StreamableHttpClientTransportConfig::with_uri(format!(
            "{}/v1/projects/one/mcp",
            server.url
        ))
        .auth_header(access_token),
    );
    ().serve(transport).await.unwrap()
}

async fn sdk_call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool: &str,
    input: Value,
) -> Value {
    let result = client
        .call_tool(
            CallToolRequestParams::new(tool.to_owned())
                .with_arguments(input.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    result.structured_content.unwrap()
}

#[tokio::test]
async fn task_scoped_agent_uses_mcp_web_and_oauth_without_project_wide_discovery() {
    let (_guard, admin, db, store) = setup().await;
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
          VALUES('reader-tenant','reader-project','connection-owner','Connection owner','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
          VALUES('reader-tenant','reader-project','connection-binding','connection-owner','agent','active');").await.unwrap();
    let authorization = serde_json::from_value(json!({
        "id":"task-connection-access","authorizer_person_id":"connection-owner","responsible_person_id":"connection-owner",
        "subject_kind":"agent","subject_id":"agent","client_id":"cli-a","session_id":null,"model_id":null,
        "scope":{"kind":"task","project_id":PROJECT,"work_item_id":"a"},"actions":["inspect","start_work"],
        "expires_at_ms":null,"status":"active","revoked_at_ms":null,"revoked_by":null,
        "verifiable_capabilities":[],"self_reported_skill_hints":[],"parent_authorization_id":null,
        "maintainer_person_id":null,"created_at_ms":1000,"binding_id":"connection-binding"
    })).unwrap();
    awr_team_pg::AuthorizationStore::from_config(common::with_app_role(
        &common::test_config(),
        &db,
    ))
    .issue(
        TENANT,
        PROJECT,
        &awr_core::IssueAuthorizationRequest {
            request_key: "task-connection-access".into(),
            authorization,
        },
    )
    .await
    .unwrap();
    let server = start(store, true).await;
    let direct = sdk_client(&server, A).await;
    for query in [
        json!({"protocol_version":1,"op":"capabilities"}),
        json!({"protocol_version":1,"op":"work.prepare","work_id":"b-private"}),
    ] {
        let denied = direct
            .call_tool(
                CallToolRequestParams::new("awr_team_query".to_owned())
                    .with_arguments(query.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(denied.is_error, Some(true));
    }
    let next = sdk_call(
        &direct,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.next"}),
    )
    .await;
    assert_eq!(next["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["data"]["items"][0]["work_id"], "a");
    let login = server
        .post("/v1/web/login")
        .header("origin", ISSUER)
        .header("x-awr-web", "1")
        .json(&json!({"bearer":A}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (client, issued) = tokens(&server, A).await;
    let refresh = encode(&[
        ("grant_type", "refresh_token"),
        ("client_id", &client),
        ("refresh_token", issued["refresh_token"].as_str().unwrap()),
        ("resource", RESOURCE),
    ]);
    let refreshed = exchange(&server, refresh).send().await.unwrap();
    assert_eq!(refreshed.status(), 200);
    let refreshed: Value = refreshed.json().await.unwrap();
    let oauth = sdk_client(&server, refreshed["access_token"].as_str().unwrap()).await;
    let list = sdk_call(
        &oauth,
        "awr_team_connections_list",
        json!({"protocol_version":1}),
    )
    .await;
    let items = list["items"].as_array().unwrap();
    let agent = items.iter().find(|item| item["kind"] == "agent").unwrap()["connection_id"].clone();
    let browser =
        items.iter().find(|item| item["kind"] == "browser").unwrap()["connection_id"].clone();
    let listed = server
        .post("/v1/web/projects/one/connections/list")
        .header("origin", ISSUER)
        .header("x-awr-web", "1")
        .header("cookie", &cookie)
        .json(&json!({"protocol_version":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);
    sdk_call(
        &direct,
        "awr_team_connections_revoke",
        json!({"protocol_version":1,"connection_id":agent}),
    )
    .await;
    assert_eq!(
        initialize(&server, "one", refreshed["access_token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    sdk_call(
        &direct,
        "awr_team_connections_revoke",
        json!({"protocol_version":1,"connection_id":browser}),
    )
    .await;
    let denied = server
        .post("/v1/web/projects/one/connections/list")
        .header("origin", ISSUER)
        .header("x-awr-web", "1")
        .header("cookie", &cookie)
        .json(&json!({"protocol_version":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    let _ = oauth.cancel().await;
    direct.cancel().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn connection_management_works_through_mcp_and_restores_browser_cutoffs_after_restart() {
    let (_guard, _, db, store) = setup().await;
    let directory =
        std::env::temp_dir().join(format!("awr-connections-http-{}", awr_core::Id::new()));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    let first_token = issue(&server, A).await;
    let first = sdk_client(&server, &first_token).await;
    let before = sdk_call(
        &first,
        "awr_team_connections_list",
        json!({"protocol_version":1}),
    )
    .await;
    let first_reference = before["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "agent")
        .unwrap()["connection_id"]
        .clone();
    let second_token = issue(&server, A).await;
    let second = sdk_client(&server, &second_token).await;
    let login = server
        .post("/v1/web/login")
        .header("origin", ISSUER)
        .header("x-awr-web", "1")
        .json(&json!({"bearer":A}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let request = json!({"protocol_version":1,"connection_id":first_reference});
    let result = sdk_call(&second, "awr_team_connections_revoke", request.clone()).await;
    assert_eq!(result["state"], "revoked");
    let cutoff = result["revoked_at_ms"].clone();
    let list = sdk_call(
        &second,
        "awr_team_connections_list",
        json!({"protocol_version":1}),
    )
    .await;
    let browser_reference = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "browser")
        .unwrap()["connection_id"]
        .clone();
    for secret in [A, &first_token, &second_token, &cookie, "sha256:"] {
        assert!(!list.to_string().contains(secret));
    }
    let _ = first.cancel().await;
    second.cancel().await.unwrap();
    server.shutdown().await;
    let store = awr_team_pg::WorkstreamReadStore::from_config(common::with_app_role(
        &common::test_config(),
        &db,
    ));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    assert_eq!(
        initialize(&server, "one", &first_token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let second = sdk_client(&server, &second_token).await;
    let replay = sdk_call(&second, "awr_team_connections_revoke", request).await;
    assert_eq!(replay["revoked_at_ms"], cutoff);
    let restored = server
        .post("/v1/web/projects/one/connections/list")
        .header("origin", ISSUER)
        .header("x-awr-web", "1")
        .header("cookie", &cookie)
        .json(&json!({"protocol_version":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(restored.status(), 200);
    let restored: Value = restored.json().await.unwrap();
    assert!(
        restored["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["connection_id"] == browser_reference && item["is_current"] == true)
    );
    sdk_call(
        &second,
        "awr_team_connections_revoke",
        json!({"protocol_version":1,"connection_id":browser_reference}),
    )
    .await;
    second.cancel().await.unwrap();
    server.shutdown().await;
    let store = awr_team_pg::WorkstreamReadStore::from_config(common::with_app_role(
        &common::test_config(),
        &db,
    ));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    let denied = server
        .post("/v1/web/projects/one/connections/list")
        .header("origin", ISSUER)
        .header("x-awr-web", "1")
        .header("cookie", &cookie)
        .json(&json!({"protocol_version":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    server.shutdown().await;
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn oauth_restart_rotation_and_concurrent_reconnect_preserve_owned_work_without_duplicate_claims()
 {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let directory = std::env::temp_dir().join(format!(
        "awr-oauth-continuity-{:x}",
        u128::from_be_bytes(random)
    ));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    let (registration, original) = tokens(&server, A).await;
    let client = sdk_client(&server, original["access_token"].as_str().unwrap()).await;
    let prepare =
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a","session_id":"session-a"});
    let before = sdk_call(&client, "awr_team_query", prepare.clone()).await;
    let acquire = serde_json::to_value(command(
        &before,
        "continuity-acquire",
        "claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1",
               "expected_work_version":"0","ttl_seconds":300}),
    ))
    .unwrap();
    let taken = sdk_call(&client, "awr_team_command", acquire.clone()).await;
    let prepared = sdk_call(&client, "awr_team_query", prepare.clone()).await;
    let checkpoint = serde_json::to_value(command(
        &prepared,
        "continuity-checkpoint",
        "session.checkpoint",
        json!({"session_id":"session-a","expected_session_version":"1",
               "context_hash":prepared["data"]["context_hash"],
               "next_action":"Continue the existing implementation after reconnecting",
               "open_loops":["Final integration verification remains unfinished"],
               "client_info":{"product":"Continuity fixture","capabilities":{
                   "model":"unsupported","usage":"unsupported","progress":"supported"}},
               "progress":{"phase":"testing","summary":"Implementation is ready for integration verification"}}),
    ))
    .unwrap();
    let saved = sdk_call(&client, "awr_team_command", checkpoint).await;
    let counts = "SELECT (SELECT count(*) FROM awr_team.sessions),
                         (SELECT count(*) FROM awr_team.claims),
                         (SELECT count(*) FROM awr_team.operations)";
    let baseline = admin.query_one(counts, &[]).await.unwrap();
    client.cancel().await.unwrap();
    server.shutdown().await;

    let store =
        awr_team_pg::WorkstreamReadStore::from_config(common::with_db(&common::test_config(), &db));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    assert_eq!(
        initialize(&server, "one", original["access_token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let rotated = exchange(
        &server,
        encode(&[
            ("grant_type", "refresh_token"),
            ("client_id", &registration),
            ("refresh_token", original["refresh_token"].as_str().unwrap()),
        ]),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    assert_ne!(rotated["refresh_token"], original["refresh_token"]);
    let access = rotated["access_token"].as_str().unwrap();
    let (one, two) = tokio::join!(sdk_client(&server, access), sdk_client(&server, access));
    let session = json!({"protocol_version":1,"op":"session.inspect","session_id":"session-a"});
    let (resumed, parallel) = tokio::join!(
        sdk_call(&one, "awr_team_query", session.clone()),
        sdk_call(&two, "awr_team_query", session)
    );
    assert_eq!(resumed, parallel);
    let session = &resumed["data"]["items"][0];
    assert_eq!(session["session_id"], "session-a");
    assert_eq!(session["session_version"], "2");
    assert_eq!(
        session["checkpoint_id"],
        saved["receipt"]["data"]["checkpoint_id"]
    );
    assert_eq!(session["contract_matches_current"], true);
    assert_eq!(
        session["open_loops"],
        json!(["Final integration verification remains unfinished"])
    );
    let claim = sdk_call(
        &one,
        "awr_team_query",
        json!({"protocol_version":1,"op":"claim.inspect","session_id":"session-a",
               "claim_id":taken["receipt"]["data"]["claim_id"]}),
    )
    .await;
    assert_eq!(claim["data"]["lease_live"], true);
    let observed = sdk_call(&one, "awr_team_query", prepare).await;
    assert_eq!(observed["data"]["work_id"], "a");
    assert_eq!(
        observed["data"]["contract_hash"],
        before["data"]["contract_hash"]
    );
    let outcome = sdk_call(
        &two,
        "awr_team_query",
        json!({"protocol_version":1,"op":"command.inspect","work_id":"a","request_id":"continuity-acquire"}),
    )
    .await;
    assert_eq!(outcome["data"]["receipt"], taken["receipt"]);
    let replay = sdk_call(&two, "awr_team_command", acquire).await;
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], taken["receipt"]);
    let after = admin.query_one(counts, &[]).await.unwrap();
    for index in 0..3 {
        assert_eq!(after.get::<_, i64>(index), baseline.get::<_, i64>(index));
    }
    let identity = admin
        .query_one(
            "SELECT actor_id,client_id FROM awr_team.sessions WHERE id='session-a'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(identity.get::<_, String>(0), "agent");
    assert_eq!(identity.get::<_, String>(1), "cli-a");
    one.cancel().await.unwrap();
    two.cancel().await.unwrap();
    server.shutdown().await;
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn refresh_retains_live_permissions_and_rejects_revocation_expiry_and_replay() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(store, true).await;
    let (client, token) = tokens(&server, A).await;
    let body = encode(&[
        ("grant_type", "refresh_token"),
        ("client_id", &client),
        ("refresh_token", token["refresh_token"].as_str().unwrap()),
    ]);
    for (revoke, restore) in [
        (
            "UPDATE awr_team.workstream_grants SET can_read=false WHERE client_id='cli-a'",
            "UPDATE awr_team.workstream_grants SET can_read=true WHERE client_id='cli-a'",
        ),
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='reader-a'",
        ),
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'",
        ),
    ] {
        admin.batch_execute(revoke).await.unwrap();
        let res = exchange(&server, body.clone()).send().await.unwrap();
        assert_eq!(res.status(), 400);
        let text = res.text().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap()["error"],
            "invalid_grant"
        );
        assert!(!text.contains(A));
        assert!(!text.contains(token["refresh_token"].as_str().unwrap()));
        admin.batch_execute(restore).await.unwrap();
    }
    let res = exchange(&server, body.clone()).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-store");
    let renewed: Value = res.json().await.unwrap();
    let access = renewed["access_token"].as_str().unwrap();
    assert_ne!(renewed["refresh_token"], token["refresh_token"]);
    assert_eq!(
        initialize(&server, "one", access)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        initialize(&server, "other", access)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(exchange(&server, body).send().await.unwrap().status(), 400);
    assert_eq!(
        initialize(&server, "one", access)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}
fn initialize(server: &Server, alias: &str, token: &str) -> reqwest::RequestBuilder {
    server.post(&format!("/v1/projects/{alias}/mcp")).bearer_auth(token)
        .header("accept","application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"synthetic-fixture","version":"1"}}}))
}
#[tokio::test]
async fn rejected_static_credentials_offer_native_reauthentication_only_when_oauth_is_enabled() {
    let (_guard, admin, db, store) = setup().await;
    let enabled = start(store, true).await;
    let disabled = start(
        awr_team_pg::WorkstreamReadStore::from_config(common::with_db(&common::test_config(), &db)),
        false,
    )
    .await;
    assert_eq!(
        initialize(&enabled, "one", A)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for change in [
        "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'",
        "UPDATE awr_team.credentials SET expires_at=NULL,revoked_at=clock_timestamp() WHERE id='reader-a'",
    ] {
        admin.batch_execute(change).await.unwrap();
        let res = initialize(&enabled, "one", A).send().await.unwrap();
        assert_eq!(res.status(), 401);
        assert_eq!(
            res.headers()["www-authenticate"],
            format!(
                "Bearer resource_metadata=\"{ISSUER}/.well-known/oauth-protected-resource/v1/projects/one/mcp\", error=\"invalid_token\""
            )
        );
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert!(!res.text().await.unwrap().contains(A));
        let legacy = initialize(&disabled, "one", A).send().await.unwrap();
        assert_eq!(legacy.status(), 403);
        assert!(legacy.headers().get("www-authenticate").is_none());
        let classic = enabled
            .post("/v1/projects/one/query")
            .bearer_auth(A)
            .json(&json!({"protocol_version":1,"op":"capabilities"}))
            .send()
            .await
            .unwrap();
        assert_eq!(classic.status(), 403);
    }
}
#[tokio::test]
async fn issued_token_works_in_native_sdk_but_cannot_expand_scope_or_replace_static_web_auth() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(store, true).await;
    let token = issue(&server, A).await;
    let transport = StreamableHttpClientTransport::with_client(
        http(),
        StreamableHttpClientTransportConfig::with_uri(format!(
            "{}/v1/projects/one/mcp",
            server.url
        ))
        .auth_header(&token),
    );
    let client = ().serve(transport).await.unwrap();
    assert!(
        client
            .list_all_tools()
            .await
            .unwrap()
            .iter()
            .any(|t| t.name == "awr_team_query")
    );
    for op in ["capabilities", "work.next"] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("awr_team_query").with_arguments(
                    json!({"protocol_version":1,"op":op})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }
    let denied = client
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"work.prepare","work_id":"b-private"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert!(
        denied.is_error.unwrap_or(false),
        "OAuth must retain existing workstream isolation"
    );
    assert_eq!(
        initialize(&server, "other", &token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    for (token, expected) in [(A, 200), (token.as_str(), 403)] {
        let classic = server
            .post("/v1/projects/one/query")
            .bearer_auth(token)
            .json(&json!({"protocol_version":1,"op":"capabilities"}))
            .send()
            .await
            .unwrap();
        assert_eq!(classic.status(), expected);
        let web = server
            .post("/v1/web/login")
            .header("origin", ISSUER)
            .header("x-awr-web", "1")
            .json(&json!({"bearer":token,"project":"one"}))
            .send()
            .await
            .unwrap();
        assert_eq!(web.status(), expected);
    }
    assert_eq!(
        initialize(&server, "one", A).send().await.unwrap().status(),
        200
    );
    // Change live grants after issuance; the adapter must not cache authority.
    admin.execute("UPDATE awr_team.workstream_grants SET can_read=false,grant_version=grant_version+1 WHERE client_id='cli-a'",&[]).await.unwrap();
    // Revoking all project read grants fails MCP admission, before tool dispatch.
    assert_eq!(
        initialize(&server, "one", &token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    admin.execute("UPDATE awr_team.workstream_grants SET can_read=true,grant_version=grant_version+1 WHERE client_id='cli-a'",&[]).await.unwrap();
    assert_eq!(
        initialize(&server, "one", &token)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    admin
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            &[],
        )
        .await
        .unwrap();
    let res = initialize(&server, "one", &token).send().await.unwrap();
    assert_eq!(res.status(), 401);
    assert!(
        res.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("invalid_token")
    );
    assert!(!res.text().await.unwrap().contains(A));
    client.cancel().await.unwrap();
}
#[tokio::test]
async fn consent_checks_current_project_access_and_failure_does_not_leak_or_consume_retry() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(store, true).await;
    let pending = begin(&server, "Synthetic client").await;
    let bad = "awr1.reader-a.synthetic-invalid-test-value";
    let res = consent(&server, &pending, "allow", bad)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(res.headers()["referrer-policy"], "same-origin");
    let html = res.text().await.unwrap();
    assert!(!html.contains(bad));
    assert!(html.contains("try again"));
    let res = consent(&server, &pending, "allow", B).send().await.unwrap();
    completion_callback(res).await;
    admin
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            &[],
        )
        .await
        .unwrap();
    let pending = begin(&server, "Revoked credential client").await;
    let res = consent(&server, &pending, "allow", A).send().await.unwrap();
    assert_eq!(res.status(), 403);
    let html = res.text().await.unwrap();
    assert!(!html.contains(A));
    completion_callback(
        consent(&server, &pending, "cancel", "")
            .send()
            .await
            .unwrap(),
    )
    .await;
}
#[tokio::test]
async fn exchange_checks_all_bindings_and_expired_underlying_credentials_stop_next_request() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(store, true).await;
    for field in ["client", "redirect", "resource", "verifier"] {
        let (client, code) = code(&server, A).await;
        let mut body = token_body(&client, &code, RESOURCE, VERIFIER);
        match field {
            "client" => body = token_body("unknown", &code, RESOURCE, VERIFIER),
            "redirect" => body = body.replace("client.example", "other.example"),
            "resource" => {
                body = token_body(
                    &client,
                    &code,
                    "https://awr.example/v1/projects/other/mcp",
                    VERIFIER,
                )
            }
            "verifier" => body = token_body(&client, &code, RESOURCE, &"z".repeat(43)),
            _ => unreachable!(),
        }
        let res = exchange(&server, body).send().await.unwrap();
        assert_eq!(res.status(), 400, "{field}");
        let value: Value = res.json().await.unwrap();
        assert_eq!(value["error"], "invalid_grant");
        assert_eq!(
            exchange(&server, token_body(&client, &code, RESOURCE, VERIFIER))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    let token = issue(&server, A).await;
    admin.execute("UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'",&[]).await.unwrap();
    assert_eq!(
        initialize(&server, "one", &token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

#[cfg(unix)]
#[tokio::test]
async fn durable_restart_keeps_identity_and_live_revocation_without_new_consent() {
    let (_guard, admin, db, store) = setup().await;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let directory = std::env::temp_dir().join(format!(
        "awr-durable-http-{:x}",
        u128::from_be_bytes(random)
    ));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    let pending = begin(&server, "Durable connection fixture").await;
    assert!(
        pending
            .html
            .contains("Approved connections survive service restarts.")
    );
    let res = consent(&server, &pending, "cancel", "")
        .send()
        .await
        .unwrap();
    completion_callback(res).await;
    let (client, original) = tokens(&server, A).await;
    server.shutdown().await;
    let store =
        awr_team_pg::WorkstreamReadStore::from_config(common::with_db(&common::test_config(), &db));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    let access = original["access_token"].as_str().unwrap();
    assert_eq!(
        initialize(&server, "one", access)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        initialize(&server, "other", access)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let body = encode(&[
        ("grant_type", "refresh_token"),
        ("client_id", &client),
        ("refresh_token", original["refresh_token"].as_str().unwrap()),
    ]);
    for (revoke, restore) in [
        (
            "UPDATE awr_team.workstream_grants SET can_read=false WHERE client_id='cli-a'",
            "UPDATE awr_team.workstream_grants SET can_read=true WHERE client_id='cli-a'",
        ),
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='reader-a'",
        ),
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'",
        ),
    ] {
        admin.batch_execute(revoke).await.unwrap();
        assert_eq!(
            initialize(&server, "one", access)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let res = exchange(&server, body.clone()).send().await.unwrap();
        assert_eq!(res.status(), 400);
        assert_eq!(res.json::<Value>().await.unwrap()["error"], "invalid_grant");
        admin.batch_execute(restore).await.unwrap();
    }
    let res = exchange(&server, body.clone()).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let rotated: Value = res.json().await.unwrap();
    server.shutdown().await;
    let store =
        awr_team_pg::WorkstreamReadStore::from_config(common::with_db(&common::test_config(), &db));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    assert_eq!(
        initialize(&server, "one", rotated["access_token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(exchange(&server, body).send().await.unwrap().status(), 400);
    server.shutdown().await;
    let store =
        awr_team_pg::WorkstreamReadStore::from_config(common::with_db(&common::test_config(), &db));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    assert_eq!(
        initialize(&server, "one", rotated["access_token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    server.shutdown().await;
    std::fs::remove_dir_all(directory).unwrap();
}

/// Register `name`, approve the connection with `credential` and exchange the code.
async fn connection(server: &Server, credential: &str, name: &str) -> (String, Value) {
    let pending = begin(server, name).await;
    let res = consent(server, &pending, "allow", credential)
        .send()
        .await
        .unwrap();
    let url = completion_callback(res).await;
    let code = url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    let res = exchange(
        server,
        token_body(&pending.client_id, &code, RESOURCE, VERIFIER),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    (pending.client_id, res.json().await.unwrap())
}

async fn one_recorded_request(server: &Server, access_token: &str) {
    let client = sdk_client(server, access_token).await;
    sdk_call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
    )
    .await;
    client.cancel().await.unwrap();
}

async fn recorded_requests(server: &Server, reader: &str) -> Vec<Value> {
    let client = sdk_client(server, reader).await;
    let reply = sdk_call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"audit.requests"}),
    )
    .await;
    client.cancel().await.unwrap();
    reply["data"]["items"].as_array().unwrap().clone()
}

async fn stored_name(admin: &tokio_postgres::Client, id: &str) -> Option<String> {
    admin
        .query_one(
            "SELECT client_name FROM awr_team.credentials WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &id],
        )
        .await
        .unwrap()
        .get(0)
}

fn rows_of<'a>(rows: &'a [Value], credential: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|row| row["credential_id"] == credential)
        .collect()
}

// AWR-PPD-062: the name a client registers for itself is recorded with the credential
// the member approves, and the audit read names that credential for a reader who holds
// the directory gate. The name is self-reported display text: two clients may share it
// and stay separate, a later connection keeps the first name, and nothing here changes
// what the connection may do.
#[cfg(unix)]
#[tokio::test]
async fn an_approved_connection_records_its_registered_name_with_its_credential() {
    let (_guard, admin, db, store) = setup().await;
    // The reader holds the directory gate: cli-a reads every workstream cli-b reads.
    admin
        .batch_execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
             workstream_id,authority_version,can_read)
             SELECT tenant_id,project_id,actor_id,'cli-a',workstream_id,authority_version,can_read
             FROM awr_team.workstream_grants WHERE client_id='cli-b'",
        )
        .await
        .unwrap();
    let c = format!("awr1.reader-c.{}", "d".repeat(64));
    let hash = awr_team_pg::workstream_credential_hash(&c).unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
             VALUES('reader-tenant','reader-c','agent','cli-c',$1)",
            &[&hash],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
             workstream_id,authority_version,can_read)
             VALUES('reader-tenant','reader-project','agent','cli-c',$1,1,true)",
            &[&awr_core::Id::from(1).to_string()],
        )
        .await
        .unwrap();
    let directory = std::env::temp_dir().join(format!("awr-client-name-{}", awr_core::Id::new()));
    let server = start_with_directory(store, true, Some(directory.clone())).await;

    // Registration still refuses a name that carries a control character or runs past
    // 120 characters, so only plain bounded text can ever reach a credential.
    for name in ["Synthetic\nclient".to_owned(), "x".repeat(121)] {
        let res = server
            .post("/oauth/register")
            .json(&json!({"client_name":name,"redirect_uris":[CALLBACK],
                "grant_types":["authorization_code","refresh_token"],"token_endpoint_auth_method":"none"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400);
    }
    // A name of exactly 120 characters is the longest a registration accepts.
    let longest = "n".repeat(120);
    assert_eq!(register(&server, &longest).await["client_name"], longest);
    assert_eq!(stored_name(&admin, "reader-c").await, None);

    // A successful connection without client_name keeps the presentation fallback
    // out of the credential and its audit rows, including after durable restore.
    let unnamed = server
        .post("/oauth/register")
        .json(&json!({"redirect_uris":[CALLBACK]}))
        .send()
        .await
        .unwrap();
    assert_eq!(unnamed.status(), 201);
    let unnamed: Value = unnamed.json().await.unwrap();
    let pending = begin_registered(&server, &unnamed).await;
    completion_callback(consent(&server, &pending, "allow", A).send().await.unwrap()).await;
    assert_eq!(stored_name(&admin, "reader-a").await, None);

    // Two clients register the same name and stay two clients, two connections and two
    // credentials. A third client connecting through reader-b later changes no name.
    let (first_client, first) = connection(&server, B, "Synthetic twin client").await;
    let (second_client, second) = connection(&server, &c, "Synthetic twin client").await;
    assert_ne!(first_client, second_client);
    let (_later_client, later) = connection(&server, B, "Synthetic later client").await;
    for token in [&first, &second, &later] {
        one_recorded_request(&server, token["access_token"].as_str().unwrap()).await;
    }
    assert_eq!(
        stored_name(&admin, "reader-b").await.as_deref(),
        Some("Synthetic twin client")
    );
    assert_eq!(
        stored_name(&admin, "reader-c").await.as_deref(),
        Some("Synthetic twin client")
    );
    assert_eq!(stored_name(&admin, "reader-a").await, None);
    let credentials = admin
        .query(
            "SELECT id,client_id FROM awr_team.credentials
             WHERE tenant_id=$1 AND client_name IS NOT NULL ORDER BY id",
            &[&TENANT],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
        .collect::<Vec<_>>();
    assert_eq!(
        credentials,
        vec![
            ("reader-b".to_owned(), "cli-b".to_owned()),
            ("reader-c".to_owned(), "cli-c".to_owned())
        ]
    );

    // The connection list still reads each connection's own registered name.
    let own = sdk_client(&server, B).await;
    let list = sdk_call(
        &own,
        "awr_team_connections_list",
        json!({"protocol_version":1}),
    )
    .await;
    let agents = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "agent")
        .collect::<Vec<_>>();
    let mut references = agents
        .iter()
        .map(|item| item["connection_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    references.sort();
    references.dedup();
    assert_eq!(references.len(), 2, "two connections through reader-b");
    let mut listed = agents
        .iter()
        .map(|item| item["client_name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    listed.sort();
    assert_eq!(
        listed,
        vec!["Synthetic later client", "Synthetic twin client"]
    );
    own.cancel().await.unwrap();

    // The audit names each credential exactly, under the directory gate.
    let rows = recorded_requests(&server, A).await;
    let twin = json!({"display_name":"Synthetic twin client"});
    for credential in ["reader-b", "reader-c"] {
        let made = rows_of(&rows, credential);
        assert!(!made.is_empty(), "{credential}: {rows:?}");
    }
    assert!(
        rows_of(&rows, "reader-b")
            .iter()
            .all(|row| row["client"] == twin)
    );
    assert!(
        rows_of(&rows, "reader-c")
            .iter()
            .all(|row| row["client"] == twin)
    );
    // The reader's own credential never approved a connection: its rows stay unnamed.
    assert!(!rows_of(&rows, "reader-a").is_empty());
    assert!(
        rows_of(&rows, "reader-a")
            .iter()
            .all(|row| row["client"]["display_name"].is_null())
    );
    // Every credential behind these rows belongs to the human `agent` actor.
    assert!(rows.iter().all(|row| row["actor"]["kind"] == "human"));

    // The name lives with the credential in PostgreSQL: it survives a restart that
    // restores the connection, and new requests stay named.
    server.shutdown().await;
    let store =
        awr_team_pg::WorkstreamReadStore::from_config(common::with_db(&common::test_config(), &db));
    let server = start_with_directory(store, true, Some(directory.clone())).await;
    let pending = begin_registered(&server, &unnamed).await;
    completion_callback(consent(&server, &pending, "allow", A).send().await.unwrap()).await;
    assert_eq!(stored_name(&admin, "reader-a").await, None);
    one_recorded_request(&server, first["access_token"].as_str().unwrap()).await;
    let rows = recorded_requests(&server, A).await;
    assert!(
        rows_of(&rows, "reader-b")
            .iter()
            .all(|row| row["client"] == twin)
    );

    // Without the directory gate the same rows return, with no name on any of them.
    let other_stream = awr_core::Id::from(2).to_string();
    admin
        .execute(
            "DELETE FROM awr_team.workstream_grants
             WHERE tenant_id=$1 AND project_id=$2 AND client_id='cli-a' AND workstream_id=$3",
            &[&TENANT, &PROJECT, &other_stream],
        )
        .await
        .unwrap();
    let rows = recorded_requests(&server, A).await;
    assert!(!rows_of(&rows, "reader-b").is_empty());
    assert!(
        rows.iter()
            .all(|row| row["client"]["display_name"].is_null() && row["actor"]["kind"].is_null())
    );
    assert_eq!(
        stored_name(&admin, "reader-b").await.as_deref(),
        Some("Synthetic twin client")
    );

    server.shutdown().await;
    std::fs::remove_dir_all(directory).unwrap();
}
