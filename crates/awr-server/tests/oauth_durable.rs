#![cfg(unix)]
use awr_server::oauth::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    time::{Duration, Instant},
};

const RESOURCE: &str = "https://awr.example/v1/projects/one/mcp";
const OTHER: &str = "https://awr.example/v1/projects/two/mcp";
const CALLBACK: &str = "https://client.example/callback";
const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
const BEARER: &str = "synthetic-durable-fixture-not-a-real-credential";

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        Self(std::env::temp_dir().join(format!(
            "awr-oauth-{}-{}",
            std::process::id(),
            URL_SAFE_NO_PAD.encode(nonce)
        )))
    }
    fn open(&self) -> OAuthStore {
        OAuthStore::open(
            vec![RESOURCE.into(), OTHER.into()],
            &self.0,
            "synthetic-project-mapping",
        )
        .unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn authorization(client: &RegisteredClient) -> AuthorizationRequest {
    AuthorizationRequest {
        client_id: client.client_id.clone(),
        redirect_uri: CALLBACK.into(),
        resource: RESOURCE.into(),
        state: None,
        code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes())),
        code_challenge_method: "S256".into(),
    }
}
fn redeem(
    store: &OAuthStore,
    client: &RegisteredClient,
    code: AuthorizationCode,
    now: Instant,
) -> AccessToken {
    store
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
        .unwrap()
}
fn issue(store: &OAuthStore, now: Instant) -> (RegisteredClient, AccessToken) {
    let client = store
        .register("Fixture".into(), vec![CALLBACK.into()], now)
        .unwrap();
    let page = store.begin(authorization(&client), now).unwrap();
    let code = store
        .approve(&page.transaction_id, &page.cookie, BEARER.into(), now)
        .unwrap();
    let token = redeem(store, &client, code, now);
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
fn restart_restores_access_registration_and_rotation_without_plaintext_secrets() {
    let directory = Directory::new();
    let store = directory.open();
    let (client, token) = issue(&store, Instant::now());
    let bytes = fs::read(directory.0.join("state.bin")).unwrap();
    for secret in [
        BEARER,
        &token.access_token,
        &token.refresh_token,
        &client.client_id,
    ] {
        assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
    }
    assert_eq!(
        fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["key.bin", "state.bin", "writer.lock"] {
        assert_eq!(
            fs::metadata(directory.0.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(store);
    let store = directory.open();
    assert_eq!(
        store
            .resolve(&token.access_token, RESOURCE, Instant::now())
            .as_deref(),
        Some(BEARER)
    );
    assert!(
        store
            .resolve(&token.access_token, OTHER, Instant::now())
            .is_none()
    );
    store.begin(authorization(&client), Instant::now()).unwrap();
    let rotated = store
        .refresh(request(&client, &token), Instant::now())
        .unwrap();
    drop(store);
    let store = directory.open();
    assert_eq!(
        store
            .resolve(&rotated.access_token, RESOURCE, Instant::now())
            .as_deref(),
        Some(BEARER)
    );
}

#[test]
fn consumed_replay_revocation_is_committed_even_when_the_result_is_an_error() {
    for preflight in [false, true] {
        let directory = Directory::new();
        let store = directory.open();
        let (client, token) = issue(&store, Instant::now());
        let rotated = store
            .refresh(request(&client, &token), Instant::now())
            .unwrap();
        drop(store);
        let store = directory.open();
        let mut foreign = request(&client, &token);
        foreign.client_id = "different-client".into();
        assert!(store.refresh(foreign, Instant::now()).is_err());
        let mut foreign = request(&client, &token);
        foreign.resource = Some(OTHER.into());
        assert!(store.refresh_access(&foreign, Instant::now()).is_err());
        assert!(
            store
                .resolve(&rotated.access_token, RESOURCE, Instant::now())
                .is_some()
        );
        if preflight {
            assert!(matches!(
                store.refresh_access(&request(&client, &token), Instant::now()),
                Err(OAuthError::InvalidGrant)
            ));
        } else {
            assert!(matches!(
                store.refresh(request(&client, &token), Instant::now()),
                Err(OAuthError::InvalidGrant)
            ));
        }
        drop(store);
        let store = directory.open();
        assert!(
            store
                .resolve(&token.access_token, RESOURCE, Instant::now())
                .is_none()
        );
        assert!(
            store
                .resolve(&rotated.access_token, RESOURCE, Instant::now())
                .is_none()
        );
        assert!(
            store
                .refresh(request(&client, &rotated), Instant::now())
                .is_err()
        );
    }
}

#[test]
fn restart_and_rotation_preserve_original_absolute_deadlines() {
    let directory = Directory::new();
    let store = directory.open();
    let now = Instant::now();
    let (client, token) = issue(&store, now);
    drop(store);
    let store = directory.open();
    assert!(
        store
            .resolve(
                &token.access_token,
                RESOURCE,
                now + ACCESS_TTL + Duration::from_secs(1)
            )
            .is_none()
    );
    let near = now + CONNECTION_TTL - Duration::from_secs(30);
    let rotated = store.refresh(request(&client, &token), near).unwrap();
    assert!((28..=30).contains(&rotated.expires_in));
    drop(store);
    let store = directory.open();
    let expired = now + CONNECTION_TTL + Duration::from_secs(1);
    assert!(
        store
            .resolve(&rotated.access_token, RESOURCE, expired)
            .is_none()
    );
    assert!(store.refresh(request(&client, &rotated), expired).is_err());
}

#[test]
fn pending_consent_and_unredeemed_codes_are_not_restored() {
    let directory = Directory::new();
    let store = directory.open();
    let now = Instant::now();
    let client = store
        .register("Fixture".into(), vec![CALLBACK.into()], now)
        .unwrap();
    let pending = store.begin(authorization(&client), now).unwrap();
    let approved = store.begin(authorization(&client), now).unwrap();
    let code = store
        .approve(
            &approved.transaction_id,
            &approved.cookie,
            BEARER.into(),
            now,
        )
        .unwrap();
    drop(store);
    let store = directory.open();
    assert!(
        store
            .consent(&pending.transaction_id, &pending.cookie, Instant::now())
            .is_err()
    );
    assert!(matches!(
        store.exchange(
            TokenRequest {
                code: code.code,
                client_id: client.client_id,
                redirect_uri: CALLBACK.into(),
                resource: RESOURCE.into(),
                code_verifier: VERIFIER.into(),
            },
            Instant::now()
        ),
        Err(OAuthError::InvalidGrant)
    ));
}

#[test]
fn corruption_wrong_key_and_missing_state_fail_closed_without_resetting() {
    for damage in ["corrupt", "key", "missing-key", "missing-state"] {
        let directory = Directory::new();
        let store = directory.open();
        issue(&store, Instant::now());
        drop(store);
        let state = directory.0.join("state.bin");
        let key = directory.0.join("key.bin");
        match damage {
            "corrupt" => {
                let mut bytes = fs::read(&state).unwrap();
                *bytes.last_mut().unwrap() ^= 1;
                fs::write(&state, bytes).unwrap();
            }
            "key" => fs::write(&key, [0u8; 32]).unwrap(),
            "missing-key" => fs::remove_file(&key).unwrap(),
            "missing-state" => fs::remove_file(&state).unwrap(),
            _ => unreachable!(),
        }
        let before = fs::read(&state).ok();
        assert!(
            OAuthStore::open(
                vec![RESOURCE.into(), OTHER.into()],
                &directory.0,
                "synthetic-project-mapping"
            )
            .is_err()
        );
        assert_eq!(fs::read(&state).ok(), before);
    }
}

#[test]
fn exclusive_writer_and_exact_deployment_binding_are_required() {
    let directory = Directory::new();
    let store = directory.open();
    assert!(
        OAuthStore::open(
            vec![RESOURCE.into(), OTHER.into()],
            &directory.0,
            "synthetic-project-mapping"
        )
        .is_err()
    );
    drop(store);
    for (resources, binding) in [
        (vec![RESOURCE.into()], "synthetic-project-mapping"),
        (
            vec![RESOURCE.into(), OTHER.into()],
            "changed-tenant-project",
        ),
    ] {
        assert!(OAuthStore::open(resources, &directory.0, binding).is_err());
    }
    directory.open();
}

#[test]
fn unsafe_permissions_symlinks_and_hardlinks_are_rejected() {
    for target in ["directory", "key.bin", "state.bin", "writer.lock"] {
        let directory = Directory::new();
        let store = directory.open();
        drop(store);
        let path = if target == "directory" {
            directory.0.clone()
        } else {
            directory.0.join(target)
        };
        let before = fs::metadata(&path).unwrap().permissions();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            OAuthStore::open(
                vec![RESOURCE.into(), OTHER.into()],
                &directory.0,
                "synthetic-project-mapping"
            )
            .is_err()
        );
        fs::set_permissions(&path, before).unwrap();
        directory.open();
    }
    for link_type in ["symlink", "hardlink"] {
        let directory = Directory::new();
        let store = directory.open();
        drop(store);
        let path = directory.0.join("state.bin");
        let copy = directory.0.join("linked.bin");
        if link_type == "symlink" {
            fs::rename(&path, &copy).unwrap();
            symlink(&copy, &path).unwrap();
        } else {
            fs::hard_link(&path, &copy).unwrap();
        }
        assert!(
            OAuthStore::open(
                vec![RESOURCE.into(), OTHER.into()],
                &directory.0,
                "synthetic-project-mapping"
            )
            .is_err()
        );
    }
}

#[test]
fn failed_rotation_disables_in_memory_access_and_never_publishes_new_tokens() {
    let directory = Directory::new();
    let store = directory.open();
    let (client, token) = issue(&store, Instant::now());
    let original = fs::read(directory.0.join("state.bin")).unwrap();
    fs::create_dir(directory.0.join("state.next")).unwrap();
    assert!(matches!(
        store.refresh(request(&client, &token), Instant::now()),
        Err(OAuthError::Unavailable)
    ));
    assert!(
        store
            .resolve(&token.access_token, RESOURCE, Instant::now())
            .is_none()
    );
    assert!(matches!(
        store.refresh_access(&request(&client, &token), Instant::now()),
        Err(OAuthError::Unavailable)
    ));
    assert_eq!(fs::read(directory.0.join("state.bin")).unwrap(), original);
    drop(store);
    fs::remove_dir(directory.0.join("state.next")).unwrap();
    let store = directory.open();
    // Only the last committed, unrotated token remains after operator repair.
    store
        .refresh(request(&client, &token), Instant::now())
        .unwrap();
}

#[test]
fn orphan_encrypted_next_snapshot_does_not_replace_the_committed_state() {
    let directory = Directory::new();
    let store = directory.open();
    let (client, token) = issue(&store, Instant::now());
    drop(store);
    fs::copy(
        directory.0.join("state.bin"),
        directory.0.join("state.next"),
    )
    .unwrap();
    let store = directory.open();
    assert!(!directory.0.join("state.next").exists());
    store
        .refresh(request(&client, &token), Instant::now())
        .unwrap();
}
