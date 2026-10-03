//! Bounded authorization-code adapter for Team MCP, with optional encrypted storage.
//!
//! The HTTP layer must validate the user's current project access before calling
//! `approve`, and recheck the returned underlying credential on every operation.
//! This module never creates identity, membership, grants or task delegation.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use url::Url;

mod durable;

pub const ACCESS_TOKEN_PREFIX: &str = "awr_oauth_";
pub const ACCESS_TTL: Duration = Duration::from_secs(3600);
pub const CONNECTION_TTL: Duration = Duration::from_secs(86400);
const REFRESH_TOKEN_PREFIX: &str = "awr_refresh_";
pub const CONSENT_TTL: Duration = Duration::from_secs(600);
const CLIENT_TTL: Duration = Duration::from_secs(86400);
const CODE_TTL: Duration = Duration::from_secs(120);
const MAX_CLIENTS: usize = 1024;
const MAX_PENDING: usize = 1024;
const MAX_CODES: usize = 1024;
const MAX_TOKENS: usize = 4096;
const MAX_REFRESH_TOKENS: usize = 32768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthError {
    InvalidRequest,
    InvalidClient,
    InvalidTarget,
    InvalidGrant,
    AccessDenied,
    Unavailable,
}

impl OAuthError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidClient => "invalid_client",
            Self::InvalidTarget => "invalid_target",
            Self::InvalidGrant => "invalid_grant",
            Self::AccessDenied => "access_denied",
            Self::Unavailable => "temporarily_unavailable",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RegisteredClient {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
}

#[derive(Clone)]
pub struct AuthorizationRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub resource: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
}

// Deliberately not Debug/Serialize: browser binding and codes are secrets.
pub struct ConsentPage {
    pub transaction_id: String,
    pub cookie: String,
    pub client_name: String,
    pub request: AuthorizationRequest,
}

#[derive(Clone)]
pub struct ConsentView {
    pub client_name: String,
    pub request: AuthorizationRequest,
}

pub struct AuthorizationCode {
    pub code: String,
    pub redirect_uri: String,
    pub state: Option<String>,
}

pub struct TokenRequest {
    pub code: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub resource: String,
    pub code_verifier: String,
}

pub struct RefreshRequest {
    pub refresh_token: String,
    pub client_id: String,
    pub resource: Option<String>,
}

// Returned only to the HTTP adapter for its live authorization check.
// Never serialize or log the underlying credential.
pub struct RefreshAccess {
    pub resource: String,
    pub bearer: String,
}

#[derive(Serialize)]
pub struct AccessToken {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub scope: &'static str,
}

#[derive(Clone)]
struct Client {
    value: RegisteredClient,
    expires: Instant,
}

#[derive(Clone)]
struct Pending {
    view: ConsentView,
    cookie_hash: [u8; 32],
    expires: Instant,
}

#[derive(Clone)]
struct Code {
    request: AuthorizationRequest,
    bearer: String,
    expires: Instant,
    connection_expires: Instant,
}

#[derive(Clone)]
struct Token {
    family: [u8; 32],
    expires: Instant,
}

#[derive(Clone)]
struct Grant {
    client_id: String,
    resource: String,
    bearer: String,
    expires: Instant,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RefreshToken {
    family: [u8; 32],
    used: bool,
}

#[derive(Clone, Default)]
struct Entries {
    clients: BTreeMap<String, Client>,
    pending: BTreeMap<String, Pending>,
    codes: BTreeMap<[u8; 32], Code>,
    tokens: BTreeMap<[u8; 32], Token>,
    grants: BTreeMap<[u8; 32], Grant>,
    refresh_tokens: BTreeMap<[u8; 32], RefreshToken>,
}

impl Entries {
    fn prune(&mut self, now: Instant) {
        self.clients.retain(|_, e| e.expires > now);
        self.pending.retain(|_, e| e.expires > now);
        self.codes.retain(|_, e| e.expires > now);
        self.grants.retain(|_, e| e.expires > now);
        self.tokens
            .retain(|_, e| e.expires > now && self.grants.contains_key(&e.family));
        self.refresh_tokens
            .retain(|_, e| self.grants.contains_key(&e.family));
    }

    fn refresh_family(
        &mut self,
        request: &RefreshRequest,
        now: Instant,
    ) -> Result<[u8; 32], OAuthError> {
        if !request.refresh_token.starts_with(REFRESH_TOKEN_PREFIX)
            || !bounded(&request.refresh_token, 128)
            || !bounded(&request.client_id, 128)
        {
            return Err(OAuthError::InvalidGrant);
        }
        self.prune(now);
        let refresh = self
            .refresh_tokens
            .get(&hash(&request.refresh_token))
            .ok_or(OAuthError::InvalidGrant)?;
        let family = refresh.family;
        let grant = self.grants.get(&family).ok_or(OAuthError::InvalidGrant)?;
        if grant.client_id != request.client_id
            || request
                .resource
                .as_ref()
                .is_some_and(|r| r != &grant.resource)
            || !self.clients.contains_key(&grant.client_id)
        {
            return Err(OAuthError::InvalidGrant);
        }
        // Retain consumed keys until the original connection expires. Reuse by
        // the bound client revokes the whole family, including issued access.
        // A different client/resource must not be able to revoke it.
        if refresh.used {
            self.grants.remove(&family);
            self.prune(now);
            return Err(OAuthError::InvalidGrant);
        }
        Ok(family)
    }
}

pub struct OAuthStore {
    resources: BTreeSet<String>,
    entries: Mutex<Entries>,
    durable: Option<durable::Store>,
}

fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn equal_hash(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter()
        .zip(b)
        .fold(0u8, |different, (x, y)| different | (x ^ y))
        == 0
}

fn random(prefix: &str) -> Result<String, OAuthError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| OAuthError::Unavailable)?;
    Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

/// Exact, canonical HTTPS redirects or HTTP loopback callbacks only. No network
/// requests are made when registering a client or validating its callback.
pub fn valid_redirect(value: &str) -> bool {
    if !bounded(value, 2048) {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    url.as_str() == value
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && (url.scheme() == "https" || (url.scheme() == "http" && loopback))
        && !url.query_pairs().any(|(key, _)| {
            matches!(
                key.as_ref(),
                "code" | "state" | "error" | "error_description"
            )
        })
}

impl OAuthStore {
    pub fn new(resources: Vec<String>) -> Result<Self, OAuthError> {
        if resources.is_empty()
            || resources.len() > 1024
            || resources.iter().any(|resource| {
                !valid_redirect(resource)
                    || !resource.starts_with("https://")
                    || resource.contains('?')
            })
        {
            return Err(OAuthError::InvalidTarget);
        }
        Ok(Self {
            resources: resources.into_iter().collect(),
            entries: Mutex::new(Entries::default()),
            durable: None,
        })
    }

    /// Restore approved connections from a private, encrypted, single-writer
    /// directory. The binding must identify the deployment's project mapping.
    /// Pending consent and unredeemed codes deliberately remain process-local.
    pub fn open(
        resources: Vec<String>,
        directory: &Path,
        binding: &str,
    ) -> Result<Self, OAuthError> {
        let mut store = Self::new(resources)?;
        let (durable, entries) = durable::Store::open(directory, &store.resources, binding)?;
        store.entries = Mutex::new(entries);
        store.durable = Some(durable);
        Ok(store)
    }

    /// Commit durable changes before publishing tokens, including revocations
    /// returned as domain errors. An uncertain disk write disables this store
    /// until restart so stale in-memory grants cannot remain usable.
    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut Entries) -> Result<T, OAuthError>,
    ) -> Result<T, OAuthError> {
        let mut entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
        let Some(durable) = &self.durable else {
            return operation(&mut entries);
        };
        durable.check()?;
        let mut candidate = entries.clone();
        let result = operation(&mut candidate);
        durable.save(&candidate, &self.resources)?;
        *entries = candidate;
        result
    }

    pub fn register(
        &self,
        client_name: String,
        redirect_uris: Vec<String>,
        now: Instant,
    ) -> Result<RegisteredClient, OAuthError> {
        if !bounded(&client_name, 120)
            || redirect_uris.is_empty()
            || redirect_uris.len() > 8
            || redirect_uris.iter().any(|u| !valid_redirect(u))
        {
            return Err(OAuthError::InvalidRequest);
        }
        self.mutate(|entries| {
            entries.prune(now);
            if entries.clients.len() >= MAX_CLIENTS {
                return Err(OAuthError::Unavailable);
            }
            let value = RegisteredClient {
                client_id: random("client_")?,
                client_name,
                redirect_uris,
            };
            entries.clients.insert(
                value.client_id.clone(),
                Client {
                    value: value.clone(),
                    expires: now + CLIENT_TTL,
                },
            );
            Ok(value)
        })
    }

    pub fn begin(
        &self,
        request: AuthorizationRequest,
        now: Instant,
    ) -> Result<ConsentPage, OAuthError> {
        if !self.resources.contains(&request.resource) {
            return Err(OAuthError::InvalidTarget);
        }
        if request.code_challenge.len() != 43 {
            return Err(OAuthError::InvalidRequest);
        }
        let challenge = URL_SAFE_NO_PAD.decode(&request.code_challenge).ok();
        if request.code_challenge_method != "S256"
            || challenge.as_ref().is_none_or(|c| {
                c.len() != 32 || URL_SAFE_NO_PAD.encode(c) != request.code_challenge
            })
            || request.state.as_ref().is_some_and(|s| !bounded(s, 1024))
        {
            return Err(OAuthError::InvalidRequest);
        }
        let mut entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
        entries.prune(now);
        let client = entries
            .clients
            .get(&request.client_id)
            .ok_or(OAuthError::InvalidClient)?;
        if !client.value.redirect_uris.contains(&request.redirect_uri) {
            return Err(OAuthError::InvalidRequest);
        }
        if entries.pending.len() >= MAX_PENDING {
            return Err(OAuthError::Unavailable);
        }
        let client_name = client.value.client_name.clone();
        let transaction_id = random("consent_")?;
        let cookie = random("browser_")?;
        entries.pending.insert(
            transaction_id.clone(),
            Pending {
                view: ConsentView {
                    client_name: client_name.clone(),
                    request: request.clone(),
                },
                cookie_hash: hash(&cookie),
                expires: now + CONSENT_TTL,
            },
        );
        Ok(ConsentPage {
            transaction_id,
            cookie,
            client_name,
            request,
        })
    }

    pub fn consent(
        &self,
        transaction: &str,
        cookie: &str,
        now: Instant,
    ) -> Result<ConsentView, OAuthError> {
        let entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
        let pending = entries
            .pending
            .get(transaction)
            .ok_or(OAuthError::AccessDenied)?;
        if pending.expires <= now || !equal_hash(&pending.cookie_hash, &hash(cookie)) {
            return Err(OAuthError::AccessDenied);
        }
        Ok(pending.view.clone())
    }

    /// Cancel only the browser-bound transaction; the registered callback and
    /// state are returned so the HTTP adapter can send `access_denied` safely.
    pub fn cancel(
        &self,
        transaction: &str,
        cookie: &str,
        now: Instant,
    ) -> Result<ConsentView, OAuthError> {
        let mut entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
        entries.prune(now);
        let pending = entries
            .pending
            .get(transaction)
            .ok_or(OAuthError::AccessDenied)?;
        if !equal_hash(&pending.cookie_hash, &hash(cookie)) {
            return Err(OAuthError::AccessDenied);
        }
        Ok(entries
            .pending
            .remove(transaction)
            .expect("validated pending entry")
            .view)
    }

    /// Caller must first validate current access for this exact pending resource.
    /// Credentials are never returned to the client; durable grants are encrypted.
    pub fn approve(
        &self,
        transaction: &str,
        cookie: &str,
        validated_bearer: String,
        now: Instant,
    ) -> Result<AuthorizationCode, OAuthError> {
        if !bounded(&validated_bearer, 4096) {
            return Err(OAuthError::InvalidRequest);
        }
        let mut entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
        entries.prune(now);
        let pending = entries
            .pending
            .get(transaction)
            .ok_or(OAuthError::AccessDenied)?;
        if !equal_hash(&pending.cookie_hash, &hash(cookie)) {
            return Err(OAuthError::AccessDenied);
        }
        if entries.codes.len() >= MAX_CODES {
            return Err(OAuthError::Unavailable);
        }
        let code = random("code_")?;
        let pending = entries
            .pending
            .remove(transaction)
            .expect("validated pending entry");
        let result = AuthorizationCode {
            code: code.clone(),
            redirect_uri: pending.view.request.redirect_uri.clone(),
            state: pending.view.request.state.clone(),
        };
        entries.codes.insert(
            hash(&code),
            Code {
                request: pending.view.request,
                bearer: validated_bearer,
                expires: now + CODE_TTL,
                connection_expires: now + CONNECTION_TTL,
            },
        );
        Ok(result)
    }

    pub fn exchange(&self, request: TokenRequest, now: Instant) -> Result<AccessToken, OAuthError> {
        if !bounded(&request.code, 128)
            || !(43..=128).contains(&request.code_verifier.len())
            || !request
                .code_verifier
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-._~".contains(&c))
        {
            return Err(OAuthError::InvalidGrant);
        }
        self.mutate(|entries| {
            entries.prune(now);
            if entries.tokens.len() >= MAX_TOKENS
                || entries.grants.len() >= MAX_TOKENS
                || entries.refresh_tokens.len() >= MAX_REFRESH_TOKENS
            {
                return Err(OAuthError::Unavailable);
            }
            // Every well-formed redemption attempt consumes the code atomically,
            // including a binding/verifier mismatch; a rejected code cannot be retried.
            let code = entries
                .codes
                .remove(&hash(&request.code))
                .ok_or(OAuthError::InvalidGrant)?;
            let challenge = URL_SAFE_NO_PAD.encode(hash(&request.code_verifier));
            if request.client_id != code.request.client_id
                || request.redirect_uri != code.request.redirect_uri
                || request.resource != code.request.resource
                || !equal_hash(&hash(&challenge), &hash(&code.request.code_challenge))
            {
                return Err(OAuthError::InvalidGrant);
            }
            let client = entries
                .clients
                .get(&request.client_id)
                .ok_or(OAuthError::InvalidGrant)?;
            let connection_expires = code.connection_expires.min(client.expires);
            let access_expires = (now + ACCESS_TTL).min(connection_expires);
            if access_expires.saturating_duration_since(now).as_secs() == 0 {
                return Err(OAuthError::InvalidGrant);
            }
            let access_token = random(ACCESS_TOKEN_PREFIX)?;
            let refresh_token = random(REFRESH_TOKEN_PREFIX)?;
            let family = hash(&refresh_token);
            entries.grants.insert(
                family,
                Grant {
                    client_id: code.request.client_id,
                    resource: code.request.resource,
                    bearer: code.bearer,
                    expires: connection_expires,
                },
            );
            entries.refresh_tokens.insert(
                family,
                RefreshToken {
                    family,
                    used: false,
                },
            );
            entries.tokens.insert(
                hash(&access_token),
                Token {
                    family,
                    expires: access_expires,
                },
            );
            Ok(AccessToken {
                access_token,
                refresh_token,
                token_type: "Bearer",
                expires_in: access_expires.duration_since(now).as_secs(),
                scope: "awr.project",
            })
        })
    }

    /// Inspect the exact refresh binding before the adapter rechecks current
    /// project access. This does not consume a valid token or cache authority.
    pub fn refresh_access(
        &self,
        request: &RefreshRequest,
        now: Instant,
    ) -> Result<RefreshAccess, OAuthError> {
        self.mutate(|entries| {
            let family = entries.refresh_family(request, now)?;
            let grant = entries
                .grants
                .get(&family)
                .expect("validated refresh grant");
            Ok(RefreshAccess {
                resource: grant.resource.clone(),
                bearer: grant.bearer.clone(),
            })
        })
    }

    /// Caller must first validate the underlying credential's current access
    /// using refresh_access. Rotation never extends the connection deadline.
    pub fn refresh(
        &self,
        request: RefreshRequest,
        now: Instant,
    ) -> Result<AccessToken, OAuthError> {
        self.mutate(|entries| {
            let family = entries.refresh_family(&request, now)?;
            if entries.tokens.len() >= MAX_TOKENS
                || entries.refresh_tokens.len() >= MAX_REFRESH_TOKENS
            {
                return Err(OAuthError::Unavailable);
            }
            let grant = entries
                .grants
                .get(&family)
                .expect("validated refresh grant");
            let expires = (now + ACCESS_TTL).min(grant.expires);
            let expires_in = expires.saturating_duration_since(now).as_secs();
            if expires_in == 0 {
                return Err(OAuthError::InvalidGrant);
            }
            let access_token = random(ACCESS_TOKEN_PREFIX)?;
            let refresh_token = random(REFRESH_TOKEN_PREFIX)?;
            entries
                .refresh_tokens
                .get_mut(&hash(&request.refresh_token))
                .expect("validated refresh token")
                .used = true;
            entries.refresh_tokens.insert(
                hash(&refresh_token),
                RefreshToken {
                    family,
                    used: false,
                },
            );
            entries
                .tokens
                .insert(hash(&access_token), Token { family, expires });
            Ok(AccessToken {
                access_token,
                refresh_token,
                token_type: "Bearer",
                expires_in,
                scope: "awr.project",
            })
        })
    }

    /// Resolves only the exact resource. The caller must recheck credential
    /// expiry, revocation, membership and grants in the existing policy store.
    pub fn resolve(&self, access_token: &str, resource: &str, now: Instant) -> Option<String> {
        if self
            .durable
            .as_ref()
            .is_some_and(|store| store.check().is_err())
        {
            return None;
        }
        if !access_token.starts_with(ACCESS_TOKEN_PREFIX) || access_token.len() > 128 {
            return None;
        }
        let entries = self.entries.lock().ok()?;
        let token = entries.tokens.get(&hash(access_token))?;
        let grant = entries.grants.get(&token.family)?;
        (token.expires > now && grant.expires > now && grant.resource == resource)
            .then(|| grant.bearer.clone())
    }
}
