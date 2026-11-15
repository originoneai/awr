//! Real usage ingestion and measured-time observation APIs (WS-041).
//!
//! Thin runtime façade over `awr_core::workstream_usage` plus store persistence.
//! Adapters authenticate the project and supply provenance-checked receipts /
//! intervals. This module:
//! - binds deduped receipts to execution / task / occurrence-time mainline;
//! - keeps actual cost, API-equivalent estimate, unknown, and coverage separate;
//! - refuses compression/model double-counting via call-identity dedup;
//! - keeps cumulative→incremental, cross-stream allocations, and corrections auditable;
//! - treats parallel wall-clock as distinct from summed execution duration;
//! - hands measured observations with coverage to WS-043 without inventing ETA.
use awr_core::workstream_usage::{
    self, UsageAllocationRecord, UsageCorrection, UsageCostTotals, UsageCounterSnapshot,
    UsageCoverageObservation, UsageError, UsageExecutionInterval, UsageOccurrenceBinding,
    UsageReceipt, UsageTimeObservationHandoff, UsageTimeTotals,
};
use awr_core::{Error, Id, Revision};
pub use awr_store::UsageRecord;
use awr_store::{Store, UsageIngestReceipt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UsageRuntimeError {
    #[error("project attestation missing for usage operation")]
    ProjectNotAttested,
    #[error("approved project does not match receipt project")]
    ProjectMismatch,
    #[error(transparent)]
    Usage(#[from] UsageError),
    #[error(transparent)]
    Store(#[from] Error),
}

/// Adapter attestation that the project was authorized for usage ingest/query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestedUsageProject {
    pub project_id: String,
    pub project: Id,
    pub attested: bool,
}

impl AttestedUsageProject {
    /// Shared JSON boundary used by the project-bound CLI and MCP adapters.
    pub fn record(
        &self,
        store: &mut Store,
        input: Value,
    ) -> std::result::Result<Value, UsageRuntimeError> {
        awr_core::ensure_public_value(&input)?;
        let request = serde_json::from_value(input).map_err(|_| {
            Error::InvalidInput("usage record does not match the published schema".into())
        })?;
        record_usage(store, self, &request)
    }

    pub fn query(
        &self,
        store: &Store,
        input: Value,
    ) -> std::result::Result<Value, UsageRuntimeError> {
        awr_core::ensure_public_value(&input)?;
        let query = serde_json::from_value(input).map_err(|_| {
            Error::InvalidInput("usage query does not match the published schema".into())
        })?;
        query_usage(store, self, &query)
    }
}

/// CLI and MCP use the same input and immutable record codec.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordUsageRequest {
    pub expected_revision: Revision,
    pub request_key: String,
    pub record: UsageRecord,
}

fn page_limit() -> usize {
    20
}

fn corrected() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum UsageQuery {
    Receipts {
        #[serde(default)]
        offset: usize,
        #[serde(default = "page_limit")]
        limit: usize,
        #[serde(default = "corrected")]
        apply_corrections: bool,
    },
    Corrections {
        #[serde(default)]
        offset: usize,
        #[serde(default = "page_limit")]
        limit: usize,
    },
    Allocations {
        #[serde(default)]
        offset: usize,
        #[serde(default = "page_limit")]
        limit: usize,
    },
    Counters {
        #[serde(default)]
        offset: usize,
        #[serde(default = "page_limit")]
        limit: usize,
    },
    Intervals {
        #[serde(default)]
        offset: usize,
        #[serde(default = "page_limit")]
        limit: usize,
    },
    Bindings {
        #[serde(default)]
        offset: usize,
        #[serde(default = "page_limit")]
        limit: usize,
    },
    Summary {
        #[serde(default = "corrected")]
        apply_corrections: bool,
    },
    Observation {
        expected_calls: Option<usize>,
        expected_time_ms: Option<u64>,
    },
}

/// The adapter supplies authorization from its bound project, never a caller boolean.
fn record_usage(
    store: &mut Store,
    project: &AttestedUsageProject,
    request: &RecordUsageRequest,
) -> std::result::Result<Value, UsageRuntimeError> {
    assert_attested(project)?;
    awr_core::ensure_public_value(&serde_json::to_value(request).map_err(Error::from)?)?;
    let (record, receipt, event) = store.record_usage_at_revision(
        project.project,
        request.expected_revision,
        &request.request_key,
        &request.record,
    )?;
    Ok(
        json!({"ok":true,"protocol":"awr-usage-ledger-v1","project_id":project.project_id,
        "project_revision":store.project(project.project)?.project_revision,
        "record":record,"receipt":{"id":receipt.event_id,"request_key":receipt.request_key,
            "subject_id":receipt.subject_id,"operation":receipt.op,"replayed":receipt.replayed,
            "recorded_at_ms":receipt.created_at_ms},
        "event":event.map(|event| json!({"id":event.id,"event_type":event.event_type,
            "project_revision":event.project_revision})),"provenance":"caller_supplied"}),
    )
}

fn usage_page<T: Serialize>(items: Vec<T>, offset: usize, limit: usize) -> awr_core::Result<Value> {
    if !(1..=100).contains(&limit) {
        return Err(Error::InvalidInput(
            "usage page limit must be 1..100".into(),
        ));
    }
    let total = items.len();
    let end = offset.saturating_add(limit).min(total);
    let page: Vec<_> = items.into_iter().skip(offset).take(limit).collect();
    Ok(json!({"items":page,"total":total,"offset":offset,
        "next_offset":(end < total).then_some(end)}))
}

/// Read one coherent caller-selected store snapshot. The adapters bound its lifetime.
fn query_usage(
    store: &Store,
    project: &AttestedUsageProject,
    query: &UsageQuery,
) -> std::result::Result<Value, UsageRuntimeError> {
    assert_attested(project)?;
    let id = project.project;
    let data = match *query {
        UsageQuery::Receipts {
            offset,
            limit,
            apply_corrections,
        } => {
            let records = if apply_corrections {
                store.usage_receipts_corrected(id)?
            } else {
                store.list_usage_receipts(id)?
            };
            let mut page = usage_page(records, offset, limit)?;
            page["corrections_applied"] = json!(apply_corrections);
            page
        }
        UsageQuery::Corrections { offset, limit } => {
            usage_page(store.list_usage_corrections(id)?, offset, limit)?
        }
        UsageQuery::Allocations { offset, limit } => {
            usage_page(store.list_usage_allocations(id)?, offset, limit)?
        }
        UsageQuery::Counters { offset, limit } => {
            usage_page(store.list_usage_counter_snapshots(id)?, offset, limit)?
        }
        UsageQuery::Intervals { offset, limit } => {
            usage_page(store.list_usage_execution_intervals(id)?, offset, limit)?
        }
        UsageQuery::Bindings { offset, limit } => {
            usage_page(store.usage_occurrence_bindings(id)?, offset, limit)?
        }
        UsageQuery::Summary { apply_corrections } => {
            let costs = store.usage_cost_totals(id, apply_corrections)?;
            let time = store.usage_time_totals(id)?;
            let corrections = if apply_corrections {
                store.list_usage_corrections(id)?
            } else {
                Vec::new()
            };
            let correction_ids: Vec<_> = corrections
                .iter()
                .take(20)
                .map(|row| &row.correction_id)
                .collect();
            let coverage = workstream_usage::usage_coverage_report(&UsageCoverageObservation {
                observed_calls: costs.unique_calls,
                expected_calls: None,
                observed_time_ms: time.map(|value| value.observed_wall_clock_ms).unwrap_or(0),
                expected_time_ms: None,
            })?;
            json!({"costs":costs,"time":time,"coverage":coverage,
                "corrections_applied":apply_corrections,
                "corrections":{"ids":correction_ids,"total":corrections.len(),
                    "next_query":(corrections.len()>20).then_some(json!({"kind":"corrections","offset":20,"limit":20}))},
                "missing":{"time":time.is_none().then_some("no_recorded_intervals"),
                    "expected_calls":"not_reported","expected_time_ms":"not_reported"}})
        }
        UsageQuery::Observation {
            expected_calls,
            expected_time_ms,
        } => {
            let costs = store.usage_cost_totals(id, true)?;
            let time = store.usage_time_totals(id)?;
            let coverage = UsageCoverageObservation {
                observed_calls: costs.unique_calls,
                expected_calls,
                observed_time_ms: time.map(|value| value.observed_wall_clock_ms).unwrap_or(0),
                expected_time_ms,
            };
            serde_json::to_value(usage_observation_for_ws043(store, project, &coverage)?)
                .map_err(Error::from)?
        }
    };
    Ok(
        json!({"ok":true,"protocol":"awr-usage-ledger-v1","project_id":project.project_id,
        "project_revision":store.project(id)?.project_revision,"read_only":true,
        "source_refresh_performed":false,"freshness_basis":"runtime_usage_ledger",
        "provenance":"caller_supplied","data":data}),
    )
}

fn assert_attested(project: &AttestedUsageProject) -> std::result::Result<(), UsageRuntimeError> {
    if !project.attested || project.project_id.trim().is_empty() {
        return Err(UsageRuntimeError::ProjectNotAttested);
    }
    if project.project.to_string() != project.project_id {
        return Err(UsageRuntimeError::ProjectMismatch);
    }
    Ok(())
}

pub fn ingest_usage_receipt(
    store: &mut Store,
    project: &AttestedUsageProject,
    request_key: &str,
    receipt: &UsageReceipt,
) -> std::result::Result<(UsageReceipt, UsageIngestReceipt), UsageRuntimeError> {
    assert_attested(project)?;
    if receipt.project_id != project.project_id {
        return Err(UsageRuntimeError::ProjectMismatch);
    }
    Ok(store.ingest_usage_receipt(project.project, request_key, receipt)?)
}

pub fn record_usage_correction(
    store: &mut Store,
    project: &AttestedUsageProject,
    request_key: &str,
    correction: &UsageCorrection,
) -> std::result::Result<(UsageCorrection, UsageIngestReceipt), UsageRuntimeError> {
    assert_attested(project)?;
    if correction.project_id != project.project_id {
        return Err(UsageRuntimeError::ProjectMismatch);
    }
    Ok(store.record_usage_correction(project.project, request_key, correction)?)
}

pub fn record_usage_allocation(
    store: &mut Store,
    project: &AttestedUsageProject,
    request_key: &str,
    record: &UsageAllocationRecord,
) -> std::result::Result<(UsageAllocationRecord, UsageIngestReceipt), UsageRuntimeError> {
    assert_attested(project)?;
    if record.project_id != project.project_id {
        return Err(UsageRuntimeError::ProjectMismatch);
    }
    Ok(store.record_usage_allocation(project.project, request_key, record)?)
}

pub fn record_usage_counter_snapshot(
    store: &mut Store,
    project: &AttestedUsageProject,
    request_key: &str,
    snapshot: &UsageCounterSnapshot,
) -> std::result::Result<(UsageCounterSnapshot, UsageIngestReceipt), UsageRuntimeError> {
    assert_attested(project)?;
    if snapshot.scope.project_id != project.project_id {
        return Err(UsageRuntimeError::ProjectMismatch);
    }
    Ok(store.record_usage_counter_snapshot(project.project, request_key, snapshot)?)
}

pub fn record_usage_execution_interval(
    store: &mut Store,
    project: &AttestedUsageProject,
    request_key: &str,
    interval: &UsageExecutionInterval,
) -> std::result::Result<(UsageExecutionInterval, UsageIngestReceipt), UsageRuntimeError> {
    assert_attested(project)?;
    Ok(store.record_usage_execution_interval(project.project, request_key, interval)?)
}

pub fn query_usage_occurrence_bindings(
    store: &Store,
    project: &AttestedUsageProject,
) -> std::result::Result<Vec<UsageOccurrenceBinding>, UsageRuntimeError> {
    assert_attested(project)?;
    Ok(store.usage_occurrence_bindings(project.project)?)
}

pub fn query_usage_cost_totals(
    store: &Store,
    project: &AttestedUsageProject,
    apply_corrections: bool,
) -> std::result::Result<UsageCostTotals, UsageRuntimeError> {
    assert_attested(project)?;
    Ok(store.usage_cost_totals(project.project, apply_corrections)?)
}

pub fn query_usage_time_totals(
    store: &Store,
    project: &AttestedUsageProject,
) -> std::result::Result<Option<UsageTimeTotals>, UsageRuntimeError> {
    assert_attested(project)?;
    Ok(store.usage_time_totals(project.project)?)
}

/// Build the WS-043 historical observation handoff with coverage.
pub fn usage_observation_for_ws043(
    store: &Store,
    project: &AttestedUsageProject,
    coverage: &UsageCoverageObservation,
) -> std::result::Result<UsageTimeObservationHandoff, UsageRuntimeError> {
    assert_attested(project)?;
    let handoff = store.usage_observation_handoff(project.project, coverage)?;
    // Belt-and-suspenders: refuse any attempt to relabel cumulative duration as ETA.
    workstream_usage::refuse_eta_from_cumulative_duration(
        "historical observation handoff",
        handoff
            .time_totals
            .map(|t| t.observed_execution_ms)
            .unwrap_or(0),
    )?;
    Ok(handoff)
}

/// Explicit refusal API for adapters that try to display cumulative duration as ETA.
pub fn refuse_eta_from_cumulative_duration(
    label: &str,
    cumulative_duration_ms: u64,
) -> std::result::Result<(), UsageRuntimeError> {
    Ok(workstream_usage::refuse_eta_from_cumulative_duration(
        label,
        cumulative_duration_ms,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::Error;
    use awr_core::workstream_usage::*;
    use awr_store::{SourceRegistration, Store};

    fn money(n: u64) -> UsageMoney {
        UsageMoney {
            currency: "USD".into(),
            micros: n,
        }
    }

    fn fixture() -> (Store, AttestedUsageProject) {
        let root = std::env::temp_dir().join(format!("awr-usage-rt-{}", Id::new()));
        std::fs::create_dir(&root).unwrap();
        let mut store = Store::open(&root.join("state.db")).unwrap();
        let project = store
            .register_project(&root, "usage-example", "Usage Example")
            .unwrap();
        let _ = store
            .register_source(
                project.id,
                &SourceRegistration {
                    domain: "ledger",
                    role: "primary",
                    locator: "file:///example/ledger.yaml",
                    format: "yaml",
                    adapter: "yaml-ledger-v1",
                },
            )
            .unwrap();
        let attested = AttestedUsageProject {
            project_id: project.id.to_string(),
            project: project.id,
            attested: true,
        };
        (store, attested)
    }

    fn receipt(project_id: &str, receipt_id: &str, call_id: &str) -> UsageReceipt {
        UsageReceipt {
            receipt_id: receipt_id.into(),
            project_id: project_id.into(),
            provider_namespace: "account".into(),
            provider: "provider".into(),
            call_id: call_id.into(),
            model: "model".into(),
            session_id: "session".into(),
            occurred_at_ms: 10,
            channel: UsageChannel::Model,
            attribution: UsageAttribution {
                work_id: "task-1".into(),
                execution_id: "exec-1".into(),
                workstream_id: Some(Id::from(7u128)),
            },
            tokens: Some(UsageTokens {
                input: 100,
                output: 20,
                cached_input: 10,
            }),
            cost: UsageCost::Actual(money(101)),
        }
    }

    #[test]
    fn ingest_binds_execution_task_mainline_and_keeps_cost_columns_separate() {
        let (mut store, project) = fixture();
        let model = receipt(&project.project_id, "r1", "call-1");
        let mut compaction = model.clone();
        compaction.receipt_id = "r1-compaction".into();
        compaction.channel = UsageChannel::Compaction;
        ingest_usage_receipt(&mut store, &project, "req-1", &model).unwrap();
        // Same call via compaction observation: stored under first identity / no double bill.
        let (stored, _) =
            ingest_usage_receipt(&mut store, &project, "req-1b", &compaction).unwrap();
        assert_eq!(stored.receipt_id, "r1");
        let mut estimate = receipt(&project.project_id, "r2", "call-2");
        estimate.cost = UsageCost::ApiEquivalentEstimate {
            amount: money(500),
            pricing_version: "price-v1".into(),
        };
        let mut unknown = receipt(&project.project_id, "r3", "call-3");
        unknown.cost = UsageCost::Unknown;
        ingest_usage_receipt(&mut store, &project, "req-2", &estimate).unwrap();
        ingest_usage_receipt(&mut store, &project, "req-3", &unknown).unwrap();
        let totals = query_usage_cost_totals(&store, &project, false).unwrap();
        assert_eq!(totals.actual_micros["USD"], 101);
        assert_eq!(totals.api_equivalent_micros["USD"], 500);
        assert_eq!(totals.unknown_calls, 1);
        assert_eq!(totals.unique_calls, 3);
        let bindings = query_usage_occurrence_bindings(&store, &project).unwrap();
        assert!(bindings.iter().any(|b| {
            b.execution_id == "exec-1"
                && b.work_id == "task-1"
                && b.occurrence_mainline_id == Some(Id::from(7u128))
        }));
    }

    #[test]
    fn corrections_allocations_counters_and_parallel_time_are_auditable() {
        let (mut store, project) = fixture();
        let r = receipt(&project.project_id, "r1", "call-1");
        ingest_usage_receipt(&mut store, &project, "req-1", &r).unwrap();
        let correction = UsageCorrection {
            correction_id: "c1".into(),
            project_id: project.project_id.clone(),
            target_receipt_id: "r1".into(),
            corrected_at_ms: 20,
            reason: "provider restatement".into(),
            prior_cost: UsageCost::Actual(money(101)),
            new_cost: UsageCost::Actual(money(80)),
            actor: "billing".into(),
        };
        record_usage_correction(&mut store, &project, "corr-1", &correction).unwrap();
        let totals = query_usage_cost_totals(&store, &project, true).unwrap();
        assert_eq!(totals.actual_micros["USD"], 80);
        let alloc = UsageAllocationRecord {
            allocation_id: "a1".into(),
            project_id: project.project_id.clone(),
            receipt_id: "r1".into(),
            rule: Some("split-v1".into()),
            shares: vec![
                UsageAllocation {
                    workstream_id: Some(Id::from(7u128)),
                    micros: 50,
                },
                UsageAllocation {
                    workstream_id: Some(Id::from(8u128)),
                    micros: 30,
                },
            ],
            recorded_at_ms: 21,
        };
        let mut stale = alloc.clone();
        stale.allocation_id = "a-stale".into();
        stale.shares[1].micros = 51;
        assert!(matches!(
            record_usage_allocation(&mut store, &project, "alloc-stale", &stale),
            Err(UsageRuntimeError::Store(Error::InvalidInput(_)))
        ));
        record_usage_allocation(&mut store, &project, "alloc-1", &alloc).unwrap();
        let mut second = alloc.clone();
        second.allocation_id = "a2".into();
        assert!(matches!(
            record_usage_allocation(&mut store, &project, "alloc-2", &second),
            Err(UsageRuntimeError::Store(Error::RuleViolation(_)))
        ));

        let snap1 = UsageCounterSnapshot {
            scope: UsageCounterScope {
                project_id: project.project_id.clone(),
                provider_namespace: "account".into(),
                provider: "provider".into(),
                model: "model".into(),
                session_id: "session".into(),
                counter_epoch: "epoch".into(),
            },
            observed_at_ms: 1,
            tokens: UsageTokens {
                input: 10,
                output: 2,
                cached_input: 1,
            },
        };
        let mut snap2 = snap1.clone();
        snap2.observed_at_ms = 2;
        snap2.tokens.input = 15;
        snap2.tokens.output = 4;
        snap2.tokens.cached_input = 2;
        record_usage_counter_snapshot(&mut store, &project, "ctr-1", &snap1).unwrap();
        record_usage_counter_snapshot(&mut store, &project, "ctr-2", &snap2).unwrap();

        record_usage_execution_interval(
            &mut store,
            &project,
            "iv-a",
            &UsageExecutionInterval {
                execution_id: "exec-a".into(),
                interval: UsageTimeInterval {
                    start_ms: 0,
                    end_ms: 10,
                },
            },
        )
        .unwrap();
        record_usage_execution_interval(
            &mut store,
            &project,
            "iv-b",
            &UsageExecutionInterval {
                execution_id: "exec-b".into(),
                interval: UsageTimeInterval {
                    start_ms: 5,
                    end_ms: 15,
                },
            },
        )
        .unwrap();
        let time = query_usage_time_totals(&store, &project).unwrap().unwrap();
        assert_eq!(time.observed_wall_clock_ms, 15);
        assert_eq!(time.observed_execution_ms, 20);

        let handoff = usage_observation_for_ws043(
            &store,
            &project,
            &UsageCoverageObservation {
                observed_calls: 1,
                expected_calls: Some(2),
                observed_time_ms: 15,
                expected_time_ms: Some(30),
            },
        )
        .unwrap();
        assert!(handoff.is_historical_observation);
        assert!(handoff.is_not_estimated_remaining_time);
        assert_eq!(handoff.applied_correction_ids, vec!["c1".to_string()]);
        assert_eq!(handoff.coverage.call_coverage, Some((1, 2)));
        assert!(
            super::refuse_eta_from_cumulative_duration("预计剩余时间", time.observed_execution_ms)
                .is_err()
        );
    }

    #[test]
    fn unattested_project_is_refused() {
        let (mut store, mut project) = fixture();
        project.attested = false;
        let r = receipt(&project.project_id, "r1", "call-1");
        assert!(matches!(
            ingest_usage_receipt(&mut store, &project, "req", &r),
            Err(UsageRuntimeError::ProjectNotAttested)
        ));
    }
}
