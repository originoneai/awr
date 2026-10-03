use awr_server::oauth::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const RESOURCE: &str = "https://awr.example/v1/projects/one/mcp";
const OTHER: &str = "https://awr.example/v1/projects/two/mcp";
const CALLBACK: &str = "https://client.example/callback";
const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
const BEARER: &str = "synthetic-refresh-fixture-not-a-real-credential";

fn issue(store: &OAuthStore, now: Instant) -> (RegisteredClient, AccessToken) {
    let client = store
        .register("Fixture".into(), vec![CALLBACK.into()], now)
        .unwrap();
    let page = store
        .begin(
            AuthorizationRequest {
                client_id: client.client_id.clone(),
                redirect_uri: CALLBACK.into(),
                resource: RESOURCE.into(),
                state: None,
                code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes())),
                code_challenge_method: "S256".into(),
            },
            now,
        )
        .unwrap();
    let code = store
        .approve(&page.transaction_id, &page.cookie, BEARER.into(), now)
        .unwrap();
    let token = store
        .exchange(
            TokenRequest {
                code: code.code,
                client_id: client.client_id.clone(),
                redirect_uri: CALLBACK.into(),
                resource: RESOURCE.into(),
                code_verifier: VERIFIER.into(),
            },
            now,
        )
        .unwrap();
    (client, token)
}

fn request(client: &RegisteredClient, token: &AccessToken) -> RefreshRequest {
    RefreshRequest {
        client_id: client.client_id.clone(),
        refresh_token: token.refresh_token.clone(),
        resource: Some(RESOURCE.into()),
    }
}

#[test]
fn expired_access_can_refresh_without_consent_and_keeps_exact_resource() {
    let store = OAuthStore::new(vec![RESOURCE.into(), OTHER.into()]).unwrap();
    let now = Instant::now();
    let (client, token) = issue(&store, now);
    let later = now + ACCESS_TTL;
    assert!(
        store
            .resolve(&token.access_token, RESOURCE, later)
            .is_none()
    );
    let mut input = request(&client, &token);
    input.resource = None; // An omitted RFC 8707 resource retains the original binding.
    let access = store.refresh_access(&input, later).unwrap();
    assert_eq!(access.resource, RESOURCE);
    assert_eq!(access.bearer, BEARER);
    let renewed = store.refresh(input, later).unwrap();
    assert_eq!(renewed.expires_in, 3600);
    assert_ne!(renewed.refresh_token, token.refresh_token);
    assert_ne!(renewed.access_token, token.access_token);
    assert_eq!(
        store
            .resolve(&renewed.access_token, RESOURCE, later)
            .as_deref(),
        Some(BEARER)
    );
    assert!(store.resolve(&renewed.access_token, OTHER, later).is_none());
    assert!(!serde_json::to_string(&renewed).unwrap().contains(BEARER));
}

#[test]
fn foreign_client_or_resource_cannot_rotate_or_revoke_a_connection() {
    let store = OAuthStore::new(vec![RESOURCE.into(), OTHER.into()]).unwrap();
    let now = Instant::now();
    let (client, token) = issue(&store, now);
    for mismatch in ["client", "resource"] {
        let mut input = request(&client, &token);
        if mismatch == "client" {
            input.client_id = "foreign-client".into();
        } else {
            input.resource = Some(OTHER.into());
        }
        assert!(matches!(
            store.refresh_access(&input, now),
            Err(OAuthError::InvalidGrant)
        ));
        assert!(matches!(
            store.refresh(input, now),
            Err(OAuthError::InvalidGrant)
        ));
        assert_eq!(
            store.resolve(&token.access_token, RESOURCE, now).as_deref(),
            Some(BEARER)
        );
    }
    let renewed = store.refresh(request(&client, &token), now).unwrap();
    let mut foreign_replay = request(&client, &token);
    foreign_replay.client_id = "foreign-client".into();
    assert!(store.refresh(foreign_replay, now).is_err());
    assert!(
        store
            .resolve(&renewed.access_token, RESOURCE, now)
            .is_some()
    );
    assert!(matches!(
        store.refresh_access(&request(&client, &token), now),
        Err(OAuthError::InvalidGrant)
    ));
    assert!(store.resolve(&token.access_token, RESOURCE, now).is_none());
    assert!(
        store
            .resolve(&renewed.access_token, RESOURCE, now)
            .is_none()
    );
    assert!(store.refresh(request(&client, &renewed), now).is_err());
}

#[test]
fn rotation_never_extends_the_approved_deadline_and_restart_requires_consent() {
    let store = OAuthStore::new(vec![RESOURCE.into()]).unwrap();
    let now = Instant::now();
    let (client, token) = issue(&store, now);
    let near_deadline = now + CONNECTION_TTL - Duration::from_secs(30);
    let renewed = store
        .refresh(request(&client, &token), near_deadline)
        .unwrap();
    assert_eq!(renewed.expires_in, 30);
    let deadline = now + CONNECTION_TTL;
    assert!(
        store
            .resolve(&renewed.access_token, RESOURCE, deadline)
            .is_none()
    );
    assert!(matches!(
        store.refresh(request(&client, &renewed), deadline),
        Err(OAuthError::InvalidGrant)
    ));
    let fresh = OAuthStore::new(vec![RESOURCE.into()]).unwrap();
    assert!(
        fresh
            .refresh_access(&request(&client, &renewed), near_deadline)
            .is_err()
    );
}

#[test]
fn late_approval_is_clipped_to_the_original_registration_deadline() {
    let store = OAuthStore::new(vec![RESOURCE.into()]).unwrap();
    let now = Instant::now();
    let client = store
        .register("Fixture".into(), vec![CALLBACK.into()], now)
        .unwrap();
    let approval = now + Duration::from_secs(12 * 3600);
    let page = store
        .begin(
            AuthorizationRequest {
                client_id: client.client_id.clone(),
                redirect_uri: CALLBACK.into(),
                resource: RESOURCE.into(),
                state: None,
                code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes())),
                code_challenge_method: "S256".into(),
            },
            approval,
        )
        .unwrap();
    let code = store
        .approve(&page.transaction_id, &page.cookie, BEARER.into(), approval)
        .unwrap();
    let token = store
        .exchange(
            TokenRequest {
                code: code.code,
                client_id: client.client_id.clone(),
                redirect_uri: CALLBACK.into(),
                resource: RESOURCE.into(),
                code_verifier: VERIFIER.into(),
            },
            approval,
        )
        .unwrap();
    let near_registration_expiry = now + CONNECTION_TTL - Duration::from_secs(30);
    let renewed = store
        .refresh(request(&client, &token), near_registration_expiry)
        .unwrap();
    assert_eq!(renewed.expires_in, 30);
    assert!(
        store
            .refresh(request(&client, &renewed), now + CONNECTION_TTL)
            .is_err()
    );
}

#[test]
fn concurrent_refreshes_issue_at_most_once_and_reuse_revokes_the_family() {
    let store = Arc::new(OAuthStore::new(vec![RESOURCE.into()]).unwrap());
    let now = Instant::now();
    let (client, token) = issue(&store, now);
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let (store, barrier, input) =
                (store.clone(), barrier.clone(), request(&client, &token));
            std::thread::spawn(move || {
                barrier.wait();
                store.refresh(input, now).ok()
            })
        })
        .collect();
    let issued: Vec<_> = workers
        .into_iter()
        .filter_map(|w| w.join().unwrap())
        .collect();
    assert_eq!(issued.len(), 1);
    assert!(
        store
            .resolve(&issued[0].access_token, RESOURCE, now)
            .is_none()
    );
    assert!(store.refresh(request(&client, &issued[0]), now).is_err());
}
