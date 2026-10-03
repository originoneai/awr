#[path = "fixtures/oauth_http.rs"]
mod fixture;
use awr_server::service::{OAuthConfig, ProjectBinding, ServiceConfig};
use awr_team_pg::WorkstreamReadStore;
use fixture::*;
use serde_json::{Value, json};

async fn server(enabled: bool) -> Server {
    start(
        WorkstreamReadStore::new("host=127.0.0.1 port=1 dbname=unused"),
        enabled,
    )
    .await
}
#[test]
fn oauth_config_is_explicit_and_requires_an_operator_bound_canonical_https_origin() {
    let base = r#"version=1
listen="127.0.0.1:0"
allowed_hosts=["awr.example"]
[[projects]]
key="one"
tenant_id="tenant"
project_id="project"
"#;
    let c: ServiceConfig = toml::from_str(base).unwrap();
    assert!(c.oauth.is_none());
    c.validate().unwrap();
    let mut c = ServiceConfig {
        version: 1,
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_hosts: vec!["awr.example".into()],
        allowed_web_origins: vec![],
        oauth: Some(OAuthConfig {
            issuer: ISSUER.into(),
            state_directory: None,
        }),
        projects: vec![ProjectBinding {
            key: "one".into(),
            tenant_id: "tenant".into(),
            project_id: "project".into(),
        }],
    };
    c.validate().unwrap();
    for issuer in [
        "http://awr.example",
        "https://awr.example/",
        "https://awr.example/path",
        "https://awr.example:443",
        "https://AWR.example",
        "https://attacker.example",
        "https://user:secret@awr.example",
        "https://awr.example?x=1",
        "https://awr.example#x",
    ] {
        c.oauth = Some(OAuthConfig {
            issuer: issuer.into(),
            state_directory: None,
        });
        assert!(c.validate().is_err(), "{issuer}");
    }
    c.oauth = Some(OAuthConfig {
        issuer: ISSUER.into(),
        state_directory: Some("relative-state".into()),
    });
    assert!(c.validate().is_err());
}
#[tokio::test]
async fn discovery_is_opt_in_and_challenges_bind_exact_project_metadata() {
    let disabled = server(false).await;
    assert_eq!(
        disabled
            .get("/.well-known/oauth-authorization-server")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        disabled
            .post("/v1/projects/one/mcp")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let s = server(true).await;
    let res = s.post("/v1/projects/one/mcp").send().await.unwrap();
    assert_eq!(res.status(), 401);
    assert_eq!(
        res.headers()["www-authenticate"],
        format!(
            "Bearer resource_metadata=\"{ISSUER}/.well-known/oauth-protected-resource/v1/projects/one/mcp\""
        )
    );
    let res = s
        .get("/.well-known/oauth-authorization-server")
        .header("forwarded", "host=attacker.example;proto=http")
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["cache-control"], "no-store");
    let metadata: Value = res.json().await.unwrap();
    assert_eq!(metadata["issuer"], ISSUER);
    assert_eq!(
        metadata["code_challenge_methods_supported"],
        json!(["S256"])
    );
    assert_eq!(
        metadata["grant_types_supported"],
        json!(["authorization_code", "refresh_token"])
    );
    let resource: Value = s
        .get("/.well-known/oauth-protected-resource/v1/projects/one/mcp")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resource["resource"], RESOURCE);
    assert_eq!(resource["authorization_servers"], json!([ISSUER]));
    assert_eq!(
        s.get("/.well-known/oauth-protected-resource/v1/projects/missing/mcp")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        s.get("/.well-known/oauth-authorization-server")
            .header("host", "attacker.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        s.get("/.well-known/oauth-authorization-server")
            .header("origin", "https://attacker.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let bad = s
        .post("/v1/projects/one/mcp")
        .bearer_auth("awr_oauth_invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 401);
    assert!(
        bad.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("invalid_token")
    );
}
#[tokio::test]
async fn registration_never_trusts_or_fetches_client_metadata_and_narrows_grants() {
    let s = server(true).await;
    let client = register(&s, "<img src=x onerror=alert(1)> & \"client\"").await;
    assert_eq!(
        client["grant_types"],
        json!(["authorization_code", "refresh_token"])
    );
    assert!(client.get("client_secret").is_none());
    for body in [
        json!({"redirect_uris":["https://client.example/callback"],"token_endpoint_auth_method":"client_secret_post"}),
        json!({"redirect_uris":["https://client.example/callback"],"grant_types":["refresh_token"]}),
        json!({"redirect_uris":["https://client.example/callback"],"grant_types":["authorization_code","client_credentials"]}),
        json!({"redirect_uris":["http://remote.example/callback"]}),
        json!({"redirect_uris":["https://client.example/callback"],"scope":"admin"}),
    ] {
        let res = s.post("/oauth/register").json(&body).send().await.unwrap();
        assert_eq!(res.status(), 400);
        let text = res.text().await.unwrap();
        assert!(!text.contains("client_secret_post"));
    }
    let pending = begin(&s, "<script>alert('x')</script> & client").await;
    assert!(
        pending
            .html
            .contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; client")
    );
    assert!(!pending.html.contains("<script>"));
    assert!(!pending.html.contains("<img"));
    assert!(pending.html.contains("type=\"password\""));
    assert!(pending.html.contains("formnovalidate"));
    assert!(pending.html.contains("Access tokens last up to one hour."));
    assert!(pending.html.contains("up to 24 hours from approval"));
}
#[tokio::test]
async fn authorization_rejects_duplicates_and_unbound_redirects_without_redirecting() {
    let s = server(true).await;
    let c = register(&s, "fixture").await;
    let id = c["client_id"].as_str().unwrap();
    let valid = auth_query(id);
    for query in [
        format!("{valid}&client_id=another"),
        format!("{valid}&%72esource={RESOURCE}"),
        valid.replace("S256", "plain"),
        valid.replace("client.example", "attacker.example"),
        valid.replace("response_type=code", "response_type=token"),
        valid.replace("scope=awr.project", "scope=admin"),
    ] {
        let res = s
            .get(&format!("/oauth/authorize?{query}"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400);
        assert!(res.headers().get("location").is_none());
        assert!(res.headers().get("set-cookie").is_none());
        let error = res.text().await.unwrap();
        assert!(!error.contains(id));
    }
}
#[tokio::test]
async fn consent_checks_origin_cookie_and_transaction_then_cancels_atomically() {
    let s = server(true).await;
    let pending = begin(&s, "fixture").await;
    let other = begin(&s, "other").await;
    for request in [
        consent(&s, &pending, "cancel", "").header("origin", "https://attacker.example"),
        consent(&s, &pending, "cancel", "").header("sec-fetch-site", "cross-site"),
        consent(&s, &pending, "cancel", "").header("cookie", &other.cookie),
        consent(&s, &pending, "cancel", "")
            .header("cookie", format!("{}; {}", pending.cookie, pending.cookie)),
    ] {
        let res = request.send().await.unwrap();
        assert_eq!(res.status(), 403);
        assert!(res.headers().get("location").is_none());
    }
    let body = encode(&[("transaction", &pending.transaction), ("action", "cancel")]);
    let res = s
        .post("/oauth/consent")
        .header("cookie", &pending.cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "Origin is mandatory for consent");
    let res = consent(&s, &pending, "cancel", "")
        .body(format!("{body}&action=allow"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let res = consent(&s, &pending, "cancel", "").send().await.unwrap();
    let callback = completion_callback(res).await;
    assert_eq!(callback.host_str(), Some("client.example"));
    let pairs: std::collections::BTreeMap<_, _> = callback.query_pairs().into_owned().collect();
    assert_eq!(pairs["error"], "access_denied");
    assert_eq!(pairs["state"], "client state & original");
    assert!(!pairs.contains_key("code"));
    assert_eq!(
        consent(&s, &pending, "cancel", "")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    completion_callback(consent(&s, &other, "cancel", "").send().await.unwrap()).await;
}
#[tokio::test]
async fn opaque_origin_is_rejected_without_consuming_the_transaction() {
    let s = server(true).await;
    let pending = begin(&s, "fixture").await;
    let res = s
        .post("/oauth/consent")
        .header("origin", "null")
        .header("cookie", &pending.cookie)
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(encode(&[
            ("transaction", &pending.transaction),
            ("action", "cancel"),
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert!(res.headers().get("location").is_none());
    let res = consent(&s, &pending, "cancel", "").send().await.unwrap();
    completion_callback(res).await;
}
#[tokio::test]
async fn completion_preserves_registered_callback_query_and_untrusted_state() {
    let s = server(true).await;
    let callback = "https://client.example/return?channel=awr&label=%27quoted%27";
    let state = "<svg onload=alert(1)>\"' &; client state";
    let client: Value = s
        .post("/oauth/register")
        .json(&json!({"redirect_uris":[callback],"token_endpoint_auth_method":"none"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let query = auth_query(client["client_id"].as_str().unwrap());
    let pairs: Vec<_> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| {
            let value = match key.as_ref() {
                "redirect_uri" => callback.into(),
                "state" => state.into(),
                _ => value,
            };
            (key, value)
        })
        .collect();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    let res = s
        .get(&format!("/oauth/authorize?{query}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html = res.text().await.unwrap();
    let transaction = html
        .split("name=\"transaction\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let res = s
        .post("/oauth/consent")
        .header("origin", ISSUER)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(encode(&[
            ("transaction", transaction),
            ("action", "cancel"),
        ]))
        .send()
        .await
        .unwrap();
    let result = completion_callback(res).await;
    assert_eq!(result.origin(), url::Url::parse(callback).unwrap().origin());
    assert_eq!(result.path(), "/return");
    let pairs: std::collections::BTreeMap<_, _> = result.query_pairs().into_owned().collect();
    assert_eq!(pairs["channel"], "awr");
    assert_eq!(pairs["label"], "'quoted'");
    assert_eq!(pairs["state"], state);
    assert_eq!(pairs["error"], "access_denied");
    assert_eq!(pairs.len(), 4);
}
#[tokio::test]
async fn request_limits_and_token_input_errors_never_echo_values() {
    let s = server(true).await;
    let res = s
        .post("/oauth/register")
        .header("content-type", "application/json")
        .body("x".repeat(16385))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert_eq!(
        s.get(&format!("/oauth/authorize?x={}", "x".repeat(16385)))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    for body in [
        "grant_type=refresh_token&refresh_token=private-test-sentinel".to_owned(),
        "grant_type=refresh_token&client_id=client&refresh_token=one&refresh_token=private-test-sentinel".to_owned(),
        "grant_type=refresh_token&client_id=client&client_id=private-test-sentinel&refresh_token=one".to_owned(),
        "grant_type=refresh_token&client_id=client&refresh_token=one&resource=one&resource=private-test-sentinel".to_owned(),
        "grant_type=refresh_token&client_id=client&refresh_token=one&scope=private-test-sentinel".to_owned(),
        "grant_type=refresh_token&client_id=client&refresh_token=one&client_secret=private-test-sentinel".to_owned(),
        token_body("client", "code", RESOURCE, VERIFIER) + "&resource=private-test-sentinel",
        token_body("client", "code", RESOURCE, VERIFIER) + "&client_secret=private-test-sentinel",
    ] {
        let res = exchange(&s, body).send().await.unwrap();
        assert_eq!(res.status(), 400);
        assert!(!res.text().await.unwrap().contains("private-test-sentinel"));
    }
}
