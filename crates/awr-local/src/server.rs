use crate::{ServiceStatus, lifecycle, registry};
use awr_core::{Error, Result};
use axum::{
    Router,
    body::Body,
    extract::{OriginalUri, Path as RoutePath, State},
    http::{HeaderMap, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    sync::{Semaphore, watch},
};

mod assets {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}

struct Server {
    dir: PathBuf,
    status: ServiceStatus,
    capacity: Semaphore,
    shutdown: watch::Sender<bool>,
}

fn failure(status: StatusCode, error: Error) -> Response {
    (
        status,
        axum::Json(json!({"ok":false,"error":error.report()})),
    )
        .into_response()
}

fn asset(name: &str) -> Response {
    let Some((_, bytes)) = assets::ASSETS.iter().find(|(key, _)| *key == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = match name.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, mime)], *bytes).into_response()
}

async fn boundary(State(server): State<Arc<Server>>, req: Request<Body>, next: Next) -> Response {
    let port = url::Url::parse(&server.status.url).unwrap().port().unwrap();
    let hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let valid_host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|host| hosts.iter().any(|allowed| allowed == host));
    let valid_origin = req.headers().get(header::ORIGIN).is_none_or(|value| {
        value
            .to_str()
            .is_ok_and(|origin| hosts.iter().any(|host| origin == format!("http://{host}")))
    });
    let valid_site = req
        .headers()
        .get("sec-fetch-site")
        .is_none_or(|value| value == "same-origin" || value == "none");
    if !valid_host || !valid_origin || !valid_site {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(req).await;
    for (key, value) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(key),
            value.parse().unwrap(),
        );
    }
    response
}

async fn projects(State(server): State<Arc<Server>>) -> Response {
    let dir = server.dir.clone();
    match tokio::task::spawn_blocking(move || {
        let projects = registry::list(&dir)?;
        let discovery = match crate::discovery_status(&dir) {
            Ok(report) => json!(report),
            Err(error) => json!({"state":"error","message":error.report().message}),
        };
        Ok::<_, Error>((projects, discovery, registry::is_default_state_dir(&dir)))
    }).await {
        Ok(Ok((projects, discovery, default_scope))) => {
            axum::Json(json!({"ok":true,"data":{"projects":projects,"discovery":discovery,"catalog_scope":if default_scope { "user" } else { "isolated" }}})).into_response()
        }
        Ok(Err(error)) => failure(StatusCode::INTERNAL_SERVER_ERROR, error),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn home(OriginalUri(uri): OriginalUri) -> Response {
    let project = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
        .any(|(name, _)| name == "project");
    asset(if project { "index.html" } else { "hub.html" })
}

fn number(
    query: &HashMap<String, String>,
    name: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<String> {
    let number = query
        .get(name)
        .map(|n| n.parse::<u64>())
        .transpose()
        .map_err(|_| Error::InvalidInput(format!("invalid {name}")))?
        .unwrap_or(default);
    if !(min..=max).contains(&number) {
        return Err(Error::InvalidInput(format!("{name} is out of range")));
    }
    Ok(number.to_string())
}

fn argv(route: &str, query: &HashMap<String, String>) -> Result<Vec<String>> {
    let mut args: Vec<String> = match route {
        "map/work-graph" => vec![
            "work".into(),
            "graph".into(),
            "--limit".into(),
            number(query, "limit", 100, 1, 1000)?,
        ],
        "map/nav" => vec!["nav".into(), "--cached".into()],
        "map/goals" => vec![
            "search".into(),
            "--type".into(),
            "goal".into(),
            "--limit".into(),
            "100".into(),
            "--cached".into(),
        ],
        "map/events" => vec![
            "event".into(),
            "history".into(),
            "--limit".into(),
            number(query, "limit", 1000, 1, 1000)?,
        ],
        "map/sessions" => vec![
            "session".into(),
            "list".into(),
            "--active".into(),
            "--limit".into(),
            "100".into(),
        ],
        "map/doctor" => vec!["doctor".into()],
        _ => return Err(Error::NotFound("progress route".into())),
    };
    if route == "map/work-graph" && query.get("cached").is_some_and(|v| v == "1") {
        args.push("--cached".into());
    }
    if route == "map/nav"
        && let Some(milestone) = query.get("milestone")
    {
        if milestone.is_empty()
            || milestone.len() > 200
            || !milestone
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:#/-".contains(&b))
        {
            return Err(Error::InvalidInput("invalid milestone key".into()));
        }
        args.extend(["--milestone".into(), milestone.clone()]);
    }
    if route == "map/events" {
        if query.contains_key("through") {
            args.extend([
                "--through-revision".into(),
                number(query, "through", 0, 0, 9_007_199_254_740_991)?,
            ]);
        }
        if let Some(cursor) = query.get("cursor") {
            if cursor.len() > 2048
                || !serde_json::from_str::<Value>(cursor).is_ok_and(|v| v.is_object())
            {
                return Err(Error::InvalidInput("invalid event cursor".into()));
            }
            args.extend(["--cursor".into(), cursor.clone()]);
        }
    }
    Ok(args)
}

async fn output(executable: &Path, root: &Path, args: Vec<String>) -> Result<Value> {
    let mut child = tokio::process::Command::new(executable)
        .arg("--project")
        .arg(root)
        .arg("--json")
        .args(args)
        .env("AWR_LOCAL_WEB", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = async move {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take(8 * 1024 * 1024 + 1);
        let mut stderr = stderr.take(1024 * 1024 + 1);
        let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        a?;
        b?;
        if out.len() > 8 * 1024 * 1024 || err.len() > 1024 * 1024 {
            return Err(Error::InvalidInput(
                "progress command output exceeds its limit".into(),
            ));
        }
        Ok::<_, Error>((out, err))
    };
    let (out, err) = tokio::time::timeout(Duration::from_secs(60), read)
        .await
        .map_err(|_| Error::Storage("progress command timed out".into()))??;
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .map_err(|_| Error::Storage("progress command did not exit".into()))??;
    let bytes = if out.is_empty() { &err } else { &out };
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| Error::Storage("progress command returned invalid JSON".into()))?;
    if value.get("code").is_some() && value.get("message").is_some() {
        return Ok(json!({"ok":false,"error":value}));
    }
    Ok(json!({"ok":status.success(),"data":value}))
}

async fn project_api(
    RoutePath((key, route)): RoutePath<(String, String)>,
    OriginalUri(uri): OriginalUri,
    State(server): State<Arc<Server>>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET
        && (route != "map/snapshot"
            || headers
                .get("x-awr-project-progress-overview")
                .is_none_or(|value| value != "1"))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let Ok(_permit) = server.capacity.try_acquire() else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            Error::Storage("local viewer is busy; retry shortly".into()),
        );
    };
    let entry = match registry::select(&server.dir, &key) {
        Ok(entry) => entry,
        Err(error) => return failure(StatusCode::NOT_FOUND, error),
    };
    if route == "map/config" {
        return axum::Json(
            json!({"ok":true,"data":{"config":null,"source":null,"bulk_snapshot":true}}),
        )
        .into_response();
    }
    if route == "map/snapshot" && method == Method::GET {
        let root = entry.root.clone();
        let result =
            tokio::task::spawn_blocking(move || awr_runtime::progress_snapshot(&root, false)).await;
        return match result {
            Ok(Ok(value)) => match registry::validate(&entry) {
                Ok(()) => axum::Json(json!({"ok":true,"data":value})).into_response(),
                Err(error) => failure(StatusCode::CONFLICT, error),
            },
            Ok(Err(error)) => failure(StatusCode::INTERNAL_SERVER_ERROR, error),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    let args = match if route == "map/snapshot" {
        Ok(vec![
            "overview".into(),
            "snapshot".into(),
            "--refresh".into(),
        ])
    } else {
        argv(&route, &query)
    } {
        Ok(args) => args,
        Err(error) => return failure(StatusCode::BAD_REQUEST, error),
    };
    match output(&server.status.executable, &entry.root, args).await {
        Ok(value) => match registry::validate(&entry) {
            Ok(()) => axum::Json(value).into_response(),
            Err(error) => failure(StatusCode::CONFLICT, error),
        },
        Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn shutdown(headers: HeaderMap, State(server): State<Arc<Server>>) -> Response {
    if headers
        .get("x-awr-local-instance")
        .is_none_or(|v| v != server.status.instance.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let _ = server.shutdown.send(true);
    axum::Json(json!({"ok":true})).into_response()
}

pub fn serve(dir: &Path, executable: &Path) -> Result<()> {
    let _lease = registry::lock(dir, "server.lock", Duration::ZERO)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let port = registry::read_json::<ServiceStatus>(&dir.join("service.json"))
            .ok()
            .and_then(|prior| url::Url::parse(&prior.url).ok())
            .and_then(|url| url.port())
            .unwrap_or(7381);
        let listener =
            match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
                Ok(listener) => listener,
                Err(_) => tokio::net::TcpListener::bind("127.0.0.1:0").await?,
            };
        let status = ServiceStatus {
            service: "awr-project-hub".into(),
            protocol: 1,
            instance: awr_core::Id::new().to_string(),
            pid: std::process::id(),
            url: format!("http://{}", listener.local_addr()?),
            executable: executable.canonicalize()?,
            binary_stamp: lifecycle::stamp(executable)?,
            version: env!("CARGO_PKG_VERSION").into(),
            started_at: awr_core::now_millis()?,
        };
        let (tx, mut rx) = watch::channel(false);
        let server = Arc::new(Server {
            dir: dir.into(),
            status: status.clone(),
            capacity: Semaphore::new(4),
            shutdown: tx,
        });
        let router = Router::new()
            .route("/", get(home))
            .route(
                "/api/health",
                get(|State(s): State<Arc<Server>>| async move {
                    axum::Json(json!({"ok":true,"data":s.status}))
                }),
            )
            .route("/api/projects", get(projects))
            .route(
                "/api/projects/{key}/{*route}",
                get(project_api).post(project_api),
            )
            .route("/api/service/stop", post(shutdown))
            .route(
                "/{*asset}",
                get(|RoutePath(name): RoutePath<String>| async move { asset(&name) }),
            )
            .layer(middleware::from_fn_with_state(server.clone(), boundary))
            .with_state(server);
        registry::write_json(&dir.join("service.json"), &status)?;
        let discovery = tokio::spawn(crate::discovery::monitor(dir.into(), rx.clone()));
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = rx.changed().await;
            })
            .await?;
        let _ = discovery.await;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_commands_are_read_only_and_validate_bounds() {
        let empty = HashMap::new();
        assert_eq!(
            argv("map/work-graph", &empty).unwrap(),
            ["work", "graph", "--limit", "100"]
        );
        assert_eq!(argv("map/nav", &empty).unwrap(), ["nav", "--cached"]);
        assert!(argv("work/complete", &empty).is_err());
        for invalid in ["0", "1001", "-1", "2.5", "; shell"] {
            assert!(
                argv(
                    "map/work-graph",
                    &HashMap::from([("limit".into(), invalid.into())])
                )
                .is_err()
            );
        }
        assert!(
            argv(
                "map/nav",
                &HashMap::from([("milestone".into(), "--json ; command".into())])
            )
            .is_err()
        );
        assert!(
            argv(
                "map/events",
                &HashMap::from([("cursor".into(), "[]".into())])
            )
            .is_err()
        );
    }

    #[test]
    fn native_payload_contains_the_retained_progress_assets() {
        for name in [
            "hub.html",
            "hub.js",
            "hub.css",
            "index.html",
            "app.js",
            "styles.css",
            "locales/en.js",
            "locales/zh-CN.js",
            "project-map/views.js",
            "project-map/interact.js",
        ] {
            assert!(
                assets::ASSETS
                    .iter()
                    .any(|(key, bytes)| *key == name && !bytes.is_empty()),
                "missing asset {name}"
            );
        }
    }
}
