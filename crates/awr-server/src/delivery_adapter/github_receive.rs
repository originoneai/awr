//! One bounded HTTPS receive-pack command with a server-enforced old-object CAS.
use super::{GitHubConfig, GitHubError};
use awr_team::delivery::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHubReceiveMethod {
    Advertise,
    Apply,
}

pub struct GitHubReceiveResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Trusted operator injection only, never an HTTP/MCP execution input.
pub trait GitHubReceiveTransport: Send + Sync {
    fn request(
        &self,
        method: GitHubReceiveMethod,
        url: &Url,
        body: &[u8],
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<GitHubReceiveResponse, GitHubError>;
}

pub(super) struct HttpsReceivePack {
    agent: ureq::Agent,
    credential: Zeroizing<String>,
}

impl HttpsReceivePack {
    pub(super) fn new(credential: Zeroizing<String>) -> Result<Self, GitHubError> {
        if credential.is_empty()
            || credential.len() > 4096
            || !credential.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(GitHubError::InvalidConfiguration);
        }
        Ok(Self {
            agent: ureq::Agent::config_builder()
                .https_only(true)
                .proxy(None)
                .max_redirects(0)
                .max_redirects_will_error(false)
                .http_status_as_error(false)
                .user_agent("AWR-delivery-integration/1")
                .build()
                .into(),
            credential,
        })
    }
}

impl GitHubReceiveTransport for HttpsReceivePack {
    fn request(
        &self,
        method: GitHubReceiveMethod,
        url: &Url,
        body: &[u8],
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<GitHubReceiveResponse, GitHubError> {
        // The URL contains no credential. The separately held token is used by
        // both the API observer and the mapped Git endpoint, with no helper.
        let basic = Zeroizing::new(format!("x-access-token:{}", self.credential.as_str()));
        let authorization = Zeroizing::new(format!("Basic {}", STANDARD.encode(basic.as_bytes())));
        let result = match method {
            GitHubReceiveMethod::Advertise => self
                .agent
                .get(url.as_str())
                .header("Authorization", authorization.as_str())
                .header("Accept", "application/x-git-receive-pack-advertisement")
                .config()
                .timeout_global(Some(timeout))
                .build()
                .call(),
            GitHubReceiveMethod::Apply => self
                .agent
                .post(url.as_str())
                .header("Authorization", authorization.as_str())
                .header("Content-Type", "application/x-git-receive-pack-request")
                .header("Accept", "application/x-git-receive-pack-result")
                .config()
                .timeout_global(Some(timeout))
                .build()
                .send(body),
        };
        let mut response = result.map_err(|e| match e {
            ureq::Error::Timeout(_) => GitHubError::TimedOut,
            ureq::Error::BodyExceedsLimit(_) => GitHubError::OutputLimit,
            _ => GitHubError::ProviderUnavailable,
        })?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = if status == 200 {
            response
                .body_mut()
                .with_config()
                .limit(max_bytes as u64)
                .read_to_vec()
                .map_err(|e| match e {
                    ureq::Error::Timeout(_) => GitHubError::TimedOut,
                    ureq::Error::BodyExceedsLimit(_) => GitHubError::OutputLimit,
                    _ => GitHubError::ProviderUnavailable,
                })?
        } else {
            Vec::new() // Never read or emit a provider error body.
        };
        Ok(GitHubReceiveResponse {
            status,
            content_type,
            body,
        })
    }
}

pub(super) fn urls(config: &GitHubConfig) -> Result<(Url, Url), GitHubError> {
    let mut base =
        Url::parse(&config.api_base_url).map_err(|_| GitHubError::InvalidConfiguration)?;
    if base.host_str() == Some("api.github.com") && base.path() == "/" && base.port().is_none() {
        base.set_host(Some("github.com"))
            .map_err(|_| GitHubError::InvalidConfiguration)?;
    }
    base.set_path("/");
    base.path_segments_mut()
        .map_err(|_| GitHubError::InvalidConfiguration)?
        .pop_if_empty()
        .extend([&config.owner, &format!("{}.git", config.repository)]);
    let mut advertise = base.clone();
    advertise
        .path_segments_mut()
        .unwrap()
        .extend(["info", "refs"]);
    advertise
        .query_pairs_mut()
        .append_pair("service", "git-receive-pack");
    base.path_segments_mut().unwrap().push("git-receive-pack");
    Ok((advertise, base))
}

fn packets(mut input: &[u8]) -> Result<Vec<Option<&[u8]>>, GitHubError> {
    let mut result = Vec::new();
    while !input.is_empty() {
        if input.len() < 4 || result.len() >= 8192 {
            return Err(GitHubError::InvalidResponse);
        }
        let size = std::str::from_utf8(&input[..4])
            .ok()
            .and_then(|s| usize::from_str_radix(s, 16).ok())
            .ok_or(GitHubError::InvalidResponse)?;
        input = &input[4..];
        if size == 0 {
            result.push(None);
        } else if !(4..=65520).contains(&size) || size - 4 > input.len() {
            return Err(GitHubError::InvalidResponse);
        } else {
            result.push(Some(&input[..size - 4]));
            input = &input[size - 4..];
        }
    }
    Ok(result)
}

fn object_id(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Missing/duplicate target or unsupported capabilities are never an invitation
/// to fall back to REST or a blind reference update.
pub(super) fn advertisement(input: &[u8], reference: &str, old: &str) -> Result<(), GitHubError> {
    let parsed = packets(input)?;
    if parsed.len() < 4
        || parsed[0] != Some(b"# service=git-receive-pack\n".as_slice())
        || parsed[1].is_some()
        || parsed.last().is_none_or(Option::is_some)
    {
        return Err(GitHubError::InvalidResponse);
    }
    let mut target = None;
    for (index, line) in parsed[2..parsed.len() - 1].iter().enumerate() {
        let line = line.ok_or(GitHubError::InvalidResponse)?;
        let text = std::str::from_utf8(line)
            .map_err(|_| GitHubError::InvalidResponse)?
            .strip_suffix('\n')
            .unwrap_or(std::str::from_utf8(line).unwrap());
        let (entry, capabilities) = match text.split_once('\0') {
            Some(pair) if index == 0 && !pair.1.contains('\0') => (pair.0, Some(pair.1)),
            None if index != 0 => (text, None),
            _ => return Err(GitHubError::InvalidResponse),
        };
        if let Some(capabilities) = capabilities {
            let caps: Vec<_> = capabilities.split_ascii_whitespace().collect();
            if !caps.contains(&"atomic")
                || !caps.contains(&"report-status")
                || caps
                    .iter()
                    .any(|c| c.starts_with("object-format=") && *c != "object-format=sha1")
            {
                return Err(GitHubError::UnsupportedGuarantee);
            }
        }
        let (id, name) = entry.split_once(' ').ok_or(GitHubError::InvalidResponse)?;
        if !object_id(id)
            || name.is_empty()
            || name.chars().any(char::is_control)
            || name.contains(' ')
        {
            return Err(GitHubError::InvalidResponse);
        }
        if name == reference {
            if target.is_some() {
                return Err(GitHubError::InvalidResponse);
            }
            target = Some(id);
        }
    }
    if target != Some(old) {
        return Err(GitHubError::PreconditionsChanged);
    }
    Ok(())
}

fn packet(body: &[u8]) -> Vec<u8> {
    let mut result = format!("{:04x}", body.len() + 4).into_bytes();
    result.extend(body);
    result
}

pub(super) fn payload(candidate: &DeliveryCandidate) -> Result<Vec<u8>, GitHubError> {
    let source = candidate
        .binding
        .source_revision
        .as_ref()
        .ok_or(GitHubError::BindingMismatch)?;
    let TargetPrecondition::Exact(old) = &candidate.binding.target.precondition else {
        return Err(GitHubError::UnsupportedGuarantee);
    };
    let reference = candidate
        .binding
        .target
        .reference
        .as_deref()
        .ok_or(GitHubError::BindingMismatch)?;
    if source.format != RevisionFormat::GitSha1
        || old.format != RevisionFormat::GitSha1
        || !object_id(&source.value)
        || !object_id(&old.value)
        || !reference.starts_with("refs/heads/")
        || reference.len() > 1024
        || reference.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        return Err(GitHubError::UnsupportedGuarantee);
    }
    let mut bytes = packet(
        format!(
            "{} {} {}\0report-status atomic\n",
            old.value, source.value, reference
        )
        .as_bytes(),
    );
    bytes.extend(b"0000");
    // PACK v2, zero objects and its standard SHA-1 trailer. The source commit
    // and every object already exist on the same provider; no content upload.
    bytes.extend(b"PACK\0\0\0\x02\0\0\0\0");
    bytes.extend([
        0x02, 0x9d, 0x08, 0x82, 0x3b, 0xd8, 0xa8, 0xea, 0xb5, 0x10, 0xad, 0x6a, 0xc7, 0x5c, 0x82,
        0x3c, 0xfd, 0x3e, 0xd3, 0x1e,
    ]);
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PushStatus {
    Accepted,
    Rejected,
}

pub(super) fn status(input: &[u8], reference: &str) -> Result<PushStatus, GitHubError> {
    let parsed = packets(input)?;
    if parsed.len() != 3 || parsed[0] != Some(b"unpack ok\n".as_slice()) || parsed[2].is_some() {
        return Err(GitHubError::InvalidResponse);
    }
    let line = std::str::from_utf8(parsed[1].ok_or(GitHubError::InvalidResponse)?)
        .map_err(|_| GitHubError::InvalidResponse)?;
    if line == format!("ok {reference}\n") {
        Ok(PushStatus::Accepted)
    } else if line.starts_with(&format!("ng {reference} ")) && line.ends_with('\n') {
        Ok(PushStatus::Rejected) // The raw provider reason never enters a report.
    } else {
        Err(GitHubError::InvalidResponse)
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

    const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const REF: &str = "refs/heads/main";

    fn advert(caps: &str, entries: &[&str]) -> Vec<u8> {
        let mut bytes = packet(b"# service=git-receive-pack\n");
        bytes.extend(b"0000");
        for (i, entry) in entries.iter().enumerate() {
            let line = if i == 0 {
                format!("{entry}\0{caps}\n")
            } else {
                format!("{entry}\n")
            };
            bytes.extend(packet(line.as_bytes()));
        }
        bytes.extend(b"0000");
        bytes
    }

    #[test]
    fn advertisement_requires_exact_old_target_and_server_guarantees() {
        let entry = format!("{OLD} {REF}");
        assert_eq!(
            advertisement(
                &advert("report-status atomic object-format=sha1", &[&entry]),
                REF,
                OLD
            ),
            Ok(())
        );
        for caps in [
            "atomic",
            "report-status",
            "report-status atomic object-format=sha256",
        ] {
            assert_eq!(
                advertisement(&advert(caps, &[&entry]), REF, OLD),
                Err(GitHubError::UnsupportedGuarantee)
            );
        }
        assert_eq!(
            advertisement(&advert("report-status atomic", &[&entry]), REF, NEW),
            Err(GitHubError::PreconditionsChanged)
        );
        assert_eq!(
            advertisement(&advert("report-status atomic", &[&entry, &entry]), REF, OLD),
            Err(GitHubError::InvalidResponse)
        );
        assert!(advertisement(&advert("report-status atomic", &[]), REF, OLD).is_err());
        for bad in [
            b"zzzz".as_slice(),
            b"0001",
            b"0002",
            b"0003",
            b"ffff",
            b"0008abc",
            b"0000junk",
        ] {
            assert!(packets(bad).is_err());
        }
        let mut trailing = advert("report-status atomic", &[&entry]);
        trailing.extend(packet(b"unrequested extra\n"));
        assert!(advertisement(&trailing, REF, OLD).is_err());
    }

    #[test]
    fn report_status_is_exact_and_never_returns_raw_provider_reasons() {
        let mut ok = packet(b"unpack ok\n");
        ok.extend(packet(format!("ok {REF}\n").as_bytes()));
        ok.extend(b"0000");
        assert_eq!(status(&ok, REF), Ok(PushStatus::Accepted));
        let mut rejected = packet(b"unpack ok\n");
        rejected.extend(packet(
            format!("ng {REF} private provider text\n").as_bytes(),
        ));
        rejected.extend(b"0000");
        assert_eq!(status(&rejected, REF), Ok(PushStatus::Rejected));
        assert!(status(&ok, "refs/heads/other").is_err());
        ok.extend(b"0000");
        assert!(status(&ok, REF).is_err());
        assert!(status(b"200 OK", REF).is_err());
    }

    struct Server {
        url: Url,
        certificate: Vec<u8>,
        requests: Arc<Mutex<Vec<Vec<u8>>>>,
        stop: Arc<AtomicBool>,
        task: Option<thread::JoinHandle<()>>,
    }
    impl Server {
        fn start(status: u16, extra: &str, body: &[u8], delay: u64) -> Self {
            let key = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            let certificate = key.cert.der().to_vec();
            let tls = Arc::new(
                rustls::ServerConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![key.cert.der().clone()],
                    rustls::pki_types::PrivateKeyDer::Pkcs8(key.signing_key.serialize_der().into()),
                )
                .unwrap(),
            );
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stopped = stop.clone();
            let seen = requests.clone();
            let mut response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
                body.len()
            )
            .into_bytes();
            response.extend(body);
            let task = thread::spawn(move || {
                while !stopped.load(Ordering::Acquire) {
                    let Ok((socket, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut stream = rustls::StreamOwned::new(
                        rustls::ServerConnection::new(tls.clone()).unwrap(),
                        socket,
                    );
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while request.len() < 8192 {
                        let n = stream.read(&mut buffer).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        request.extend(&buffer[..n]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head =
                                String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                            let length = head
                                .lines()
                                .find_map(|s| s.strip_prefix("content-length: "))
                                .and_then(|s| s.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if request.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        seen.lock().unwrap().push(request);
                        thread::sleep(Duration::from_millis(delay));
                        let _ = stream.write_all(&response);
                        let _ = stream.flush();
                    }
                }
            });
            Self {
                url: Url::parse(&format!(
                    "https://localhost:{port}/acme/demo.git/git-receive-pack"
                ))
                .unwrap(),
                certificate,
                requests,
                stop,
                task: Some(task),
            }
        }
        fn trusted(&self) -> HttpsReceivePack {
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
            HttpsReceivePack {
                agent,
                credential: Zeroizing::new("synthetic-test-secret".into()),
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            self.task.take().unwrap().join().unwrap();
        }
    }

    #[test]
    fn actual_https_post_preserves_payload_and_uses_separately_held_basic_credential() {
        let server = Server::start(
            200,
            "Content-Type: application/x-git-receive-pack-result\r\n",
            b"0000",
            0,
        );
        let response = server
            .trusted()
            .request(
                GitHubReceiveMethod::Apply,
                &server.url,
                b"PACK\0fixture",
                Duration::from_secs(2),
                1024,
            )
            .unwrap();
        assert_eq!(response.body, b"0000");
        let requests = server.requests.lock().unwrap();
        let raw = &requests[0];
        let text = String::from_utf8_lossy(raw).to_ascii_lowercase();
        assert!(text.starts_with("post /acme/demo.git/git-receive-pack http/1.1"));
        assert!(text.contains("content-type: application/x-git-receive-pack-request"));
        assert!(
            text.contains(
                &format!(
                    "authorization: basic {}",
                    STANDARD.encode("x-access-token:synthetic-test-secret")
                )
                .to_ascii_lowercase()
            )
        );
        assert!(raw.ends_with(b"PACK\0fixture"));
    }

    #[test]
    fn actual_https_get_redirect_error_tls_size_and_timeout_contracts_are_enforced() {
        let server = Server::start(
            200,
            "Content-Type: application/x-git-receive-pack-advertisement\r\n",
            b"0000",
            0,
        );
        assert!(
            HttpsReceivePack::new(Zeroizing::new("fixture".into()))
                .unwrap()
                .request(
                    GitHubReceiveMethod::Advertise,
                    &server.url,
                    &[],
                    Duration::from_secs(1),
                    1024
                )
                .is_err()
        );
        let mut plaintext = server.url.clone();
        plaintext.set_scheme("http").unwrap();
        assert!(
            server
                .trusted()
                .request(
                    GitHubReceiveMethod::Advertise,
                    &plaintext,
                    &[],
                    Duration::from_secs(1),
                    1024
                )
                .is_err()
        );
        assert!(server.requests.lock().unwrap().is_empty());
        let ok = server
            .trusted()
            .request(
                GitHubReceiveMethod::Advertise,
                &server.url,
                &[],
                Duration::from_secs(1),
                1024,
            )
            .unwrap();
        assert_eq!(ok.body, b"0000");
        assert!(String::from_utf8_lossy(&server.requests.lock().unwrap()[0]).starts_with("GET "));
        for status in [302, 401, 403, 429, 500] {
            let s = Server::start(
                status,
                "Location: https://invalid.example/credential-leak\r\n",
                b"private error text",
                0,
            );
            let r = s
                .trusted()
                .request(
                    GitHubReceiveMethod::Apply,
                    &s.url,
                    b"fixture",
                    Duration::from_secs(1),
                    1024,
                )
                .unwrap();
            assert_eq!(r.status, status);
            assert!(r.body.is_empty());
            assert_eq!(s.requests.lock().unwrap().len(), 1);
        }
        let size = Server::start(200, "", &vec![b'x'; 2048], 0);
        assert_eq!(
            size.trusted()
                .request(
                    GitHubReceiveMethod::Apply,
                    &size.url,
                    b"fixture",
                    Duration::from_secs(1),
                    1024
                )
                .err(),
            Some(GitHubError::OutputLimit)
        );
        let slow = Server::start(200, "", b"0000", 150);
        assert_eq!(
            slow.trusted()
                .request(
                    GitHubReceiveMethod::Apply,
                    &slow.url,
                    b"fixture",
                    Duration::from_millis(30),
                    1024
                )
                .err(),
            Some(GitHubError::TimedOut)
        );
    }
}
