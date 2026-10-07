//! Optional fixed-repository workers. An observation never grants approval or completion.
use super::{ProjectBinding, ServiceConfig, delivery_sync::WorkerState};
use crate::{
    config::{LocalGitWorkerConfig, LocalGitWorkerSpec},
    delivery_adapter::{
        LocalGitAdapter, LocalGitError, LocalGitIntegrationConfig, LocalGitIntegrator,
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
pub enum LocalGitWorkerFailure {
    AuthorizationUnavailable,
    PreconditionsChanged,
    Contention,
    RepositoryUnavailable,
    ProofUnavailable,
    OutcomeUnknown,
    StoreUnavailable,
    InvalidQueueResponse,
}

/// Finite operator observations; these counters are not delivery receipts.
#[derive(Clone, Debug, Serialize)]
pub struct LocalGitWorkerSnapshot {
    pub project: String,
    pub worker_id: String,
    pub state: WorkerState,
    pub failure_code: Option<LocalGitWorkerFailure>,
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

type Snapshot = Arc<Mutex<LocalGitWorkerSnapshot>>;
#[derive(Clone)]
pub struct LocalGitWorkerMonitor {
    snapshots: Vec<Snapshot>,
}
impl LocalGitWorkerMonitor {
    pub fn snapshots(&self) -> Vec<LocalGitWorkerSnapshot> {
        self.snapshots
            .iter()
            .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .collect()
    }
}

fn observe(snapshot: &Snapshot, update: impl FnOnce(&mut LocalGitWorkerSnapshot)) {
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
            serde_json::json!({"service":"awr-local-git-worker","project":value.project,
            "worker_id":value.worker_id,"state":value.state,"failure_code":value.failure_code,
            "observed_at_unix_ms":value.observed_at_unix_ms})
        );
    }
}

struct Worker {
    spec: LocalGitWorkerSpec,
    binding: ProjectBinding,
    credential: Zeroizing<String>,
    observer: LocalGitAdapter,
    integrator: Option<LocalGitIntegrator>,
    snapshot: Snapshot,
}

pub(super) struct LocalGitWorkerAdmission {
    workers: Vec<Worker>,
    store: Arc<DeliverySyncStore>,
}

pub struct LocalGitWorkerRuntime {
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    monitor: LocalGitWorkerMonitor,
}

impl LocalGitWorkerRuntime {
    pub async fn start(
        service: &ServiceConfig,
        config: Option<LocalGitWorkerConfig>,
        store: DeliverySyncStore,
        credential_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, String> {
        Ok(
            Self::admit(service, config, Arc::new(store), credential_lookup)
                .await?
                .spawn(),
        )
    }

    pub(super) async fn admit(
        service: &ServiceConfig,
        config: Option<LocalGitWorkerConfig>,
        store: Arc<DeliverySyncStore>,
        mut credential_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<LocalGitWorkerAdmission, String> {
        service.validate()?;
        let Some(config) = config else {
            return Ok(LocalGitWorkerAdmission {
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
                        .ok_or_else(|| "local Git worker credential is unavailable".to_string())?,
                );
                if credential.is_empty()
                    || credential.len() > 4096
                    || credential.chars().any(char::is_control)
                {
                    return Err("local Git worker credential is invalid".to_string());
                }
                let binding = service
                    .projects
                    .iter()
                    .find(|p| p.key == spec.project)
                    .ok_or_else(|| "local Git worker project is unavailable".to_string())?
                    .clone();
                let admitted =
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
                            return Err(LocalGitWorkerFailure::PreconditionsChanged);
                        }
                        Ok::<_, LocalGitWorkerFailure>(())
                    })
                    .await
                    .map_err(|_| "local Git worker startup check timed out".to_string())?
                    .map_err(|code| format!("local Git worker startup check failed: {code:?}"))?;
                let () = admitted;
                let observer = tokio::time::timeout(
                    Duration::from_millis(spec.operation_timeout_ms),
                    LocalGitAdapter::open(spec.repository.clone()),
                )
                .await
                .map_err(|_| "local Git worker repository check timed out".to_string())?
                .map_err(|code| format!("local Git worker repository check failed: {code:?}"))?;
                let integrator = if spec.integration_enabled {
                    Some(
                        tokio::time::timeout(
                            Duration::from_millis(spec.operation_timeout_ms),
                            LocalGitIntegrator::open(LocalGitIntegrationConfig {
                                enabled: true,
                                repository: spec.repository.clone(),
                            }),
                        )
                        .await
                        .map_err(|_| "local Git worker integrator check timed out".to_string())?
                        .map_err(|code| {
                            format!("local Git worker integrator check failed: {code:?}")
                        })?,
                    )
                } else {
                    None
                };
                let snapshot = Arc::new(Mutex::new(LocalGitWorkerSnapshot {
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
        .map_err(|_| "local Git worker startup checks timed out".to_string())??;
        Ok(LocalGitWorkerAdmission { workers, store })
    }

    pub(super) fn empty() -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            stop,
            tasks: vec![],
            monitor: LocalGitWorkerMonitor { snapshots: vec![] },
        }
    }
    pub fn monitor(&self) -> LocalGitWorkerMonitor {
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

impl LocalGitWorkerAdmission {
    pub(super) fn spawn(self) -> LocalGitWorkerRuntime {
        let (stop, receiver) = watch::channel(false);
        let mut runtime = LocalGitWorkerRuntime {
            stop,
            tasks: vec![],
            monitor: LocalGitWorkerMonitor { snapshots: vec![] },
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
impl Drop for LocalGitWorkerRuntime {
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

fn pg_failure(error: PgError) -> LocalGitWorkerFailure {
    match error {
        PgError::Forbidden
        | PgError::ProjectNotAvailable
        | PgError::Workstream(awr_core::WorkstreamError::AccessDenied) => {
            LocalGitWorkerFailure::AuthorizationUnavailable
        }
        PgError::ClaimHeld
        | PgError::ResourceConflict
        | PgError::StaleFence
        | PgError::LeaseExpired => LocalGitWorkerFailure::Contention,
        PgError::PreconditionsChanged
        | PgError::EpochChanged
        | PgError::CursorExpired
        | PgError::SourceDivergence
        | PgError::InactiveCandidate
        | PgError::RecoveryBlocked
        | PgError::Workstream(_) => LocalGitWorkerFailure::PreconditionsChanged,
        _ => LocalGitWorkerFailure::StoreUnavailable,
    }
}
fn git_failure(error: LocalGitError) -> LocalGitWorkerFailure {
    match error {
        LocalGitError::Contention => LocalGitWorkerFailure::Contention,
        LocalGitError::AuthorizationUnavailable => LocalGitWorkerFailure::AuthorizationUnavailable,
        LocalGitError::BindingMismatch
        | LocalGitError::PreconditionsChanged
        | LocalGitError::DomainRejected => LocalGitWorkerFailure::PreconditionsChanged,
        LocalGitError::RepositoryUnavailable | LocalGitError::InvalidConfiguration => {
            LocalGitWorkerFailure::RepositoryUnavailable
        }
        LocalGitError::ReportUnavailable | LocalGitError::ReportConflict => {
            LocalGitWorkerFailure::ProofUnavailable
        }
        LocalGitError::StoreUnavailable => LocalGitWorkerFailure::StoreUnavailable,
        LocalGitError::TimedOut => LocalGitWorkerFailure::OutcomeUnknown,
        _ => LocalGitWorkerFailure::InvalidQueueResponse,
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
    future: impl Future<Output = Result<T, LocalGitWorkerFailure>>,
) -> Option<Result<T, LocalGitWorkerFailure>> {
    tokio::select! { biased;
        _ = stopped(stop) => None,
        value = tokio::time::timeout(Duration::from_millis(milliseconds), future) => Some(value.unwrap_or(Err(LocalGitWorkerFailure::OutcomeUnknown))),
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
fn page(spec: &LocalGitWorkerSpec, value: Value) -> Result<Page, LocalGitWorkerFailure> {
    let page: Page =
        serde_json::from_value(value).map_err(|_| LocalGitWorkerFailure::InvalidQueueResponse)?;
    if page.connector.connector_id != spec.repository.connector_id
        || page.connector.provider != "local_git"
        || page.connector.resource != spec.repository.resource
        || page.read_set.work_id != spec.repository.work_id
        || page.read_set.workstream_id.to_string() != spec.repository.workstream_id
        || !matches!(
            page.selection_state.as_str(),
            "current" | "missing" | "stale"
        )
        || page.truncated != page.next_cursor.is_some()
    {
        return Err(LocalGitWorkerFailure::PreconditionsChanged);
    }
    Ok(page)
}

async fn original(
    worker: &Worker,
    store: &DeliverySyncStore,
    set: &DeliveryReadSet,
    intent: Intent,
    stop: &mut watch::Receiver<bool>,
) -> Option<Result<(), LocalGitWorkerFailure>> {
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
            return Some(Err(LocalGitWorkerFailure::InvalidQueueResponse));
        }
        let lease = match bounded(stop, milliseconds, async {
            store
                .lease_integration(
                    tenant,
                    project,
                    bearer,
                    LeaseDeliveryIntegration {
                        request_id: format!("git-lease:{}", awr_core::Id::new()),
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
            Err(LocalGitWorkerFailure::Contention) => return Some(Ok(())),
            Err(error) => return Some(Err(error)),
        };
        let Some(lease_id) = lease["data"]["lease_id"].as_str() else {
            return Some(Err(LocalGitWorkerFailure::InvalidQueueResponse));
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
                        request_id: format!("git-dispatch:{lease_id}"),
                        read_set: set.clone(),
                        integration_id: intent.integration_id.clone(),
                        lease_id: lease_id.into(),
                    },
                )
                .await
                .map_err(git_failure)
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
    Some(
        bounded(stop, milliseconds, async {
            let value = integrator
                .reconcile_original_current(store, bearer, &intent.integration_id)
                .await
                .map_err(git_failure)?;
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
        .await?,
    )
}

async fn poll(
    worker: &Worker,
    store: &DeliverySyncStore,
    cursor: &mut Option<String>,
    stop: &mut watch::Receiver<bool>,
) -> Option<Result<(), LocalGitWorkerFailure>> {
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
            return Some(Err(LocalGitWorkerFailure::InvalidQueueResponse));
        }
        if index == 0 {
            if page.selection_state == "current" {
                match bounded(stop, worker.spec.operation_timeout_ms, async {
                    worker
                        .observer
                        .reconcile_current(store, &worker.credential)
                        .await
                        .map_err(git_failure)
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
                    Err(LocalGitWorkerFailure::AuthorizationUnavailable) => {
                        return Some(Err(LocalGitWorkerFailure::AuthorizationUnavailable));
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
                if error == LocalGitWorkerFailure::AuthorizationUnavailable {
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
