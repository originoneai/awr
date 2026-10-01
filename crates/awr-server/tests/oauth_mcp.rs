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
    assert_eq!(res.status(), 303);
    assert_eq!(res.headers()["cache-control"], "no-store");
    let location = res.headers()["location"].to_str().unwrap();
    assert!(!location.contains(credential));
    let url = url::Url::parse(location).unwrap();
    let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs["state"], "client state & original");
    (pending.client_id, pairs["code"].clone())
}
async fn issue(server: &Server, credential: &str) -> String {
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
    assert!(token.get("refresh_token").is_none());
    assert_eq!(exchange(server, body).send().await.unwrap().status(), 400);
    token["access_token"].as_str().unwrap().to_owned()
}
fn initialize(server: &Server, alias: &str, token: &str) -> reqwest::RequestBuilder {
    server.post(&format!("/v1/projects/{alias}/mcp")).bearer_auth(token)
        .header("accept","application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"synthetic-fixture","version":"1"}}}))
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
    let result = client
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"work.prepare","work_id":"a"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert!(result.is_error.unwrap_or(false));
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
    let html = res.text().await.unwrap();
    assert!(!html.contains(bad));
    assert!(html.contains("try again"));
    let res = consent(&server, &pending, "allow", B).send().await.unwrap();
    assert_eq!(res.status(), 303);
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
    assert_eq!(
        consent(&server, &pending, "cancel", "")
            .send()
            .await
            .unwrap()
            .status(),
        303
    );
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
