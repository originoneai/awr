//! Bounded, process-local authorization-code adapter for Team MCP.
//!
//! The HTTP layer must validate the user's current project access before calling
//! `approve`, and recheck the returned underlying credential on every operation.
//! This module never creates identity, membership, grants or task delegation.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use url::Url;

pub const ACCESS_TOKEN_PREFIX: &str = "awr_oauth_";
pub const ACCESS_TTL: Duration = Duration::from_secs(3600);
pub const CONSENT_TTL: Duration = Duration::from_secs(600);
const CLIENT_TTL: Duration = Duration::from_secs(86400);
const CODE_TTL: Duration = Duration::from_secs(120);
const MAX_CLIENTS: usize = 1024;
const MAX_PENDING: usize = 1024;
const MAX_CODES: usize = 1024;
const MAX_TOKENS: usize = 4096;

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

#[derive(Clone, Serialize)]
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

#[derive(Serialize)]
pub struct AccessToken {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub scope: &'static str,
}

struct Client {
    value: RegisteredClient,
    expires: Instant,
}

struct Pending {
    view: ConsentView,
    cookie_hash: [u8; 32],
    expires: Instant,
}

struct Code {
    request: AuthorizationRequest,
    bearer: String,
    expires: Instant,
}

struct Token {
    resource: String,
    bearer: String,
    expires: Instant,
}

#[derive(Default)]
struct Entries {
    clients: BTreeMap<String, Client>,
    pending: BTreeMap<String, Pending>,
    codes: BTreeMap<[u8; 32], Code>,
    tokens: BTreeMap<[u8; 32], Token>,
}

impl Entries {
    fn prune(&mut self, now: Instant) {
        self.clients.retain(|_, e| e.expires > now);
        self.pending.retain(|_, e| e.expires > now);
        self.codes.retain(|_, e| e.expires > now);
        self.tokens.retain(|_, e| e.expires > now);
    }
}

pub struct OAuthStore {
    resources: BTreeSet<String>,
    entries: Mutex<Entries>,
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
        })
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
        let mut entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
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

    /// Caller must first validate current access for this exact pending resource.
    /// Credential contents are kept only in memory, never returned to the client.
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
        let mut entries = self.entries.lock().map_err(|_| OAuthError::Unavailable)?;
        entries.prune(now);
        if entries.tokens.len() >= MAX_TOKENS {
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
        let access_token = random(ACCESS_TOKEN_PREFIX)?;
        entries.tokens.insert(
            hash(&access_token),
            Token {
                resource: code.request.resource,
                bearer: code.bearer,
                expires: now + ACCESS_TTL,
            },
        );
        Ok(AccessToken {
            access_token,
            token_type: "Bearer",
            expires_in: ACCESS_TTL.as_secs(),
            scope: "awr.project",
        })
    }

    /// Resolves only the exact resource. The caller must recheck credential
    /// expiry, revocation, membership and grants in the existing policy store.
    pub fn resolve(&self, access_token: &str, resource: &str, now: Instant) -> Option<String> {
        if !access_token.starts_with(ACCESS_TOKEN_PREFIX) || access_token.len() > 128 {
            return None;
        }
        let entries = self.entries.lock().ok()?;
        let token = entries.tokens.get(&hash(access_token))?;
        (token.expires > now && token.resource == resource).then(|| token.bearer.clone())
    }
}
