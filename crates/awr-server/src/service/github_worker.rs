//! Optional fixed-repository workers. An observation never grants approval or completion.
use super::{ProjectBinding, ServiceConfig, delivery_sync::WorkerState};
use crate::{
    config::{GitHubWorkerConfig, GitHubWorkerSpec},
    delivery_adapter::{
        GitHubAdapter, GitHubError, GitHubIntegrationConfig, GitHubIntegrator,
        GitHubReceiveTransport, GitHubTransport,
    },
};
use awr_team_pg::{
    DeliveryReadSet, DeliveryScheduleQuery, DeliverySyncStore, DispatchDeliveryIntegration,
    LeaseDeliveryIntegration, PgError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, task::JoinHandle};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubWorkerFailure {
    AuthorizationUnavailable,
    PreconditionsChanged,
    Contention,
    ProviderUnavailable,
    ProviderAuthorizationUnavailable,
    RateLimited,
    ProofUnavailable,
    OutcomeUnknown,
    StoreUnavailable,
    InvalidQueueResponse,
}

/// Finite operator observations; these counters are not delivery receipts.
#[derive(Clone, Debug, Serialize)]
pub struct GitHubWorkerSnapshot {
    pub project: String,
    pub worker_id: String,
    pub state: WorkerState,
    pub failure_code: Option<GitHubWorkerFailure>,
    pub polls: u64,
    pub current_changed: u64,
    pub current_unchanged: u64,
    pub selection_waits: u64,
    pub visited_intents: u64,
    pub live_leases_observed: u64,
    pub terminal_intents_observed: u64,
    pub dispatch_attempts: u64,
    pub original_queries: u64,
    pub original_unchanged: u64,
    pub confirmed: u64,
    pub historical_confirmations: u64,
    pub superseded_observations: u64,
    pub unknown: u64,
    pub failures: u64,
    pub retry_delay_ms: u64,
    pub observed_at_unix_ms: u64,
}

type Snapshot = Arc<Mutex<GitHubWorkerSnapshot>>;
#[derive(Clone)]
pub struct GitHubWorkerMonitor {
    snapshots: Vec<Snapshot>,
}
impl GitHubWorkerMonitor {
    pub fn snapshots(&self) -> Vec<GitHubWorkerSnapshot> {
        self.snapshots
            .iter()
            .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .collect()
    }
}

fn observe(snapshot: &Snapshot, update: impl FnOnce(&mut GitHubWorkerSnapshot)) {
    let mut value = snapshot.lock().unwrap_or_else(|e| e.into_inner());
    let before = (value.state, value.failure_code);
    update(&mut value);
    value.observed_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    if before != (value.state, value.failure_code) {
        eprintln!(
            "{}",
            serde_json::json!({"service":"awr-github-worker","project":value.project,
            "worker_id":value.worker_id,"state":value.state,"failure_code":value.failure_code,
            "observed_at_unix_ms":value.observed_at_unix_ms})
        );
    }
}

/// Trusted library injection, never deserialized from TOML, HTTP or MCP input.
pub struct GitHubWorkerTransports {
    pub api: Arc<dyn GitHubTransport>,
    pub receive: Option<Arc<dyn GitHubReceiveTransport>>,
}

struct Worker {
    spec: GitHubWorkerSpec,
    binding: ProjectBinding,
    credential: Zeroizing<String>,
    observer: GitHubAdapter,
    integrator: Option<GitHubIntegrator>,
    snapshot: Snapshot,
}

pub(super) struct GitHubWorkerAdmission {
    workers: Vec<Worker>,
    store: Arc<DeliverySyncStore>,
}

pub struct GitHubWorkerRuntime {
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    monitor: GitHubWorkerMonitor,
}

impl GitHubWorkerRuntime {
    pub async fn start(
        service: &ServiceConfig,
        config: Option<GitHubWorkerConfig>,
        store: DeliverySyncStore,
        credential_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, String> {
        Self::start_with_transports(service, config, store, credential_lookup, |_, _| Ok(None))
            .await
    }

    /// Trusted transport extension. `None` chooses production HTTPS; configuration
    /// and fresh PG authority are checked even when the operator supplies transports.
    pub async fn start_with_transports(
        service: &ServiceConfig,
        config: Option<GitHubWorkerConfig>,
        store: DeliverySyncStore,
        credential_lookup: impl FnMut(&str) -> Option<String>,
        transports: impl FnMut(
            &GitHubWorkerSpec,
            Zeroizing<String>,
        ) -> Result<Option<GitHubWorkerTransports>, GitHubError>,
    ) -> Result<Self, String> {
        Ok(Self::admit(
            service,
            config,
            Arc::new(store),
            credential_lookup,
            transports,
        )
        .await?
        .spawn())
    }

    pub(super) async fn admit(
        service: &ServiceConfig,
        config: Option<GitHubWorkerConfig>,
        store: Arc<DeliverySyncStore>,
        mut credential_lookup: impl FnMut(&str) -> Option<String>,
        mut transports: impl FnMut(
            &GitHubWorkerSpec,
            Zeroizing<String>,
        ) -> Result<Option<GitHubWorkerTransports>, GitHubError>,
    ) -> Result<GitHubWorkerAdmission, String> {
        service.validate()?;
        let Some(config) = config else {
            return Ok(GitHubWorkerAdmission {
                workers: vec![],
                store,
            });
        };
        config.validate(service)?;
        let workers = tokio::time::timeout(Duration::from_secs(30), async {
            let mut workers = Vec::new();
            for spec in config.workers {
                let credential = Zeroizing::new(
                    credential_lookup(&spec.credential_env)
                        .ok_or_else(|| "GitHub worker credential is unavailable".to_string())?,
                );
                if credential.is_empty()
                    || credential.len() > 4096
                    || credential.chars().any(char::is_control)
                {
                    return Err("GitHub worker credential is invalid".to_string());
                }
                let binding = service
                    .projects
                    .iter()
                    .find(|p| p.key == spec.project)
                    .ok_or_else(|| "GitHub worker project is unavailable".to_string())?
                    .clone();
                tokio::time::timeout(Duration::from_millis(spec.operation_timeout_ms), async {
                    let schedule = store
                        .schedule(
                            &binding.tenant_id,
                            &binding.project_id,
                            &credential,
                            DeliveryScheduleQuery {
                                work_id: spec.repository.work_id.clone(),
                                connector_id: spec.repository.connector_id.clone(),
                                cursor: None,
                                limit: 1,
                            },
                        )
                        .await
                        .map_err(pg_failure)?;
                    let page = page(&spec, schedule)?;
                    let access = store
                        .check_worker_access(
                            &binding.tenant_id,
                            &binding.project_id,
                            &credential,
                            &page.read_set,
                            &spec.repository.connector_id,
                            spec.integration_enabled,
                        )
                        .await
                        .map_err(pg_failure)?;
                    if access["connector_version"] != page.connector.connector_version
                        || access["resource"] != spec.repository.resource
                    {
                        return Err(GitHubWorkerFailure::PreconditionsChanged);
                    }
                    Ok::<_, GitHubWorkerFailure>(())
                })
                .await
                .map_err(|_| "GitHub worker startup check timed out".to_string())?
                .map_err(|code| format!("GitHub worker startup check failed: {code:?}"))?;
                let provider_credential = Zeroizing::new(
                    credential_lookup(&spec.provider_credential_env)
                        .ok_or_else(|| "GitHub provider credential is unavailable".to_string())?,
                );
                if provider_credential.is_empty()
                    || provider_credential.len() > 4096
                    || provider_credential.chars().any(char::is_control)
                {
                    return Err("GitHub provider credential is invalid".to_string());
                }
                let supplied = transports(&spec, provider_credential.clone())
                    .map_err(|code| format!("GitHub worker transport check failed: {code:?}"))?;
                let (observer, integrator) = if let Some(supplied) = supplied {
                    let observer = GitHubAdapter::from_transport(
                        spec.repository.clone(),
                        supplied.api.clone(),
                    )
                    .map_err(|code| format!("GitHub worker adapter check failed: {code:?}"))?;
                    let integrator = if spec.integration_enabled {
                        Some(
                            GitHubIntegrator::from_transports(
                                GitHubIntegrationConfig {
                                    enabled: true,
                                    repository: spec.repository.clone(),
                                },
                                supplied.api,
                                supplied.receive.ok_or_else(|| {
                                    "GitHub effect transport is unavailable".to_string()
                                })?,
                            )
                            .map_err(|code| {
                                format!("GitHub worker integrator check failed: {code:?}")
                            })?,
                        )
                    } else {
                        None
                    };
                    (observer, integrator)
                } else {
                    let observer = GitHubAdapter::open(
                        spec.repository.clone(),
                        Some(provider_credential.clone()),
                    )
                    .map_err(|code| format!("GitHub worker adapter check failed: {code:?}"))?;
                    let integrator = if spec.integration_enabled {
                        Some(
                            GitHubIntegrator::open(
                                GitHubIntegrationConfig {
                                    enabled: true,
                                    repository: spec.repository.clone(),
                                },
                                provider_credential,
                            )
                            .await
                            .map_err(|code| {
                                format!("GitHub worker integrator check failed: {code:?}")
                            })?,
                        )
                    } else {
                        None
                    };
                    (observer, integrator)
                };
                let snapshot = Arc::new(Mutex::new(GitHubWorkerSnapshot {
                    project: spec.project.clone(),
                    worker_id: spec.worker_id.clone(),
                    state: WorkerState::Starting,
                    failure_code: None,
                    polls: 0,
                    current_changed: 0,
                    current_unchanged: 0,
                    selection_waits: 0,
                    visited_intents: 0,
                    live_leases_observed: 0,
                    terminal_intents_observed: 0,
                    dispatch_attempts: 0,
                    original_queries: 0,
                    original_unchanged: 0,
                    confirmed: 0,
                    historical_confirmations: 0,
                    superseded_observations: 0,
                    unknown: 0,
                    failures: 0,
                    retry_delay_ms: 0,
                    observed_at_unix_ms: 0,
                }));
                workers.push(Worker {
                    spec,
                    binding,
                    credential,
                    observer,
                    integrator,
                    snapshot,
                });
            }
            Ok::<_, String>(workers)
        })
        .await
        .map_err(|_| "GitHub worker startup checks timed out".to_string())??;
        Ok(GitHubWorkerAdmission { workers, store })
    }

    pub(super) fn empty() -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            stop,
            tasks: vec![],
            monitor: GitHubWorkerMonitor { snapshots: vec![] },
        }
    }
    pub fn monitor(&self) -> GitHubWorkerMonitor {
        self.monitor.clone()
    }
    pub fn request_stop(&self) {
        let _ = self.stop.send(true);
    }
    pub async fn shutdown(mut self) {
        self.request_stop();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        for mut task in self.tasks.drain(..) {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

impl GitHubWorkerAdmission {
    pub(super) fn spawn(self) -> GitHubWorkerRuntime {
        let (stop, receiver) = watch::channel(false);
        let mut runtime = GitHubWorkerRuntime {
            stop,
            tasks: vec![],
            monitor: GitHubWorkerMonitor { snapshots: vec![] },
        };
        for worker in self.workers {
            runtime.monitor.snapshots.push(worker.snapshot.clone());
            let stopped = StopObservation(worker.snapshot.clone());
            let store = self.store.clone();
            let receiver = receiver.clone();
            runtime.tasks.push(tokio::spawn(async move {
                let _on_stop = stopped;
                run(worker, store, receiver).await;
            }));
        }
        runtime
    }
}
impl Drop for GitHubWorkerRuntime {
    fn drop(&mut self) {
        self.request_stop();
        for task in &self.tasks {
            task.abort();
        }
    }
}
struct StopObservation(Snapshot);
impl Drop for StopObservation {
    fn drop(&mut self) {
        observe(&self.0, |s| {
            s.state = WorkerState::Stopped;
            s.retry_delay_ms = 0;
        });
    }
}

fn pg_failure(error: PgError) -> GitHubWorkerFailure {
    match error {
        PgError::Forbidden
        | PgError::ProjectNotAvailable
        | PgError::Workstream(awr_core::WorkstreamError::AccessDenied) => {
            GitHubWorkerFailure::AuthorizationUnavailable
        }
        PgError::ClaimHeld
        | PgError::ResourceConflict
        | PgError::StaleFence
        | PgError::LeaseExpired => GitHubWorkerFailure::Contention,
        PgError::PreconditionsChanged
        | PgError::EpochChanged
        | PgError::CursorExpired
        | PgError::SourceDivergence
        | PgError::InactiveCandidate
        | PgError::RecoveryBlocked
        | PgError::Workstream(_) => GitHubWorkerFailure::PreconditionsChanged,
        _ => GitHubWorkerFailure::StoreUnavailable,
    }
}
fn provider_failure(error: GitHubError) -> GitHubWorkerFailure {
    match error {
        GitHubError::Contention => GitHubWorkerFailure::Contention,
        GitHubError::AuthorizationUnavailable => GitHubWorkerFailure::AuthorizationUnavailable,
        GitHubError::ProviderAuthorizationUnavailable => {
            GitHubWorkerFailure::ProviderAuthorizationUnavailable
        }
        GitHubError::RateLimited => GitHubWorkerFailure::RateLimited,
        GitHubError::BindingMismatch
        | GitHubError::PreconditionsChanged
        | GitHubError::DomainRejected
        | GitHubError::ProviderPolicyUnsupported
        | GitHubError::UnsupportedGuarantee => GitHubWorkerFailure::PreconditionsChanged,
        GitHubError::ProviderUnavailable | GitHubError::InvalidConfiguration => {
            GitHubWorkerFailure::ProviderUnavailable
        }
        GitHubError::ReportUnavailable | GitHubError::ReportConflict => {
            GitHubWorkerFailure::ProofUnavailable
        }
        GitHubError::StoreUnavailable => GitHubWorkerFailure::StoreUnavailable,
        GitHubError::TimedOut => GitHubWorkerFailure::OutcomeUnknown,
        _ => GitHubWorkerFailure::InvalidQueueResponse,
    }
}
async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() || stop.changed().await.is_err() {
            return;
        }
    }
}
async fn bounded<T>(
    stop: &mut watch::Receiver<bool>,
    milliseconds: u64,
    future: impl Future<Output = Result<T, GitHubWorkerFailure>>,
) -> Option<Result<T, GitHubWorkerFailure>> {
    tokio::select! { biased;
        _ = stopped(stop) => None,
        value = tokio::time::timeout(Duration::from_millis(milliseconds), future) => Some(value.unwrap_or(Err(GitHubWorkerFailure::OutcomeUnknown))),
    }
}

#[derive(Deserialize)]
struct Connector {
    connector_id: String,
    connector_version: String,
    provider: String,
    resource: String,
}
#[derive(Deserialize)]
struct Page {
    read_set: DeliveryReadSet,
    connector: Connector,
    selection_state: String,
    integrations: Vec<Intent>,
    next_cursor: Option<String>,
    truncated: bool,
}
#[derive(Deserialize)]
struct Intent {
    integration_id: String,
    state: String,
    lease_live: bool,
    query_original: bool,
}
fn page(spec: &GitHubWorkerSpec, value: Value) -> Result<Page, GitHubWorkerFailure> {
    let page: Page =
        serde_json::from_value(value).map_err(|_| GitHubWorkerFailure::InvalidQueueResponse)?;
    if page.connector.connector_id != spec.repository.connector_id
        || page.connector.provider != "github"
        || page.connector.resource != spec.repository.resource
        || page.read_set.work_id != spec.repository.work_id
        || page.read_set.workstream_id.to_string() != spec.repository.workstream_id
        || !matches!(
            page.selection_state.as_str(),
            "current" | "missing" | "stale"
        )
        || page.truncated != page.next_cursor.is_some()
    {
        return Err(GitHubWorkerFailure::PreconditionsChanged);
    }
    Ok(page)
}

async fn original(
    worker: &Worker,
    store: &DeliverySyncStore,
    set: &DeliveryReadSet,
    intent: Intent,
    stop: &mut watch::Receiver<bool>,
) -> Option<Result<(), GitHubWorkerFailure>> {
    let Some(integrator) = &worker.integrator else {
        return Some(Ok(()));
    };
    let (tenant, project, bearer) = (
        worker.binding.tenant_id.as_str(),
        worker.binding.project_id.as_str(),
        worker.credential.as_str(),
    );
    let milliseconds = worker.spec.operation_timeout_ms;
    if matches!(intent.state.as_str(), "confirmed" | "rejected") {
        observe(&worker.snapshot, |s| {
            s.terminal_intents_observed = s.terminal_intents_observed.saturating_add(1)
        });
        return Some(Ok(()));
    }
    if !intent.query_original {
        if intent.state == "leased" && intent.lease_live {
            observe(&worker.snapshot, |s| {
                s.live_leases_observed = s.live_leases_observed.saturating_add(1)
            });
            return Some(Ok(()));
        }
        if !matches!(intent.state.as_str(), "prepared" | "leased") {
            return Some(Err(GitHubWorkerFailure::InvalidQueueResponse));
        }
        let lease = match bounded(stop, milliseconds, async {
            store
                .lease_integration(
                    tenant,
                    project,
                    bearer,
                    LeaseDeliveryIntegration {
                        request_id: format!("github-lease:{}", awr_core::Id::new()),
                        read_set: set.clone(),
                        integration_id: intent.integration_id.clone(),
                        lease_seconds: worker.spec.lease_seconds,
                    },
                )
                .await
                .map_err(pg_failure)
        })
        .await?
        {
            Ok(lease) => lease,
            Err(GitHubWorkerFailure::Contention) => return Some(Ok(())),
            Err(error) => return Some(Err(error)),
        };
        let Some(lease_id) = lease["data"]["lease_id"].as_str() else {
            return Some(Err(GitHubWorkerFailure::InvalidQueueResponse));
        };
        observe(&worker.snapshot, |s| {
            s.dispatch_attempts = s.dispatch_attempts.saturating_add(1)
        });
        if let Err(error) = bounded(stop, milliseconds, async {
            integrator
                .execute(
                    store,
                    bearer,
                    DispatchDeliveryIntegration {
                        request_id: format!("github-dispatch:{lease_id}"),
                        read_set: set.clone(),
                        integration_id: intent.integration_id.clone(),
                        lease_id: lease_id.into(),
                    },
                )
                .await
                .map_err(provider_failure)
        })
        .await?
        {
            // Cancellation or missing replies keep the durable original guard. A new
            // loop must discover dispatch state, never infer permission to launch again.
            return Some(Err(error));
        }
    }
    observe(&worker.snapshot, |s| {
        s.original_queries = s.original_queries.saturating_add(1)
    });
    bounded(stop, milliseconds, async {
        let value = integrator
            .reconcile_original_current(store, bearer, &intent.integration_id)
            .await
            .map_err(provider_failure)?;
        observe(&worker.snapshot, |s| {
            if value["unchanged"] == true {
                s.original_unchanged = s.original_unchanged.saturating_add(1);
            }
            if value["observation"]["data"]["observation_receipt"]["state"] == "superseded" {
                s.superseded_observations = s.superseded_observations.saturating_add(1);
            }
            let result = if value["integration"]["data"].is_object() {
                &value["integration"]["data"]
            } else {
                &value["integration"]
            };
            if result["state"] == "confirmed" {
                if result["current"] == true || result["confirmation_current"] == true {
                    s.confirmed = s.confirmed.saturating_add(1);
                } else {
                    s.historical_confirmations = s.historical_confirmations.saturating_add(1);
                }
            }
            if result["state"] == "unknown" {
                s.unknown = s.unknown.saturating_add(1);
            }
        });
        Ok(())
    })
    .await
}

async fn poll(
    worker: &Worker,
    store: &DeliverySyncStore,
    cursor: &mut Option<String>,
    stop: &mut watch::Receiver<bool>,
) -> Option<Result<(), GitHubWorkerFailure>> {
    observe(&worker.snapshot, |s| s.polls = s.polls.saturating_add(1));
    let mut visited = 0;
    let mut failure = None;
    for index in 0..worker.spec.max_pages_per_poll {
        let remaining = worker.spec.max_jobs_per_poll.saturating_sub(visited);
        if remaining == 0 {
            break;
        }
        let limit = worker.spec.page_size.min(remaining);
        let value = bounded(stop, worker.spec.operation_timeout_ms, async {
            store
                .schedule(
                    &worker.binding.tenant_id,
                    &worker.binding.project_id,
                    &worker.credential,
                    DeliveryScheduleQuery {
                        work_id: worker.spec.repository.work_id.clone(),
                        connector_id: worker.spec.repository.connector_id.clone(),
                        cursor: cursor.clone(),
                        limit: i32::from(limit),
                    },
                )
                .await
                .map_err(pg_failure)
        })
        .await?;
        let page = match value.and_then(|v| page(&worker.spec, v)) {
            Ok(page) => page,
            Err(error) => {
                *cursor = None;
                return Some(Err(error));
            }
        };
        if page.integrations.len() > usize::from(limit) {
            return Some(Err(GitHubWorkerFailure::InvalidQueueResponse));
        }
        if index == 0 {
            if page.selection_state == "current" {
                match bounded(stop, worker.spec.operation_timeout_ms, async {
                    worker
                        .observer
                        .reconcile_current(store, &worker.credential)
                        .await
                        .map_err(provider_failure)
                })
                .await?
                {
                    Ok(value) => observe(&worker.snapshot, |s| {
                        if value["unchanged"] == true {
                            s.current_unchanged = s.current_unchanged.saturating_add(1);
                        } else if value["observation"]["data"]["observation_receipt"]["state"]
                            == "applied"
                        {
                            s.current_changed = s.current_changed.saturating_add(1);
                        } else {
                            s.superseded_observations = s.superseded_observations.saturating_add(1);
                        }
                    }),
                    Err(GitHubWorkerFailure::AuthorizationUnavailable) => {
                        return Some(Err(GitHubWorkerFailure::AuthorizationUnavailable));
                    }
                    Err(error) => failure = Some(error),
                }
            } else {
                observe(&worker.snapshot, |s| {
                    s.selection_waits = s.selection_waits.saturating_add(1)
                });
            }
        }
        if worker.integrator.is_none() {
            *cursor = None;
            break;
        }
        for intent in page.integrations {
            visited += 1;
            observe(&worker.snapshot, |s| {
                s.visited_intents = s.visited_intents.saturating_add(1)
            });
            if let Err(error) = original(worker, store, &page.read_set, intent, stop).await? {
                if error == GitHubWorkerFailure::AuthorizationUnavailable {
                    return Some(Err(error));
                }
                failure = Some(error);
            }
        }
        // Every returned row has been handled. No partial page can advance past
        // unvisited original requests, even when terminal history fills a page.
        *cursor = page.next_cursor;
        if !page.truncated {
            break;
        }
    }
    Some(failure.map_or(Ok(()), Err))
}

async fn run(worker: Worker, store: Arc<DeliverySyncStore>, mut stop: watch::Receiver<bool>) {
    let mut cursor = None;
    let mut delay = worker.spec.poll_interval_ms;
    observe(&worker.snapshot, |s| s.state = WorkerState::Running);
    loop {
        let Some(result) = poll(&worker, &store, &mut cursor, &mut stop).await else {
            return;
        };
        match result {
            Ok(()) => {
                delay = worker.spec.poll_interval_ms;
                observe(&worker.snapshot, |s| {
                    s.state = WorkerState::Running;
                    s.failure_code = None;
                    s.retry_delay_ms = delay;
                });
            }
            Err(error) => {
                delay = delay.saturating_mul(2).min(worker.spec.max_backoff_ms);
                observe(&worker.snapshot, |s| {
                    s.state = WorkerState::Backoff;
                    s.failure_code = Some(error);
                    s.failures = s.failures.saturating_add(1);
                    s.retry_delay_ms = delay;
                });
            }
        }
        tokio::select! { biased; _ = stopped(&mut stop) => return, _ = tokio::time::sleep(Duration::from_millis(delay)) => {} }
    }
}
