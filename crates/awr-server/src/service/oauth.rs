//! Opt-in OAuth discovery and browser consent. Authority still comes from the
//! original credential and live PostgreSQL checks, never client metadata.
use super::*;
use crate::oauth::{AuthorizationRequest, ConsentView, OAuthError, OAuthStore, TokenRequest};
use axum::{
    body::{Body, to_bytes},
    extract::{RawQuery, Request},
    http::{HeaderValue, header},
    middleware::{self, Next},
    routing::get,
};
use std::time::Instant;
use url::Url;

const MAX_REQUEST_BYTES: usize = 16_384;
const COOKIE: &str = "__Host-awr_mcp_consent";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthConfig {
    /// Canonical HTTPS origin, without a trailing slash. Never inferred from
    /// request Host/Forwarded headers or from registered client metadata.
    pub issuer: String,
}

impl OAuthConfig {
    pub(super) fn validate(&self, allowed_hosts: &[String]) -> Result<(), String> {
        let url = Url::parse(&self.issuer).map_err(|_| "invalid OAuth issuer".to_string())?;
        if self.issuer.len() > 255
            || url.scheme() != "https"
            || url.origin().ascii_serialization() != self.issuer
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || !allowed_hosts
                .iter()
                .any(|host| host == self.issuer.strip_prefix("https://").unwrap_or_default())
        {
            return Err(
                "OAuth requires a canonical HTTPS origin and its exact allowed_host".into(),
            );
        }
        Ok(())
    }
}

pub(super) struct OAuthRuntime {
    issuer: String,
    pub(super) store: OAuthStore,
}

impl OAuthRuntime {
    pub(super) fn new(config: &OAuthConfig, projects: &[ProjectBinding]) -> Result<Self, String> {
        let resources = projects
            .iter()
            .map(|p| format!("{}/v1/projects/{}/mcp", config.issuer, p.key))
            .collect();
        let store =
            OAuthStore::new(resources).map_err(|_| "invalid OAuth resources".to_string())?;
        Ok(Self {
            issuer: config.issuer.clone(),
            store,
        })
    }
    pub(super) fn resource(&self, key: &str) -> String {
        format!("{}/v1/projects/{key}/mcp", self.issuer)
    }
}

pub(super) fn challenge(state: &StateData, key: &str, invalid: bool) -> Response {
    let Some(oauth) = &state.oauth else {
        return denied();
    };
    let mut res = response(
        StatusCode::UNAUTHORIZED,
        json!({"code":"Unauthorized","message":"MCP authorization required"}),
    );
    let error = if invalid {
        ", error=\"invalid_token\""
    } else {
        ""
    };
    let value = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/v1/projects/{key}/mcp\"{error}",
        oauth.issuer
    );
    res.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        value.parse().expect("validated metadata URL"),
    );
    res
}

pub(super) fn router(state: Arc<StateData>) -> Router {
    Router::new()
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route(
            "/.well-known/oauth-protected-resource/v1/projects/{project}/mcp",
            get(resource_metadata),
        )
        .route("/oauth/register", post(register))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/consent", post(consent))
        .route("/oauth/token", post(token))
        .layer(middleware::from_fn_with_state(state.clone(), admission))
        .with_state(state)
}

fn security_headers(mut res: Response) -> Response {
    for (key, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        (
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    ] {
        if !res.headers().contains_key(key) {
            res.headers_mut()
                .insert(key, HeaderValue::from_static(value));
        }
    }
    res
}

async fn admission(State(state): State<Arc<StateData>>, request: Request, next: Next) -> Response {
    let oauth = state.oauth.as_ref().expect("enabled OAuth routes");
    let headers = request.headers();
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok());
    if headers.get_all(header::HOST).iter().count() != 1
        || host != oauth.issuer.strip_prefix("https://")
        || headers.get_all(header::ORIGIN).iter().count() > 1
        || (headers.contains_key(header::ORIGIN) && origin != Some(oauth.issuer.as_str()))
    {
        return security_headers(oauth_error(OAuthError::AccessDenied));
    }
    if request
        .uri()
        .query()
        .is_some_and(|q| q.len() > MAX_REQUEST_BYTES)
    {
        return security_headers(oauth_error(OAuthError::InvalidRequest));
    }
    let Ok(_permit) = state.permits.clone().try_acquire_owned() else {
        return security_headers(oauth_error(OAuthError::Unavailable));
    };
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let (parts, body) = request.into_parts();
        let Ok(body) = to_bytes(body, MAX_REQUEST_BYTES).await else {
            return response(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"error":"invalid_request"}),
            );
        };
        next.run(Request::from_parts(parts, Body::from(body))).await
    })
    .await
    .unwrap_or_else(|_| oauth_error(OAuthError::Unavailable));
    security_headers(result)
}

fn oauth_error(error: OAuthError) -> Response {
    let status = match error {
        OAuthError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        OAuthError::AccessDenied => StatusCode::FORBIDDEN,
        _ => StatusCode::BAD_REQUEST,
    };
    response(status, json!({"error":error.code()}))
}

async fn metadata(State(state): State<Arc<StateData>>) -> Response {
    let oauth = state.oauth.as_ref().expect("enabled OAuth routes");
    let issuer = &oauth.issuer;
    response(
        StatusCode::OK,
        json!({
            "issuer":issuer,
            "authorization_endpoint":format!("{issuer}/oauth/authorize"),
            "token_endpoint":format!("{issuer}/oauth/token"),
            "registration_endpoint":format!("{issuer}/oauth/register"),
            "response_types_supported":["code"],
            "grant_types_supported":["authorization_code"],
            "token_endpoint_auth_methods_supported":["none"],
            "code_challenge_methods_supported":["S256"],
            "scopes_supported":["awr.project"]
        }),
    )
}

async fn resource_metadata(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
) -> Response {
    if !state.projects.contains_key(&key) {
        return response(StatusCode::NOT_FOUND, json!({"error":"invalid_target"}));
    }
    let oauth = state.oauth.as_ref().expect("enabled OAuth routes");
    response(
        StatusCode::OK,
        json!({
            "resource":oauth.resource(&key), "authorization_servers":[oauth.issuer],
            "bearer_methods_supported":["header"], "scopes_supported":["awr.project"],
            "resource_name":format!("AWR project: {key}")
        }),
    )
}

// RFC 7591 clients may send extra descriptive metadata. It is ignored, never
// fetched, and cannot grant identity or expand the advertised flow.
#[derive(Deserialize)]
struct Registration {
    #[serde(default = "default_client_name")]
    client_name: String,
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    scope: Option<String>,
}
fn default_client_name() -> String {
    "MCP client".into()
}
fn valid_scope(scope: Option<&str>) -> bool {
    scope.is_none_or(|s| s.len() <= 128 && s.split_ascii_whitespace().eq(["awr.project"]))
}
fn content_type(headers: &HeaderMap, expected: &str) -> bool {
    headers.get_all(header::CONTENT_TYPE).iter().count() == 1
        && headers
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| {
                h.split(';')
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case(expected))
            })
}
async fn register(
    State(state): State<Arc<StateData>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !content_type(&headers, "application/json") {
        return oauth_error(OAuthError::InvalidRequest);
    }
    let Ok(input) = serde_json::from_slice::<Registration>(&body) else {
        return oauth_error(OAuthError::InvalidRequest);
    };
    if input
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|m| m != "none")
        || input
            .grant_types
            .as_ref()
            .is_some_and(|g| !g.iter().any(|v| v == "authorization_code"))
        || input
            .response_types
            .as_ref()
            .is_some_and(|g| !g.iter().any(|v| v == "code"))
        || !valid_scope(input.scope.as_deref())
    {
        return response(
            StatusCode::BAD_REQUEST,
            json!({"error":"invalid_client_metadata"}),
        );
    }
    let oauth = state.oauth.as_ref().expect("enabled OAuth routes");
    match oauth
        .store
        .register(input.client_name, input.redirect_uris, Instant::now())
    {
        Ok(client) => response(
            StatusCode::CREATED,
            json!({
                "client_id":client.client_id,"client_name":client.client_name,
                "redirect_uris":client.redirect_uris,"token_endpoint_auth_method":"none",
                "grant_types":["authorization_code"],"response_types":["code"],"scope":"awr.project"
            }),
        ),
        Err(e) => oauth_error(e),
    }
}

fn parameters(raw: &str) -> Result<BTreeMap<String, String>, OAuthError> {
    if raw.len() > MAX_REQUEST_BYTES {
        return Err(OAuthError::InvalidRequest);
    }
    let mut params = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
        if key.is_empty()
            || key.len() > 128
            || value.len() > 4096
            || key.chars().any(char::is_control)
            || value.chars().any(char::is_control)
            || params
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return Err(OAuthError::InvalidRequest);
        }
    }
    Ok(params)
}
fn required(params: &BTreeMap<String, String>, key: &str) -> Result<String, OAuthError> {
    params
        .get(key)
        .filter(|s| !s.is_empty())
        .cloned()
        .ok_or(OAuthError::InvalidRequest)
}
fn parse_authorization(query: &str) -> Result<AuthorizationRequest, OAuthError> {
    let params = parameters(query)?;
    if required(&params, "response_type")? != "code"
        || !valid_scope(params.get("scope").map(String::as_str))
    {
        return Err(OAuthError::InvalidRequest);
    }
    Ok(AuthorizationRequest {
        client_id: required(&params, "client_id")?,
        redirect_uri: required(&params, "redirect_uri")?,
        resource: required(&params, "resource")?,
        state: params.get("state").cloned(),
        code_challenge: required(&params, "code_challenge")?,
        code_challenge_method: required(&params, "code_challenge_method")?,
    })
}
async fn authorize(State(state): State<Arc<StateData>>, RawQuery(query): RawQuery) -> Response {
    let request = match parse_authorization(query.as_deref().unwrap_or_default()) {
        Ok(request) => request,
        Err(e) => return oauth_error(e),
    };
    let oauth = state.oauth.as_ref().expect("enabled OAuth routes");
    match oauth.store.begin(request, Instant::now()) {
        Ok(page) => {
            let view = ConsentView {
                client_name: page.client_name,
                request: page.request,
            };
            let mut res = consent_page(&page.transaction_id, &view, None, StatusCode::OK);
            set_cookie(&mut res, &page.cookie, crate::oauth::CONSENT_TTL.as_secs());
            res
        }
        Err(e) => oauth_error(e),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn consent_page(
    transaction: &str,
    view: &ConsentView,
    error: Option<&str>,
    status: StatusCode,
) -> Response {
    let client = escape(&view.client_name);
    let callback = escape(&view.request.redirect_uri);
    let resource = escape(&view.request.resource);
    let transaction = escape(transaction);
    let error = error
        .map(|e| format!("<p class=error role=alert>{}</p>", escape(e)))
        .unwrap_or_default();
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Connect your agent · AWR</title><style>
*{{box-sizing:border-box}}body{{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:#f1f6ff;color:#142c50;font:16px/1.5 system-ui,sans-serif}}main{{width:min(100%,560px);background:white;border:1px solid #dbe7fa;border-radius:20px;padding:36px;box-shadow:0 20px 60px #14376d0d}}.brand{{color:#2563eb;font-size:24px;font-weight:800;letter-spacing:2px}}h1{{font-size:28px;line-height:1.2;margin:24px 0 12px}}.muted,small{{color:#526580}}.details{{background:#f7faff;border:1px solid #e0e9f8;border-radius:12px;padding:16px;margin:20px 0;font-size:14px;overflow-wrap:anywhere}}dt{{font-weight:650;margin-top:10px}}dt:first-child{{margin-top:0}}dd{{margin:4px 0 0}}label{{display:block;font-weight:650;margin:20px 0 8px}}input{{width:100%;border:1px solid #b9cbea;border-radius:8px;padding:12px;font:inherit}}input:focus{{outline:3px solid #d4e2ff;border-color:#2563eb}}.actions{{display:flex;gap:12px;margin:24px 0 12px}}button{{border:0;border-radius:8px;padding:12px 18px;font:inherit;font-weight:650;cursor:pointer}}.allow{{background:#2563eb;color:white;flex:1}}.cancel{{background:#eaf0fa;color:#203b64}}.error{{background:#fff1f1;color:#993131;padding:12px;border-radius:8px}}small{{display:block;font-size:13px}}
</style></head><body><main><div class="brand">AWR</div><h1>Connect your agent</h1><p class="muted">Allow <strong>{client}</strong> to work with this project using your current permissions.</p><dl class="details"><dt>Project access</dt><dd>{resource}</dd><dt>Return address</dt><dd>{callback}</dd><dt>Client identity</dt><dd>This client name and address are self-registered and have not been verified by AWR. Continue only if you started this connection.</dd></dl><p>Your agent can read project context and perform only actions you already have permission to perform. Authorizing this connection does not change your membership or permissions.</p>{error}<form method="post" action="/oauth/consent"><input type="hidden" name="transaction" value="{transaction}"><label for="credential">Your personal access credential</label><input id="credential" name="credential" type="password" autocomplete="off" spellcheck="false" required maxlength="512"><small>Get this credential from your project administrator. It stays with AWR and is never sent to your agent.</small><div class="actions"><button class="allow" name="action" value="allow" type="submit">Allow access</button><button class="cancel" name="action" value="cancel" type="submit" formnovalidate>Cancel</button></div></form><small>This connection lasts up to one hour. Revoking your credential or project permissions stops its access. After a service restart, connect again.</small></main></body></html>"#
    );
    let mut res = (status, [("content-type", "text/html; charset=utf-8")], html).into_response();
    // A no-referrer form navigation can send Origin: null in browsers. Keep
    // the issuer origin for the same-origin POST while withholding referrers
    // from other origins. Completion responses retain no-referrer.
    res.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("same-origin"));
    res
}

fn set_cookie(res: &mut Response, value: &str, max_age: u64) {
    let cookie =
        format!("{COOKIE}={value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={max_age}");
    res.headers_mut().append(
        header::SET_COOKIE,
        cookie.parse().expect("generated cookie"),
    );
}
fn browser_cookie(headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        for part in value.to_str().ok()?.split(';') {
            let Some((key, value)) = part.trim().split_once('=') else {
                continue;
            };
            if key == COOKIE {
                if found.is_some()
                    || value.len() > 128
                    || !value.starts_with("browser_")
                    || !value
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                {
                    return None;
                }
                found = Some(value.to_owned());
            }
        }
    }
    found
}
fn return_to_client(callback: &str, parameter: &str, value: &str, state: Option<&str>) -> Response {
    let mut url = Url::parse(callback).expect("registered callback");
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair(parameter, value);
        if let Some(state) = state {
            pairs.append_pair("state", state);
        }
    }
    // End the credential form submission before navigating to the registered
    // callback. A 303 keeps its entire redirect chain subject to form-action,
    // including any later origin changes controlled by the client. A completion
    // document can navigate without granting another origin form access.
    let target = escape(url.as_str());
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="refresh" content="0;url={target}"><title>Returning to your agent · AWR</title><style>
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#f1f6ff;color:#142c50;font:16px/1.5 system-ui,sans-serif}}main{{margin:24px;padding:32px;background:white;border:1px solid #dbe7fa;border-radius:16px}}h1{{font-size:24px}}a{{color:#2563eb}}
</style></head><body><main><h1>Returning to your agent</h1><p>Your response is ready. This page will return you to your agent automatically.</p><p>If this page stays open, <a href="{target}" rel="noreferrer">continue to your agent</a>.</p></main></body></html>"#
    );
    let mut res = (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        html,
    )
        .into_response();
    set_cookie(&mut res, "", 0);
    res
}
async fn consent(State(state): State<Arc<StateData>>, headers: HeaderMap, body: Bytes) -> Response {
    let oauth = state.oauth.as_ref().expect("enabled OAuth routes");
    if headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) != Some(oauth.issuer.as_str())
        || headers.get_all("sec-fetch-site").iter().count() > 1
        || headers
            .get("sec-fetch-site")
            .is_some_and(|h| h != "same-origin")
        || !content_type(&headers, "application/x-www-form-urlencoded")
    {
        return oauth_error(OAuthError::AccessDenied);
    }
    let Some(cookie) = browser_cookie(&headers) else {
        return oauth_error(OAuthError::AccessDenied);
    };
    let params = match std::str::from_utf8(&body)
        .ok()
        .and_then(|s| parameters(s).ok())
    {
        Some(params) => params,
        None => return oauth_error(OAuthError::InvalidRequest),
    };
    let transaction = match required(&params, "transaction") {
        Ok(t) => t,
        Err(e) => return oauth_error(e),
    };
    let view = match oauth.store.consent(&transaction, &cookie, Instant::now()) {
        Ok(view) => view,
        Err(e) => return oauth_error(e),
    };
    if params.get("action").is_some_and(|v| v == "cancel") {
        return match oauth.store.cancel(&transaction, &cookie, Instant::now()) {
            Ok(view) => return_to_client(
                &view.request.redirect_uri,
                "error",
                "access_denied",
                view.request.state.as_deref(),
            ),
            Err(e) => oauth_error(e),
        };
    }
    if params.get("action").is_none_or(|v| v != "allow") {
        return oauth_error(OAuthError::InvalidRequest);
    }
    let credential = params
        .get("credential")
        .map(|c| c.trim())
        .unwrap_or_default();
    if credential.is_empty()
        || credential.len() > 512
        || credential.chars().any(char::is_whitespace)
    {
        return oauth_error(OAuthError::AccessDenied);
    }
    let project = state
        .projects
        .values()
        .find(|p| oauth.resource(&p.key) == view.request.resource)
        .expect("registered project resource");
    let capabilities: WorkstreamQuery =
        serde_json::from_value(json!({"protocol_version":1,"op":"capabilities"}))
            .expect("static query");
    if let Err(error) = state
        .store
        .query(
            &project.tenant_id,
            &project.project_id,
            credential,
            capabilities,
        )
        .await
    {
        return if matches!(error, PgError::Forbidden) {
            consent_page(
                &transaction,
                &view,
                Some(
                    "Your credential was not accepted for this project. Check the credential with your administrator and try again.",
                ),
                StatusCode::FORBIDDEN,
            )
        } else {
            oauth_error(OAuthError::Unavailable)
        };
    }
    match oauth
        .store
        .approve(&transaction, &cookie, credential.to_owned(), Instant::now())
    {
        Ok(code) => return_to_client(
            &code.redirect_uri,
            "code",
            &code.code,
            code.state.as_deref(),
        ),
        Err(e) => oauth_error(e),
    }
}
async fn token(State(state): State<Arc<StateData>>, headers: HeaderMap, body: Bytes) -> Response {
    if !content_type(&headers, "application/x-www-form-urlencoded")
        || headers.contains_key(header::AUTHORIZATION)
    {
        return oauth_error(OAuthError::InvalidRequest);
    }
    let request = (|| {
        let raw = std::str::from_utf8(&body).map_err(|_| OAuthError::InvalidRequest)?;
        let params = parameters(raw)?;
        if required(&params, "grant_type")? != "authorization_code"
            || params.contains_key("client_secret")
        {
            return Err(OAuthError::InvalidRequest);
        }
        Ok(TokenRequest {
            code: required(&params, "code")?,
            client_id: required(&params, "client_id")?,
            redirect_uri: required(&params, "redirect_uri")?,
            resource: required(&params, "resource")?,
            code_verifier: required(&params, "code_verifier")?,
        })
    })();
    let request = match request {
        Ok(r) => r,
        Err(e) => return oauth_error(e),
    };
    let oauth = state.oauth.as_ref().expect("enabled OAuth routes");
    match oauth.store.exchange(request, Instant::now()) {
        Ok(token) => response(
            StatusCode::OK,
            serde_json::to_value(token).expect("access token"),
        ),
        Err(e) => oauth_error(e),
    }
}
