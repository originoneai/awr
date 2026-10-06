//! Bounded server-owned scheduling of the durable PG pump. No repository effects or approval.
use super::{ProjectBinding, ServiceConfig};
use crate::config::{DeliveryWorkerConfig, DeliveryWorkerSpec};
use awr_core::Id;
use awr_team_pg::{ClaimDeliverySyncIntent, DeliveryReadSet, DeliverySyncStore, PgError, PgResult};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, task::JoinHandle};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Starting,
    Running,
    Backoff,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerFailure {
    AuthorizationUnavailable,
    PreconditionsChanged,
    Contention,
    DatabaseUnavailable,
    InvalidQueueResponse,
    OutcomeUnknown,
    SourceConflict,
    SourceUnavailable,
    SourceFailed,
}

/// Operator observations, not an acceptance, integration or execution receipt.
#[derive(Clone, Debug, Serialize)]
pub struct DeliveryWorkerSnapshot {
    pub project: String,
    pub worker_id: String,
    pub state: WorkerState,
    pub failure_code: Option<WorkerFailure>,
    pub polls: u64,
    pub observed_intents: u64,
    pub attempts: u64,
    pub synchronized: u64,
    pub deferred: u64,
    pub contention: u64,
    pub live_leases_observed: u64,
    pub outdated_bindings_observed: u64,
    pub failures: u64,
    pub retry_delay_ms: u64,
    pub observed_at_unix_ms: u64,
}

type Snapshot = Arc<Mutex<DeliveryWorkerSnapshot>>;

#[derive(Clone)]
pub struct DeliveryWorkerMonitor {
    snapshots: Vec<Snapshot>,
}

impl DeliveryWorkerMonitor {
    pub fn snapshots(&self) -> Vec<DeliveryWorkerSnapshot> {
        self.snapshots
            .iter()
            .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .collect()
    }
}

fn observe(snapshot: &Snapshot, update: impl FnOnce(&mut DeliveryWorkerSnapshot)) {
    let mut value = snapshot.lock().unwrap_or_else(|e| e.into_inner());
    let before = (value.state, value.failure_code);
    update(&mut value);
    value.observed_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    if before != (value.state, value.failure_code) {
        // Only configured identifiers and finite codes. Never format a PG/provider error.
        eprintln!(
            "{}",
            serde_json::json!({"service":"awr-delivery-worker","project":value.project,
                "worker_id":value.worker_id,"state":value.state,"failure_code":value.failure_code,
                "observed_at_unix_ms":value.observed_at_unix_ms})
        );
    }
}

struct Worker {
    spec: DeliveryWorkerSpec,
    binding: ProjectBinding,
    credential: Zeroizing<String>,
    snapshot: Snapshot,
}

pub struct DeliveryWorkerRuntime {
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    monitor: DeliveryWorkerMonitor,
}

impl DeliveryWorkerRuntime {
    /// Validate every configured worker before starting any. No implicit identity or grant.
    pub async fn start(
        service: &ServiceConfig,
        config: Option<DeliveryWorkerConfig>,
        store: DeliverySyncStore,
        mut credential_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, String> {
        service.validate()?;
        let (stop, receiver) = watch::channel(false);
        let mut runtime = Self {
            stop,
            tasks: Vec::new(),
            monitor: DeliveryWorkerMonitor { snapshots: vec![] },
        };
        let Some(config) = config else {
            return Ok(runtime);
        };
        config.validate(service)?;
        let mut workers = Vec::new();
        for spec in config.workers {
            let credential = Zeroizing::new(
                credential_lookup(&spec.credential_env)
                    .ok_or_else(|| "delivery worker credential is unavailable".to_string())?,
            );
            if credential.is_empty()
                || credential.len() > 4096
                || credential.chars().any(char::is_control)
            {
                return Err("delivery worker credential is invalid".into());
            }
            let binding = service
                .projects
                .iter()
                .find(|p| p.key == spec.project)
                .ok_or_else(|| "delivery worker project is unavailable".to_string())?
                .clone();
            let snapshot = Arc::new(Mutex::new(DeliveryWorkerSnapshot {
                project: spec.project.clone(),
                worker_id: spec.worker_id.clone(),
                state: WorkerState::Starting,
                failure_code: None,
                polls: 0,
                observed_intents: 0,
                attempts: 0,
                synchronized: 0,
                deferred: 0,
                contention: 0,
                live_leases_observed: 0,
                outdated_bindings_observed: 0,
                failures: 0,
                retry_delay_ms: 0,
                observed_at_unix_ms: 0,
            }));
            workers.push(Worker {
                spec,
                binding,
                credential,
                snapshot,
            });
        }
        // Cap the whole admission sequence as well as each authenticated read.
        tokio::time::timeout(Duration::from_secs(30), async {
            for worker in &workers {
                for &stream in &worker.spec.workstreams {
                    tokio::time::timeout(
                        Duration::from_millis(worker.spec.operation_timeout_ms),
                        store.sync_intents(
                            &worker.binding.tenant_id,
                            &worker.binding.project_id,
                            &worker.credential,
                            stream,
                            1,
                            None,
                        ),
                    )
                    .await
                    .map_err(|_| "delivery worker startup check timed out".to_string())?
                    .map_err(|error| {
                        format!(
                            "delivery worker startup check failed: {:?}",
                            classify(&error)
                        )
                    })?;
                }
            }
            Ok::<_, String>(())
        })
        .await
        .map_err(|_| "delivery worker startup checks timed out".to_string())??;
        let store = Arc::new(store);
        for worker in workers {
            runtime.monitor.snapshots.push(worker.snapshot.clone());
            // Capture the observation before spawning, including cancellation before the
            // first poll. Constructing it inside an async fn would miss that case.
            let observation = StopObservation(worker.snapshot.clone());
            let store = store.clone();
            let receiver = receiver.clone();
            runtime.tasks.push(tokio::spawn(async move {
                let _on_stop = observation;
                run_worker(worker, store, receiver).await;
            }));
        }
        Ok(runtime)
    }

    pub fn monitor(&self) -> DeliveryWorkerMonitor {
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

impl Drop for DeliveryWorkerRuntime {
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
    future: impl Future<Output = PgResult<T>>,
) -> Option<Result<T, WorkerFailure>> {
    tokio::select! {
        biased;
        _ = stopped(stop) => None,
        result = tokio::time::timeout(Duration::from_millis(milliseconds), future) => Some(
            result.map_err(|_| WorkerFailure::OutcomeUnknown)
                .and_then(|result| result.map_err(|error| classify(&error)))),
    }
}

fn classify(error: &PgError) -> WorkerFailure {
    match error {
        PgError::Forbidden
        | PgError::ProjectNotAvailable
        | PgError::Workstream(awr_core::WorkstreamError::AccessDenied) => {
            WorkerFailure::AuthorizationUnavailable
        }
        PgError::StaleFence | PgError::ResourceConflict | PgError::LeaseExpired => {
            WorkerFailure::Contention
        }
        PgError::PreconditionsChanged
        | PgError::EpochChanged
        | PgError::SourceDivergence
        | PgError::InactiveCandidate
        | PgError::SchemaIncompatible(_)
        | PgError::Workstream(_) => WorkerFailure::PreconditionsChanged,
        _ => WorkerFailure::DatabaseUnavailable,
    }
}

#[derive(Deserialize)]
struct Page {
    intents: Vec<Intent>,
    has_more: bool,
    next_after: Option<String>,
}

#[derive(Deserialize)]
struct Intent {
    intent_id: String,
    state: String,
    fence: String,
    read_set: DeliveryReadSet,
    lease_live: bool,
    retry_due: bool,
    binding_current: bool,
}

impl Intent {
    fn eligible(&self) -> bool {
        self.retry_due
            && match self.state.as_str() {
                "pending" | "blocked" => true,
                "leased" => !self.lease_live,
                _ => false,
            }
    }
}

fn source_failure(value: &serde_json::Value) -> Option<(&'static str, WorkerFailure)> {
    match value["data"]["phase"].as_str() {
        Some("conflict") => Some(("source_conflict", WorkerFailure::SourceConflict)),
        Some("failed") if value["data"]["failure_code"] == "source_unavailable" => {
            Some(("source_unavailable", WorkerFailure::SourceUnavailable))
        }
        Some("failed") => Some(("source_failed", WorkerFailure::SourceFailed)),
        _ => None,
    }
}

async fn poll(
    worker: &Worker,
    store: &DeliverySyncStore,
    stream: Id,
    cursor: &mut Option<String>,
    stop: &mut watch::Receiver<bool>,
    delay: u64,
) -> Option<Result<(), WorkerFailure>> {
    let (tenant, project, bearer) = (
        worker.binding.tenant_id.as_str(),
        worker.binding.project_id.as_str(),
        worker.credential.as_str(),
    );
    let limits = &worker.spec;
    let mut jobs = 0;
    let mut failure = None;
    observe(&worker.snapshot, |s| s.polls = s.polls.saturating_add(1));
    for _ in 0..limits.max_pages_per_poll {
        let page = match bounded(
            stop,
            limits.operation_timeout_ms,
            store.sync_intents(
                tenant,
                project,
                bearer,
                stream,
                limits.page_size,
                cursor.as_deref(),
            ),
        )
        .await?
        {
            Ok(value) => match serde_json::from_value::<Page>(value) {
                Ok(page) => page,
                Err(_) => return Some(Err(WorkerFailure::InvalidQueueResponse)),
            },
            Err(error) => {
                // A source activation can retire a cursor. Re-read current scope on the next
                // poll, still using the same authenticated principal; never change its grants.
                *cursor = None;
                return Some(Err(error));
            }
        };
        let mut exhausted = true;
        for intent in page.intents {
            if jobs >= limits.max_jobs_per_poll {
                exhausted = false;
                break;
            }
            *cursor = Some(intent.intent_id.clone());
            observe(&worker.snapshot, |s| {
                s.observed_intents = s.observed_intents.saturating_add(1);
                if intent.state == "leased" && intent.lease_live {
                    s.live_leases_observed = s.live_leases_observed.saturating_add(1);
                }
                if !intent.binding_current {
                    s.outdated_bindings_observed = s.outdated_bindings_observed.saturating_add(1);
                }
            });
            if !intent.eligible() {
                continue;
            }
            jobs += 1;
            observe(&worker.snapshot, |s| {
                s.attempts = s.attempts.saturating_add(1)
            });
            let lease = match bounded(
                stop,
                limits.operation_timeout_ms,
                store.claim_sync_intent(
                    tenant,
                    project,
                    bearer,
                    ClaimDeliverySyncIntent {
                        request_id: format!("worker-{}", Id::new()),
                        read_set: intent.read_set,
                        intent_id: intent.intent_id,
                        worker_id: limits.worker_id.clone(),
                        expected_fence: intent.fence,
                        lease_seconds: limits.lease_seconds,
                    },
                ),
            )
            .await?
            {
                Ok(Some(lease)) => lease,
                Ok(None) => continue, // Superseded observation, not successful publication.
                Err(WorkerFailure::Contention) => {
                    observe(&worker.snapshot, |s| {
                        s.contention = s.contention.saturating_add(1)
                    });
                    continue;
                }
                Err(error) => {
                    // Losing a mutation response is unknown; inspect the durable queue next
                    // time. A fresh request cannot reclaim a committed live lease.
                    failure = Some(if error == WorkerFailure::DatabaseUnavailable {
                        WorkerFailure::OutcomeUnknown
                    } else {
                        error
                    });
                    if error == WorkerFailure::AuthorizationUnavailable {
                        return Some(Err(error));
                    }
                    continue;
                }
            };
            match bounded(
                stop,
                limits.operation_timeout_ms,
                store.process_sync_intent(tenant, project, bearer, &lease, limits.lease_seconds),
            )
            .await?
            {
                Ok(value)
                    if value["data"]["phase"] == "confirmed"
                        || value["data"]["refresh_available"] == true =>
                {
                    observe(&worker.snapshot, |s| {
                        s.synchronized = s.synchronized.saturating_add(1);
                        s.failure_code = None;
                    });
                }
                Ok(value) => {
                    if let Some((code, error)) = source_failure(&value) {
                        failure = Some(error);
                        let retry = delay.div_ceil(1000).clamp(1, 3600) as i32;
                        match bounded(
                            stop,
                            limits.operation_timeout_ms,
                            store.defer_sync_intent(tenant, project, bearer, &lease, code, retry),
                        )
                        .await?
                        {
                            Ok(_) => observe(&worker.snapshot, |s| {
                                s.deferred = s.deferred.saturating_add(1)
                            }),
                            Err(error) => failure = Some(error),
                        }
                    } else {
                        failure = Some(WorkerFailure::OutcomeUnknown);
                    }
                }
                Err(WorkerFailure::Contention) => {
                    observe(&worker.snapshot, |s| {
                        s.contention = s.contention.saturating_add(1)
                    });
                }
                Err(WorkerFailure::PreconditionsChanged) => {
                    // A known changed prerequisite can be durably deferred. This does not
                    // discard an associated publication or settle an unknown file effect.
                    failure = Some(WorkerFailure::PreconditionsChanged);
                    match bounded(
                        stop,
                        limits.operation_timeout_ms,
                        store.defer_sync_intent(
                            tenant,
                            project,
                            bearer,
                            &lease,
                            "preconditions_changed",
                            delay.div_ceil(1000).clamp(1, 3600) as i32,
                        ),
                    )
                    .await?
                    {
                        Ok(_) => observe(&worker.snapshot, |s| {
                            s.deferred = s.deferred.saturating_add(1)
                        }),
                        Err(error) => failure = Some(error),
                    }
                }
                Err(error) => {
                    // A source effect may precede a database failure. No failure acknowledgement
                    // or blind effect retry: keep its journal and await a new fenced recovery.
                    failure = Some(if error == WorkerFailure::DatabaseUnavailable {
                        WorkerFailure::OutcomeUnknown
                    } else {
                        error
                    });
                    if error == WorkerFailure::AuthorizationUnavailable {
                        return Some(Err(error));
                    }
                }
            }
        }
        if !exhausted {
            break; // Resume after the last visited item, not after unprocessed page contents.
        }
        *cursor = page.next_after;
        if !page.has_more {
            break;
        }
    }
    Some(failure.map_or(Ok(()), Err))
}

async fn run_worker(
    worker: Worker,
    store: Arc<DeliverySyncStore>,
    mut stop: watch::Receiver<bool>,
) {
    let mut cursors = vec![None; worker.spec.workstreams.len()];
    let mut scope = 0;
    let mut delay = worker.spec.poll_interval_ms;
    observe(&worker.snapshot, |s| s.state = WorkerState::Running);
    loop {
        let Some(result) = poll(
            &worker,
            &store,
            worker.spec.workstreams[scope],
            &mut cursors[scope],
            &mut stop,
            delay,
        )
        .await
        else {
            return;
        };
        scope = (scope + 1) % worker.spec.workstreams.len();
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
        tokio::select! {
            biased;
            _ = stopped(&mut stop) => return,
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
        }
    }
}
