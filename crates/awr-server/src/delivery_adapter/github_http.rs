//! The production transport only performs bounded GETs to operator-mapped HTTPS URLs.
use super::github::GitHubError;
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

pub struct GitHubResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Trusted operator extension for synthetic fixtures or a managed HTTP transport.
/// This is not an RPC input and cannot grant connector or repository authority.
pub trait GitHubTransport: Send + Sync {
    fn get(
        &self,
        url: &Url,
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<GitHubResponse, GitHubError>;
}

pub(super) struct HttpsTransport {
    agent: ureq::Agent,
    credential: Option<Zeroizing<String>>,
}

impl HttpsTransport {
    pub(super) fn new(credential: Option<Zeroizing<String>>) -> Result<Self, GitHubError> {
        if credential.as_ref().is_some_and(|s| {
            s.is_empty() || s.len() > 4096 || !s.bytes().all(|b| b.is_ascii_graphic())
        }) {
            return Err(GitHubError::InvalidConfiguration);
        }
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .user_agent("AWR-delivery-observer/1")
            .build();
        Ok(Self {
            agent: config.into(),
            credential,
        })
    }
}

fn error(e: ureq::Error) -> GitHubError {
    match e {
        ureq::Error::Timeout(_) => GitHubError::TimedOut,
        ureq::Error::BodyExceedsLimit(_) => GitHubError::OutputLimit,
        _ => GitHubError::ProviderUnavailable,
    }
}

impl GitHubTransport for HttpsTransport {
    fn get(
        &self,
        url: &Url,
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<GitHubResponse, GitHubError> {
        let mut request = self
            .agent
            .get(url.as_str())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(secret) = &self.credential {
            request = request.header("Authorization", format!("Bearer {}", secret.as_str()));
        }
        let mut response = request
            .config()
            .timeout_global(Some(timeout))
            .timeout_resolve(Some(timeout))
            .timeout_connect(Some(timeout))
            .build()
            .call()
            .map_err(error)?;
        let status = response.status().as_u16();
        // GitHub's primary rate limit commonly uses 403 rather than 429.
        if status == 403
            && response
                .headers()
                .get("x-ratelimit-remaining")
                .is_some_and(|v| v == "0")
        {
            return Err(GitHubError::RateLimited);
        }
        // Provider error bodies can include credentials, source or user text.
        // Neither parse them nor attach them to our finite diagnostic enum.
        let body = if status == 200 {
            response
                .body_mut()
                .with_config()
                .limit(max_bytes as u64)
                .read_to_vec()
                .map_err(error)?
        } else {
            Vec::new()
        };
        Ok(GitHubResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    struct Server {
        url: Url,
        certificate: Vec<u8>,
        requests: Arc<Mutex<Vec<Vec<u8>>>>,
        stop: Arc<AtomicBool>,
        task: Option<thread::JoinHandle<()>>,
    }
    impl Server {
        fn start(status: u16, headers: &str, body: &str, delay: u64) -> Self {
            let key = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            let certificate = key.cert.der().to_vec();
            let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![key.cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key.signing_key.serialize_der().into()),
            )
            .unwrap();
            let tls = Arc::new(tls);
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stopped = stop.clone();
            let seen = requests.clone();
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                body.len()
            );
            let task = thread::spawn(move || {
                while !stopped.load(Ordering::Acquire) {
                    let Ok((socket, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    // macOS can inherit the listener's nonblocking mode on accept.
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let connection = rustls::ServerConnection::new(tls.clone()).unwrap();
                    let mut stream = rustls::StreamOwned::new(connection, socket);
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 512];
                    while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                        let count = stream.read(&mut buffer).unwrap_or(0);
                        if count == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..count]);
                    }
                    if request.ends_with(b"\r\n\r\n") {
                        seen.lock().unwrap().push(request);
                        thread::sleep(Duration::from_millis(delay));
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                    }
                }
            });
            Self {
                url: Url::parse(&format!(
                    "https://localhost:{}/repos/acme/demo",
                    address.port()
                ))
                .unwrap(),
                certificate,
                requests,
                stop,
                task: Some(task),
            }
        }
        fn trusted(&self) -> HttpsTransport {
            // Trust this ephemeral test certificate while preserving hostname and
            // chain verification. There is no insecure runtime TLS option.
            let root = ureq::tls::Certificate::from_der(&self.certificate).to_owned();
            let agent = ureq::Agent::config_builder()
                .https_only(true)
                .proxy(None)
                .max_redirects(0)
                .max_redirects_will_error(false)
                .http_status_as_error(false)
                .tls_config(
                    ureq::tls::TlsConfig::builder()
                        .root_certs(vec![root].into())
                        .build(),
                )
                .build()
                .into();
            HttpsTransport {
                agent,
                credential: Some(Zeroizing::new("synthetic-test-secret".into())),
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(task) = self.task.take() {
                task.join().unwrap();
            }
        }
    }

    #[test]
    fn github_https_transport_verifies_tls_and_sends_only_the_pinned_get_contract() {
        let server = Server::start(200, "Content-Type: application/json\r\n", "{}", 0);
        let response = server
            .trusted()
            .get(&server.url, Duration::from_secs(2), 1024)
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{}");
        let requests = server.requests.lock().unwrap();
        let text = String::from_utf8_lossy(&requests[0]).to_lowercase();
        assert!(text.starts_with("get /repos/acme/demo http/1.1"));
        assert!(text.contains("x-github-api-version: 2022-11-28"));
        assert!(text.contains("authorization: bearer synthetic-test-secret"));
    }

    #[test]
    fn github_https_transport_does_not_follow_redirects_or_read_error_bodies() {
        for status in [302, 401, 403, 429, 500] {
            let server = Server::start(
                status,
                "Location: https://elsewhere.invalid/secret\r\n",
                "synthetic-sensitive-error-text",
                0,
            );
            let response = server
                .trusted()
                .get(&server.url, Duration::from_secs(2), 1024)
                .unwrap();
            assert_eq!(response.status, status);
            assert!(response.body.is_empty());
            assert_eq!(server.requests.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn github_https_transport_rejects_untrusted_tls_and_plaintext() {
        let server = Server::start(200, "", "{}", 0);
        let client = HttpsTransport::new(None).unwrap();
        assert!(
            client
                .get(&server.url, Duration::from_secs(2), 1024)
                .is_err()
        );
        let mut url = server.url.clone();
        url.set_scheme("http").unwrap();
        assert!(client.get(&url, Duration::from_secs(2), 1024).is_err());
        assert!(server.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn github_https_transport_classifies_primary_rate_limit_without_error_body() {
        let server = Server::start(
            403,
            "X-RateLimit-Remaining: 0\r\n",
            "synthetic-sensitive-error-text",
            0,
        );
        assert_eq!(
            server
                .trusted()
                .get(&server.url, Duration::from_secs(2), 1024)
                .err(),
            Some(GitHubError::RateLimited)
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn github_https_transport_enforces_real_body_limits_and_timeout() {
        let large = Server::start(200, "", &"x".repeat(2048), 0);
        assert_eq!(
            large
                .trusted()
                .get(&large.url, Duration::from_secs(2), 1024)
                .err(),
            Some(GitHubError::OutputLimit)
        );
        let slow = Server::start(200, "", "{}", 250);
        assert_eq!(
            slow.trusted()
                .get(&slow.url, Duration::from_millis(100), 1024)
                .err(),
            Some(GitHubError::TimedOut)
        );
    }
}
