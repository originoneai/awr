use super::QueryRequest;
use crate::cli_failure::{CliFailure, CliResult};
use awr_core::Error;
use awr_team::RemoteProfile;
use serde_json::{Value, json};
use std::time::Duration;
use ureq::http::{HeaderValue, Request};
use url::Url;

const TIMEOUT: Duration = Duration::from_secs(35);
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;

pub(super) struct TeamClient {
    url: Url,
    token: String,
    authorization: HeaderValue,
    agent: ureq::Agent,
}

impl TeamClient {
    pub(super) fn new(profile: &RemoteProfile) -> CliResult<Self> {
        let url = query_url(profile)?;
        let invalid_credential = || {
            Error::InvalidInput(format!(
                "set {} to a nonempty valid Bearer credential",
                profile.credential_env
            ))
        };
        let token = std::env::var(&profile.credential_env).map_err(|_| invalid_credential())?;
        // RFC 6750 b64token. Never trim a credential into a different credential.
        let data = token.trim_end_matches('=');
        if data.is_empty()
            || !data
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/".contains(&b))
        {
            return Err(invalid_credential().into());
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| invalid_credential())?;
        authorization.set_sensitive(true);
        Ok(Self {
            url,
            token,
            authorization,
            agent: agent(TIMEOUT),
        })
    }

    pub(super) fn query(&self, query: &QueryRequest) -> CliResult<Value> {
        let body = serde_json::to_vec(query).expect("query request serializes");
        let request = Request::post(self.url.as_str())
            .header("Authorization", self.authorization.clone())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body.as_slice())
            .map_err(|_| Error::InvalidInput("invalid Team query address or headers".into()))?;
        let response = self.agent.run(request).map_err(transport_error)?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(CliFailure::client(
                "TeamRedirect",
                "Team redirects are not followed; check the remote service base address",
                json!({"http_status":status}),
            ));
        }
        if response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > MAX_RESPONSE_BYTES)
        {
            return Err(response_too_large());
        }
        let bytes = response
            .into_body()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(transport_error)?;
        let parsed = serde_json::from_slice::<Value>(&bytes);
        if status == 200 {
            return parsed.map_err(|_| {
                CliFailure::invalid_team_response("expected a JSON query result", status)
            });
        }
        if (200..300).contains(&status) {
            return Err(CliFailure::invalid_team_response(
                "expected HTTP 200 with a query result",
                status,
            ));
        }
        let mut report = parsed.map_err(|_| CliFailure::client("TeamHttpError",
            "Team service returned a non-JSON HTTP error; check the remote base address and service",
            json!({"http_status":status})))?;
        let valid = report.as_object().is_some_and(|object| {
            object
                .get("code")
                .and_then(Value::as_str)
                .is_some_and(|code| {
                    !code.is_empty() && code.len() <= 128 && !code.chars().any(char::is_control)
                })
                && object.get("message").is_none_or(Value::is_string)
        });
        if !valid {
            return Err(CliFailure::invalid_team_response(
                "invalid Team error object",
                status,
            ));
        }
        redact(&mut report, &self.token);
        Err(CliFailure::remote(report, status))
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(timeout))
        .user_agent(concat!("awr/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn query_url(profile: &RemoteProfile) -> awr_core::Result<Url> {
    profile.validate().map_err(crate::remote::team_err)?;
    if matches!(profile.project_key.as_str(), "." | "..") {
        return Err(Error::InvalidInput(
            "project key cannot be a URL dot segment".into(),
        ));
    }
    let mut url = Url::parse(&profile.endpoint)
        .map_err(|_| Error::InvalidInput("invalid Team service base address".into()))?;
    let prefix = url.path().trim_end_matches('/').to_owned();
    url.set_path(&prefix);
    url.path_segments_mut()
        .map_err(|_| Error::InvalidInput("invalid Team service base path".into()))?
        .pop_if_empty()
        .extend(["v1", "projects", &profile.project_key, "query"]);
    Ok(url)
}

fn response_too_large() -> CliFailure {
    CliFailure::client(
        "TeamResponseTooLarge",
        "Team response exceeds the client transport limit",
        json!({"limit_bytes":MAX_RESPONSE_BYTES}),
    )
}

fn transport_error(error: ureq::Error) -> CliFailure {
    match error {
        ureq::Error::Timeout(_) => CliFailure::client(
            "TeamTimeout",
            "Team query timed out",
            json!({"timeout_seconds":TIMEOUT.as_secs()}),
        ),
        ureq::Error::BodyExceedsLimit(_) => response_too_large(),
        error => {
            let reason = match error {
                ureq::Error::HostNotFound => "dns",
                ureq::Error::Tls(_) | ureq::Error::Rustls(_) => "tls",
                ureq::Error::Io(_) => "io",
                ureq::Error::Protocol(_) => "http_protocol",
                _ => "connection",
            };
            CliFailure::client(
                "TeamTransport",
                "could not complete the Team query; check the service and remote configuration",
                json!({"reason":reason}),
            )
        }
    }
}

fn redact(value: &mut Value, token: &str) {
    match value {
        Value::String(text) => *text = text.replace(token, "[redacted]"),
        Value::Array(items) => items.iter_mut().for_each(|v| redact(v, token)),
        Value::Object(object) => {
            // Also guard unexpected diagnostic keys that reflect a credential.
            *object = std::mem::take(object)
                .into_iter()
                .map(|(key, mut value)| {
                    redact(&mut value, token);
                    (key.replace(token, "[redacted]"), value)
                })
                .collect();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_addresses_preserve_prefixes_and_encode_one_alias_segment() {
        let mut profile = RemoteProfile {
            name: "test".into(),
            endpoint: String::new(),
            project_key: "demo".into(),
            credential_env: "AWR_TEST_TOKEN".into(),
            protocol_version: 1,
        };
        for (base, path) in [
            ("https://example.invalid", "/v1/projects/demo/query"),
            (
                "https://example.invalid/awr///",
                "/awr/v1/projects/demo/query",
            ),
            (
                "https://example.invalid/proxy//a%20b/",
                "/proxy//a%20b/v1/projects/demo/query",
            ),
            ("https://example.invalid/mcp", "/mcp/v1/projects/demo/query"),
        ] {
            profile.endpoint = base.into();
            assert_eq!(query_url(&profile).unwrap().path(), path);
        }
        profile.endpoint = "https://example.invalid/awr/".into();
        profile.project_key = "a/b c%2f中文".into();
        assert_eq!(
            query_url(&profile).unwrap().path(),
            "/awr/v1/projects/a%2Fb%20c%252f%E4%B8%AD%E6%96%87/query"
        );
        for alias in [".", ".."] {
            profile.project_key = alias.into();
            assert!(query_url(&profile).is_err());
        }
        assert_eq!(agent(TIMEOUT).config().max_redirects(), 0);
        assert_eq!(TIMEOUT, Duration::from_secs(35));
    }

    #[test]
    fn the_global_deadline_applies_while_reading_a_response_body() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, held_body) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            assert!(socket.read(&mut [0; 4096]).unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{")
                .unwrap();
            // Hold the incomplete body until the client finishes, with a bounded failure path.
            let _ = held_body.recv_timeout(Duration::from_secs(5));
        });
        let mut response = agent(Duration::from_secs(1))
            .get(format!("http://{address}"))
            .call()
            .unwrap();
        let error = response.body_mut().read_to_vec().unwrap_err();
        assert!(matches!(error, ureq::Error::Timeout(_)), "{error:?}");
        release.send(()).unwrap();
        server.join().unwrap();
    }
}
