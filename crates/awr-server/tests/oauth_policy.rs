use awr_server::oauth::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const RESOURCE: &str = "https://awr.example/v1/projects/one/mcp";
const OTHER: &str = "https://awr.example/v1/projects/two/mcp";
const CALLBACK: &str = "https://client.example/oauth/callback?channel=awr";
const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
const SYNTHETIC_BEARER: &str = "synthetic-fixture-only-never-a-real-credential";

fn store() -> OAuthStore {
    OAuthStore::new(vec![RESOURCE.into(), OTHER.into()]).unwrap()
}
fn client(store: &OAuthStore, now: Instant) -> RegisteredClient {
    store
        .register("Fixture client".into(), vec![CALLBACK.into()], now)
        .unwrap()
}
fn request(client: &RegisteredClient) -> AuthorizationRequest {
    AuthorizationRequest {
        client_id: client.client_id.clone(),
        redirect_uri: CALLBACK.into(),
        resource: RESOURCE.into(),
        state: Some("client-owned-state".into()),
        code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes())),
        code_challenge_method: "S256".into(),
    }
}
fn issue(store: &OAuthStore, client: &RegisteredClient, now: Instant) -> TokenRequest {
    let page = store.begin(request(client), now).unwrap();
    let code = store
        .approve(
            &page.transaction_id,
            &page.cookie,
            SYNTHETIC_BEARER.into(),
            now,
        )
        .unwrap();
    assert_eq!(code.redirect_uri, CALLBACK);
    assert_eq!(code.state.as_deref(), Some("client-owned-state"));
    TokenRequest {
        code: code.code,
        client_id: client.client_id.clone(),
        redirect_uri: CALLBACK.into(),
        resource: RESOURCE.into(),
        code_verifier: VERIFIER.into(),
    }
}

#[test]
fn exchange_is_single_use_resource_bound_and_never_returns_original_credential() {
    let store = store();
    let now = Instant::now();
    let client = client(&store, now);
    let input = issue(&store, &client, now);
    let duplicate = TokenRequest {
        ..issue(&store, &client, now)
    };
    let original_code = input.code.clone();
    let access = store.exchange(input, now).unwrap();
    assert_eq!(access.token_type, "Bearer");
    assert_eq!(access.scope, "awr.project");
    assert_eq!(access.expires_in, 3600);
    let wire = serde_json::to_string(&access).unwrap();
    assert!(!wire.contains(SYNTHETIC_BEARER));
    assert!(!wire.contains(&original_code));
    assert_eq!(
        store
            .resolve(&access.access_token, RESOURCE, now)
            .as_deref(),
        Some(SYNTHETIC_BEARER)
    );
    assert_eq!(store.resolve(&access.access_token, OTHER, now), None);
    assert_eq!(
        store.resolve(&access.access_token, RESOURCE, now + ACCESS_TTL),
        None
    );
    assert_eq!(store.resolve(SYNTHETIC_BEARER, RESOURCE, now), None);
    assert!(matches!(
        store.exchange(
            TokenRequest {
                code: original_code,
                ..duplicate
            },
            now
        ),
        Err(OAuthError::InvalidGrant)
    ));
}

#[test]
fn every_code_binding_and_verifier_is_enforced() {
    let store = store();
    let now = Instant::now();
    let client = client(&store, now);
    for field in ["client", "redirect", "resource", "verifier"] {
        let mut input = issue(&store, &client, now);
        let code = input.code.clone();
        match field {
            "client" => input.client_id = "unknown-client".into(),
            "redirect" => input.redirect_uri = "https://client.example/another".into(),
            "resource" => input.resource = OTHER.into(),
            "verifier" => input.code_verifier = "z".repeat(43),
            _ => unreachable!(),
        }
        assert!(
            matches!(store.exchange(input, now), Err(OAuthError::InvalidGrant)),
            "{field}"
        );
        let valid = issue(&store, &client, now);
        assert!(
            matches!(
                store.exchange(TokenRequest { code, ..valid }, now),
                Err(OAuthError::InvalidGrant)
            ),
            "failed attempt must consume the code"
        );
    }
}

#[test]
fn pkce_downgrades_invalid_targets_and_unregistered_callbacks_fail_before_consent() {
    let store = store();
    let now = Instant::now();
    let client = client(&store, now);
    for variant in [
        "plain",
        "no-method",
        "short",
        "padding",
        "bad-base64",
        "redirect",
        "resource",
        "unknown-client",
        "state",
    ] {
        let mut input = request(&client);
        match variant {
            "plain" => input.code_challenge_method = "plain".into(),
            "no-method" => input.code_challenge_method.clear(),
            "short" => input.code_challenge = "x".repeat(42),
            "padding" => input.code_challenge.push('='),
            "bad-base64" => input.code_challenge = "!".repeat(43),
            "redirect" => input.redirect_uri = "https://attacker.example/callback".into(),
            "resource" => input.resource = "https://awr.example/v1/projects/one/mcp/".into(),
            "unknown-client" => input.client_id = "unknown".into(),
            "state" => input.state = Some("bad\nstate".into()),
            _ => unreachable!(),
        }
        assert!(store.begin(input, now).is_err(), "{variant}");
    }
    let mut without_state = request(&client);
    without_state.state = None;
    assert!(
        store.begin(without_state, now).is_ok(),
        "browser binding does not depend on optional client state"
    );
}

#[test]
fn consent_requires_its_browser_and_cannot_be_replayed_or_extended() {
    let store = store();
    let now = Instant::now();
    let client = client(&store, now);
    let page = store.begin(request(&client), now).unwrap();
    let other = store.begin(request(&client), now).unwrap();
    assert!(
        store
            .consent(&page.transaction_id, &other.cookie, now)
            .is_err()
    );
    assert!(
        store
            .approve(
                &page.transaction_id,
                &other.cookie,
                SYNTHETIC_BEARER.into(),
                now
            )
            .is_err()
    );
    assert!(
        store
            .consent(&page.transaction_id, &page.cookie, now)
            .is_ok()
    );
    store
        .approve(
            &page.transaction_id,
            &page.cookie,
            SYNTHETIC_BEARER.into(),
            now,
        )
        .unwrap();
    assert!(
        store
            .consent(&page.transaction_id, &page.cookie, now)
            .is_err()
    );
    assert!(
        store
            .approve(
                &page.transaction_id,
                &page.cookie,
                SYNTHETIC_BEARER.into(),
                now
            )
            .is_err()
    );
    assert!(
        store
            .consent(&other.transaction_id, &other.cookie, now + CONSENT_TTL)
            .is_err()
    );
    assert!(
        store
            .approve(
                &other.transaction_id,
                &other.cookie,
                SYNTHETIC_BEARER.into(),
                now + CONSENT_TTL
            )
            .is_err()
    );
    let input = issue(&store, &client, now);
    assert!(matches!(
        store.exchange(input, now + Duration::from_secs(120)),
        Err(OAuthError::InvalidGrant)
    ));
}

#[test]
fn callbacks_allow_https_and_literal_loopback_without_fetching_or_normalizing() {
    for valid in [
        CALLBACK,
        "http://localhost:1234/callback",
        "http://127.0.0.1:4321/callback",
        "http://[::1]:3210/callback",
    ] {
        assert!(valid_redirect(valid), "{valid}");
    }
    for invalid in [
        "http://remote.example/callback",
        "https://user:password@client.example/callback",
        "https://client.example/callback#fragment",
        "javascript:alert(1)",
        "https://client.example/callback\n",
        "https://CLIENT.example/callback",
        "https://client.example/callback?code=existing",
        "http://127.0.0.2:1234/callback",
        "https://client.example/callback?%73tate=existing",
    ] {
        assert!(!valid_redirect(invalid), "{invalid}");
    }
    assert!(OAuthStore::new(vec!["http://awr.example/mcp".into()]).is_err());
    assert!(OAuthStore::new(vec![format!("{RESOURCE}?project=other")]).is_err());
}

#[test]
fn registration_and_pending_capacity_recovers_only_after_expiry() {
    let store = store();
    let now = Instant::now();
    let client = client(&store, now);
    for _ in 1..1024 {
        store
            .register("Fixture".into(), vec![CALLBACK.into()], now)
            .unwrap();
    }
    assert!(matches!(
        store.register("Full".into(), vec![CALLBACK.into()], now),
        Err(OAuthError::Unavailable)
    ));
    for _ in 0..1024 {
        store.begin(request(&client), now).unwrap();
    }
    assert!(matches!(
        store.begin(request(&client), now),
        Err(OAuthError::Unavailable)
    ));
    store.begin(request(&client), now + CONSENT_TTL).unwrap();
    assert!(
        store
            .begin(request(&client), now + Duration::from_secs(86400))
            .is_err()
    );
    store
        .register(
            "After expiry".into(),
            vec![CALLBACK.into()],
            now + Duration::from_secs(86400),
        )
        .unwrap();
}

#[test]
fn cancellation_checks_browser_binding_and_consumes_only_its_transaction() {
    let store = store();
    let now = Instant::now();
    let client = client(&store, now);
    let page = store.begin(request(&client), now).unwrap();
    let other = store.begin(request(&client), now).unwrap();
    assert!(
        store
            .cancel(&page.transaction_id, &other.cookie, now)
            .is_err()
    );
    let cancelled = store
        .cancel(&page.transaction_id, &page.cookie, now)
        .unwrap();
    assert_eq!(cancelled.request.redirect_uri, CALLBACK);
    assert_eq!(
        cancelled.request.state.as_deref(),
        Some("client-owned-state")
    );
    assert!(
        store
            .cancel(&page.transaction_id, &page.cookie, now)
            .is_err()
    );
    assert!(
        store
            .approve(
                &page.transaction_id,
                &page.cookie,
                SYNTHETIC_BEARER.into(),
                now
            )
            .is_err()
    );
    assert!(
        store
            .consent(&other.transaction_id, &other.cookie, now)
            .is_ok()
    );
    assert!(
        store
            .cancel(&other.transaction_id, &other.cookie, now + CONSENT_TTL)
            .is_err()
    );
}

#[test]
fn code_and_token_capacity_is_bounded_and_expiry_releases_slots() {
    let store = store();
    let now = Instant::now();
    let client = client(&store, now);
    for _ in 0..1024 {
        issue(&store, &client, now);
    }
    let page = store.begin(request(&client), now).unwrap();
    assert!(matches!(
        store.approve(
            &page.transaction_id,
            &page.cookie,
            SYNTHETIC_BEARER.into(),
            now
        ),
        Err(OAuthError::Unavailable)
    ));
    store
        .approve(
            &page.transaction_id,
            &page.cookie,
            SYNTHETIC_BEARER.into(),
            now + Duration::from_secs(120),
        )
        .unwrap();

    let later = now + Duration::from_secs(121);
    for _ in 0..4096 {
        let input = issue(&store, &client, later);
        store.exchange(input, later).unwrap();
    }
    let input = issue(&store, &client, later);
    assert!(matches!(
        store.exchange(input, later),
        Err(OAuthError::Unavailable)
    ));
    // A connection remains refreshable after its access token expires, so its
    // family capacity is released only at the fixed connection deadline.
    let after_expiry = later + CONNECTION_TTL;
    let new_client = self::client(&store, after_expiry);
    let input = issue(&store, &new_client, after_expiry);
    store.exchange(input, after_expiry).unwrap();
}

#[test]
fn concurrent_redemptions_issue_exactly_one_token_and_restart_invalidates_it() {
    let store = Arc::new(store());
    let now = Instant::now();
    let client = client(&store, now);
    let input = issue(&store, &client, now);
    let code = input.code;
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let (store, barrier, code, client_id) = (
                store.clone(),
                barrier.clone(),
                code.clone(),
                client.client_id.clone(),
            );
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .exchange(
                        TokenRequest {
                            code,
                            client_id,
                            redirect_uri: CALLBACK.into(),
                            resource: RESOURCE.into(),
                            code_verifier: VERIFIER.into(),
                        },
                        now,
                    )
                    .ok()
            })
        })
        .collect();
    let tokens: Vec<_> = workers
        .into_iter()
        .filter_map(|w| w.join().unwrap())
        .collect();
    assert_eq!(tokens.len(), 1);
    let fresh = OAuthStore::new(vec![RESOURCE.into()]).unwrap();
    assert!(
        fresh
            .resolve(&tokens[0].access_token, RESOURCE, now)
            .is_none()
    );
}
