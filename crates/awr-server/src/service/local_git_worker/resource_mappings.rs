//! Opt-in scheduling; task bindings never replace the configured repository.
use super::*;
use awr_team_pg::DeliveryWorkerMappingsQuery;
use std::collections::VecDeque;

#[derive(Deserialize)]
struct Mapping {
    connector_id: String,
    connector_version: String,
    work_id: String,
}
#[derive(Deserialize)]
pub(super) struct Mappings {
    protocol: String,
    workstream_id: String,
    provider: String,
    resource: String,
    items: Vec<Mapping>,
    next_cursor: Option<String>,
}

#[derive(Default)]
pub(super) struct State {
    cursor: Option<String>,
    queue: VecDeque<Mapping>,
    current: Option<Worker>,
    schedule_cursor: Option<String>,
}

pub(super) async fn discover(
    spec: &LocalGitWorkerSpec,
    binding: &ProjectBinding,
    credential: &str,
    store: &DeliverySyncStore,
    cursor: Option<String>,
) -> Result<Mappings, LocalGitWorkerFailure> {
    let result = store
        .worker_mappings(
            &binding.tenant_id,
            &binding.project_id,
            credential,
            DeliveryWorkerMappingsQuery {
                workstream_id: spec
                    .repository
                    .workstream_id
                    .parse()
                    .map_err(|_| LocalGitWorkerFailure::InvalidQueueResponse)?,
                provider: "local_git".into(),
                resource: spec.repository.resource.clone(),
                require_integration: spec.integration_enabled,
                cursor,
                limit: i32::from(spec.page_size.min(8)),
            },
        )
        .await
        .map_err(pg_failure)?;
    let page: Mappings =
        serde_json::from_value(result).map_err(|_| LocalGitWorkerFailure::InvalidQueueResponse)?;
    if page.protocol != "awr-delivery-worker-mappings-v1"
        || page.workstream_id != spec.repository.workstream_id
        || page.provider != "local_git"
        || page.resource != spec.repository.resource
        || page.items.len() > usize::from(spec.page_size.min(8))
    {
        return Err(LocalGitWorkerFailure::InvalidQueueResponse);
    }
    Ok(page)
}

async fn bind(
    worker: &Worker,
    store: &DeliverySyncStore,
    mapping: Mapping,
) -> Result<Worker, LocalGitWorkerFailure> {
    let mut spec = worker.spec.clone();
    spec.repository.work_id = mapping.work_id;
    spec.repository.connector_id = mapping.connector_id;
    let schedule = store
        .schedule(
            &worker.binding.tenant_id,
            &worker.binding.project_id,
            &worker.credential,
            DeliveryScheduleQuery {
                work_id: spec.repository.work_id.clone(),
                connector_id: spec.repository.connector_id.clone(),
                cursor: None,
                limit: 1,
            },
        )
        .await
        .map_err(pg_failure)?;
    let current = page(&spec, schedule)?;
    if current.connector.connector_version != mapping.connector_version {
        return Err(LocalGitWorkerFailure::PreconditionsChanged);
    }
    let access = store
        .check_worker_access(
            &worker.binding.tenant_id,
            &worker.binding.project_id,
            &worker.credential,
            &current.read_set,
            &spec.repository.connector_id,
            spec.integration_enabled,
        )
        .await
        .map_err(pg_failure)?;
    if access["connector_version"] != current.connector.connector_version
        || access["resource"] != spec.repository.resource
    {
        return Err(LocalGitWorkerFailure::PreconditionsChanged);
    }
    let observer = LocalGitAdapter::open(spec.repository.clone())
        .await
        .map_err(git_failure)?;
    let integrator = if spec.integration_enabled {
        Some(
            LocalGitIntegrator::open(LocalGitIntegrationConfig {
                enabled: true,
                repository: spec.repository.clone(),
            })
            .await
            .map_err(git_failure)?,
        )
    } else {
        None
    };
    Ok(Worker {
        spec,
        binding: worker.binding.clone(),
        credential: Zeroizing::new(worker.credential.to_string()),
        observer,
        integrator,
        snapshot: worker.snapshot.clone(),
    })
}

pub(super) async fn poll(
    worker: &Worker,
    store: &DeliverySyncStore,
    state: &mut State,
    stop: &mut watch::Receiver<bool>,
) -> Option<Result<(), LocalGitWorkerFailure>> {
    let before = worker
        .snapshot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .visited_intents;
    let mut fetched = false;
    let mut failure = None;
    for _ in 0..worker.spec.page_size.min(8) {
        let used = worker
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .visited_intents
            .saturating_sub(before);
        let remaining = worker
            .spec
            .max_jobs_per_poll
            .saturating_sub(used.min(u64::from(u16::MAX)) as u16);
        if remaining == 0 {
            break;
        }
        if state.current.is_none() {
            if state.queue.is_empty() {
                if fetched {
                    break;
                }
                fetched = true;
                observe(&worker.snapshot, |s| s.polls = s.polls.saturating_add(1));
                let result = bounded(
                    stop,
                    worker.spec.operation_timeout_ms,
                    discover(
                        &worker.spec,
                        &worker.binding,
                        &worker.credential,
                        store,
                        state.cursor.clone(),
                    ),
                )
                .await?;
                match result {
                    Ok(page) => {
                        state.cursor = page.next_cursor;
                        state.queue = page.items.into();
                    }
                    Err(error) => {
                        *state = State::default();
                        return Some(Err(error));
                    }
                }
            }
            let Some(mapping) = state.queue.pop_front() else {
                break;
            };
            match bounded(
                stop,
                worker.spec.operation_timeout_ms,
                bind(worker, store, mapping),
            )
            .await?
            {
                Ok(child) => {
                    state.current = Some(child);
                    state.schedule_cursor = None;
                }
                Err(error) => {
                    failure = Some(error);
                    continue;
                }
            }
        }
        let child = state.current.as_mut().unwrap();
        child.spec.max_jobs_per_poll = remaining;
        let result = super::poll(child, store, &mut state.schedule_cursor, stop).await?;
        if let Err(error) = result {
            failure = Some(error);
            state.current = None;
            state.schedule_cursor = None;
        } else if state.schedule_cursor.is_none() {
            state.current = None;
        } else {
            break;
        }
    }
    Some(failure.map_or(Ok(()), Err))
}
