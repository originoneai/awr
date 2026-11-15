use super::{OAuthError, OAuthStore, hash};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// Public connection references never authorize requests. They are deliberately
/// distinct from access tokens, refresh tokens and the private family key.
fn reference(family: &[u8; 32]) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"awr-oauth-connection-reference-v1:");
    digest.update(family);
    format!("agent_{:x}", digest.finalize())
}

pub(super) fn now_ms() -> Result<u64, OAuthError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .ok_or(OAuthError::Unavailable)
}

#[derive(Serialize)]
pub struct OAuthConnection {
    pub connection_id: String,
    pub client_name: String,
    pub created_at_ms: Option<u64>,
    pub expires_in_seconds: u64,
    pub revoked_at_ms: Option<u64>,
}

// Only the encrypted snapshot may serialize browser session material.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowserSession {
    pub id: String,
    pub connection_id: String,
    pub bearer: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub last_seen_ms: u64,
    pub revoked: bool,
}

impl BrowserSession {
    pub(crate) fn valid(&self, key: &str) -> bool {
        key == self.id
            && key.starts_with("ws_")
            && key.len() == 67
            && key[3..].bytes().all(|b| b.is_ascii_hexdigit())
            && self.connection_id == Self::reference(key)
            && !self.bearer.is_empty()
            && self.bearer.len() <= 4096
            && self.expires_at_ms > self.created_at_ms
            && self.last_seen_ms >= self.created_at_ms
    }

    pub(crate) fn reference(id: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(b"awr-browser-connection-reference-v1:");
        digest.update(hash(id));
        format!("browser_{:x}", digest.finalize())
    }
}

impl OAuthStore {
    /// The adapter supplies a live, project-authorized credential predicate.
    /// Raw bearers and private family keys never leave this method.
    pub fn connections(
        &self,
        resource: &str,
        allows: impl Fn(&str) -> bool,
        now: Instant,
    ) -> Result<Vec<OAuthConnection>, OAuthError> {
        let entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
        if let Some(store) = &self.durable {
            store.check()?;
        }
        Ok(entries
            .grants
            .iter()
            .filter(|(_, grant)| {
                grant.resource == resource && grant.expires > now && allows(&grant.bearer)
            })
            .map(|(key, grant)| OAuthConnection {
                connection_id: reference(key),
                client_name: entries
                    .clients
                    .get(&grant.client_id)
                    .map(|client| client.value.client_name.clone())
                    .unwrap_or_default(),
                created_at_ms: grant.created_at_ms,
                expires_in_seconds: grant.expires.saturating_duration_since(now).as_secs(),
                revoked_at_ms: grant.revoked_at_ms,
            })
            .collect())
    }

    /// A repeated revocation returns the original cutoff. Unknown, expired and
    /// unauthorized references share one denial and never imply success.
    pub fn revoke_connection(
        &self,
        resource: &str,
        connection_id: &str,
        allows: impl Fn(&str) -> bool,
        now: Instant,
    ) -> Result<u64, OAuthError> {
        self.mutate(|entries| {
            let grant = entries
                .grants
                .iter_mut()
                .find_map(|(key, grant)| {
                    (grant.resource == resource
                        && grant.expires > now
                        && reference(key) == connection_id
                        && allows(&grant.bearer))
                    .then_some(grant)
                })
                .ok_or(OAuthError::AccessDenied)?;
            let cutoff = match grant.revoked_at_ms {
                Some(cutoff) => cutoff,
                None => now_ms()?,
            };
            grant.revoked_at_ms = Some(cutoff);
            entries.prune(now);
            Ok(cutoff)
        })
    }

    /// Reads may update last-seen metadata in memory. Admission and revocation
    /// persist the complete candidate before exposing their effect.
    pub(crate) fn browser_sessions<T>(
        &self,
        persist: bool,
        operation: impl FnOnce(&mut BTreeMap<String, BrowserSession>) -> Result<T, OAuthError>,
    ) -> Result<T, OAuthError> {
        if persist {
            return self.mutate(|entries| operation(&mut entries.browser_sessions));
        }
        let mut entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
        if let Some(store) = &self.durable {
            store.check()?;
        }
        operation(&mut entries.browser_sessions)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn browser_admission_and_revocation_are_encrypted_and_restart_durable() {
        const ISOLATED: &str = "AWR_BROWSER_RESTART_TEST_CHILD";
        const VERIFIED: &str = "browser_restart_fixture_verified";
        // Parallel process-spawning tests can inherit an open writer-lock
        // descriptor during fork. Keep this fixture's lock lifetime in its own process.
        if std::env::var_os(ISOLATED).as_deref() != Some(std::ffi::OsStr::new("1")) {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "oauth::connections::tests::browser_admission_and_revocation_are_encrypted_and_restart_durable",
                    "--nocapture",
                ])
                .env(ISOLATED, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains(VERIFIED),
                "isolated browser restart fixture failed\nstdout={}\nstderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            return;
        }
        let directory =
            std::env::temp_dir().join(format!("awr-browser-state-{}", awr_core::Id::new()));
        let open = || {
            OAuthStore::open(
                vec!["https://awr.example/v1/projects/p/mcp".into()],
                &directory,
                "fixture",
            )
            .unwrap()
        };
        let id = format!("ws_{}", "a".repeat(64));
        let public = BrowserSession::reference(&id);
        let bearer = "synthetic-browser-test-value";
        let store = open();
        store
            .browser_sessions(true, |sessions| {
                sessions.insert(
                    id.clone(),
                    BrowserSession {
                        id: id.clone(),
                        connection_id: public.clone(),
                        bearer: bearer.into(),
                        created_at_ms: 1000,
                        expires_at_ms: u64::MAX / 2,
                        last_seen_ms: 1000,
                        revoked: false,
                    },
                );
                Ok(())
            })
            .unwrap();
        let bytes = std::fs::read(directory.join("state.bin")).unwrap();
        for secret in [&id, bearer] {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes())
            );
        }
        drop(store);
        let store = open();
        store
            .browser_sessions(true, |sessions| {
                let session = sessions.get_mut(&id).unwrap();
                assert_eq!(session.connection_id, public);
                assert!(!session.revoked);
                session.revoked = true;
                Ok(())
            })
            .unwrap();
        drop(store);
        let store = open();
        assert!(
            store
                .browser_sessions(false, |sessions| Ok(sessions.get(&id).unwrap().revoked))
                .unwrap()
        );
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
        println!("{VERIFIED}");
    }
}
