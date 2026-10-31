//! SQLite persistence for deduped usage receipts and measured time (WS-041).
use crate::{Store, db_error};
use awr_core::workstream_usage::{
    self, UsageAllocationRecord, UsageChannel, UsageCorrection, UsageCost, UsageCostTotals,
    UsageCounterSnapshot, UsageCoverageObservation, UsageError, UsageExecutionInterval,
    UsageOccurrenceBinding, UsageReceipt, UsageTimeObservationHandoff, UsageTimeTotals,
};
use awr_core::{Error, Id, Result, now_millis};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageIngestReceipt {
    pub request_key: String,
    pub subject_id: String,
    pub op: String,
    pub event_id: String,
    pub replayed: bool,
    pub created_at_ms: i64,
}

fn map_usage(err: UsageError) -> Error {
    match err {
        UsageError::Invalid(msg) => Error::InvalidInput(msg.into()),
        UsageError::Conflict => Error::RuleViolation("usage identity conflict".into()),
        UsageError::CounterBoundary => Error::InvalidInput("usage counter boundary".into()),
        UsageError::Overflow => Error::InvalidInput("usage arithmetic overflow".into()),
        UsageError::EtaFromCumulativeDuration => Error::RuleViolation(
            "cumulative duration must not be presented as estimated remaining time".into(),
        ),
        UsageError::CorrectionAudit => Error::RuleViolation("usage correction audit failed".into()),
    }
}

fn require_same<T: PartialEq>(stored: T, incoming: &T) -> Result<T> {
    if &stored == incoming {
        Ok(stored)
    } else {
        Err(map_usage(UsageError::Conflict))
    }
}

#[derive(Serialize, Deserialize)]
struct CounterSubject {
    provider_namespace: String,
    provider: String,
    model: String,
    session_id: String,
    counter_epoch: String,
    observed_at_ms: u64,
}

fn counter_subject(snapshot: &UsageCounterSnapshot) -> Result<String> {
    serde_json::to_string(&CounterSubject {
        provider_namespace: snapshot.scope.provider_namespace.clone(),
        provider: snapshot.scope.provider.clone(),
        model: snapshot.scope.model.clone(),
        session_id: snapshot.scope.session_id.clone(),
        counter_epoch: snapshot.scope.counter_epoch.clone(),
        observed_at_ms: snapshot.observed_at_ms,
    })
    .map_err(|e| Error::Storage(e.to_string()))
}

fn cost_kind(cost: &UsageCost) -> &'static str {
    match cost {
        UsageCost::Actual(_) => "actual",
        UsageCost::ApiEquivalentEstimate { .. } => "api_equivalent_estimate",
        UsageCost::Unknown => "unknown",
    }
}

fn cost_parts(cost: &UsageCost) -> (Option<String>, Option<u64>) {
    match cost {
        UsageCost::Actual(m) | UsageCost::ApiEquivalentEstimate { amount: m, .. } => {
            (Some(m.currency.clone()), Some(m.micros))
        }
        UsageCost::Unknown => (None, None),
    }
}

fn channel_str(channel: UsageChannel) -> &'static str {
    match channel {
        UsageChannel::Model => "model",
        UsageChannel::Compaction => "compaction",
    }
}

fn load_ingest(
    conn: &rusqlite::Connection,
    project: &str,
    request_key: &str,
    op: &str,
) -> Result<Option<UsageIngestReceipt>> {
    let row = conn
        .query_row(
            "SELECT request_key, subject_id, op, event_id, replayed, created_at_ms
             FROM usage_ingest_receipts WHERE project_id=?1 AND request_key=?2",
            params![project, request_key],
            |r| {
                Ok(UsageIngestReceipt {
                    request_key: r.get(0)?,
                    subject_id: r.get(1)?,
                    op: r.get(2)?,
                    event_id: r.get(3)?,
                    replayed: r.get::<_, i64>(4)? != 0,
                    created_at_ms: r.get(5)?,
                })
            },
        )
        .optional()
        .map_err(db_error)?;
    match row {
        Some(receipt) if receipt.op == op => Ok(Some(receipt)),
        Some(_) => Err(Error::RuleViolation(
            "usage ingest request_key reused with different op".into(),
        )),
        None => Ok(None),
    }
}

fn record_ingest(
    conn: &rusqlite::Connection,
    project: &str,
    request_key: &str,
    subject_id: &str,
    op: &str,
) -> Result<UsageIngestReceipt> {
    let created_at_ms = now_millis()?;
    let event_id = Id::new().to_string();
    conn.execute(
        "INSERT INTO usage_ingest_receipts(
            project_id, request_key, subject_id, op, event_id, replayed, created_at_ms)
         VALUES(?1,?2,?3,?4,?5,0,?6)",
        params![
            project,
            request_key,
            subject_id,
            op,
            event_id,
            created_at_ms
        ],
    )
    .map_err(db_error)?;
    Ok(UsageIngestReceipt {
        request_key: request_key.into(),
        subject_id: subject_id.into(),
        op: op.into(),
        event_id,
        replayed: false,
        created_at_ms,
    })
}

fn load_receipt_body(
    conn: &rusqlite::Connection,
    project: &str,
    receipt_id: &str,
) -> Result<Option<UsageReceipt>> {
    let row = conn
        .query_row(
            "SELECT body_json FROM usage_receipts WHERE project_id=?1 AND receipt_id=?2",
            params![project, receipt_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    match row {
        None => Ok(None),
        Some(json) => {
            Ok(Some(serde_json::from_str(&json).map_err(|e| {
                Error::Storage(format!("corrupt usage receipt: {e}"))
            })?))
        }
    }
}

fn list_receipts(conn: &rusqlite::Connection, project: &str) -> Result<Vec<UsageReceipt>> {
    let mut stmt = conn
        .prepare(
            "SELECT body_json FROM usage_receipts WHERE project_id=?1 ORDER BY occurred_at_ms, receipt_id",
        )
        .map_err(db_error)?;
    let rows = stmt
        .query_map(params![project], |r| r.get::<_, String>(0))
        .map_err(db_error)?;
    let mut out = Vec::new();
    for row in rows {
        let json = row.map_err(db_error)?;
        out.push(
            serde_json::from_str(&json)
                .map_err(|e| Error::Storage(format!("corrupt usage receipt: {e}")))?,
        );
    }
    Ok(out)
}

fn list_corrections(conn: &rusqlite::Connection, project: &str) -> Result<Vec<UsageCorrection>> {
    let mut stmt = conn
        .prepare(
            "SELECT body_json FROM usage_corrections WHERE project_id=?1 ORDER BY corrected_at_ms, correction_id",
        )
        .map_err(db_error)?;
    let rows = stmt
        .query_map(params![project], |r| r.get::<_, String>(0))
        .map_err(db_error)?;
    let mut out = Vec::new();
    for row in rows {
        let json = row.map_err(db_error)?;
        out.push(
            serde_json::from_str(&json)
                .map_err(|e| Error::Storage(format!("corrupt usage correction: {e}")))?,
        );
    }
    Ok(out)
}

fn list_intervals(
    conn: &rusqlite::Connection,
    project: &str,
) -> Result<Vec<UsageExecutionInterval>> {
    let mut stmt = conn
        .prepare(
            "SELECT body_json FROM usage_execution_intervals WHERE project_id=?1 ORDER BY start_ms, execution_id",
        )
        .map_err(db_error)?;
    let rows = stmt
        .query_map(params![project], |r| r.get::<_, String>(0))
        .map_err(db_error)?;
    let mut out = Vec::new();
    for row in rows {
        let json = row.map_err(db_error)?;
        out.push(
            serde_json::from_str(&json)
                .map_err(|e| Error::Storage(format!("corrupt usage execution interval: {e}")))?,
        );
    }
    Ok(out)
}

fn load_counter_by_subject(
    conn: &rusqlite::Connection,
    project: &str,
    subject_id: &str,
) -> Result<UsageCounterSnapshot> {
    let subject: CounterSubject = serde_json::from_str(subject_id)
        .map_err(|e| Error::Storage(format!("corrupt usage counter subject: {e}")))?;
    let json: String = conn
        .query_row(
            "SELECT body_json FROM usage_counter_snapshots
             WHERE project_id=?1 AND provider_namespace=?2 AND provider=?3 AND model=?4
               AND session_id=?5 AND counter_epoch=?6 AND observed_at_ms=?7",
            params![
                project,
                subject.provider_namespace,
                subject.provider,
                subject.model,
                subject.session_id,
                subject.counter_epoch,
                subject.observed_at_ms as i64
            ],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    serde_json::from_str(&json).map_err(|e| Error::Storage(format!("corrupt usage counter: {e}")))
}

fn effective_receipt(
    conn: &rusqlite::Connection,
    project: &str,
    receipt_id: &str,
) -> Result<UsageReceipt> {
    let receipts = list_receipts(conn, project)?;
    let corrections = list_corrections(conn, project)?;
    let (corrected, _) =
        workstream_usage::apply_usage_corrections(project, &receipts, &corrections)
            .map_err(map_usage)?;
    corrected
        .into_iter()
        .find(|receipt| receipt.receipt_id == receipt_id)
        .ok_or_else(|| Error::NotFound("usage receipt missing for allocation".into()))
}

impl Store {
    pub fn ingest_usage_receipt(
        &mut self,
        project: Id,
        request_key: &str,
        receipt: &UsageReceipt,
    ) -> Result<(UsageReceipt, UsageIngestReceipt)> {
        let project_s = project.to_string();
        if project_s != receipt.project_id {
            return Err(Error::InvalidInput(
                "usage receipt project must match store project".into(),
            ));
        }
        if request_key.trim().is_empty() {
            return Err(Error::InvalidInput(
                "usage ingest request_key required".into(),
            ));
        }
        workstream_usage::deduplicate_usage(&project_s, &[receipt.clone()]).map_err(map_usage)?;
        if let Some(existing) = load_ingest(&self.conn, &project_s, request_key, "ingest_receipt")?
        {
            let stored = load_receipt_body(&self.conn, &project_s, &existing.subject_id)?
                .ok_or_else(|| Error::Storage("usage receipt missing for ingest receipt".into()))?;
            let stored = require_same(stored, receipt)?;
            return Ok((
                stored,
                UsageIngestReceipt {
                    replayed: true,
                    ..existing
                },
            ));
        }
        if let Some(prior) = load_receipt_body(&self.conn, &project_s, &receipt.receipt_id)? {
            if &prior != receipt {
                return Err(map_usage(UsageError::Conflict));
            }
        }
        // Call-id uniqueness across receipt IDs.
        let conflict: Option<String> = self
            .conn
            .query_row(
                "SELECT receipt_id FROM usage_receipts
                 WHERE project_id=?1 AND provider_namespace=?2 AND provider=?3 AND call_id=?4",
                params![
                    project_s,
                    receipt.provider_namespace,
                    receipt.provider,
                    receipt.call_id
                ],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(other_id) = conflict {
            if other_id != receipt.receipt_id {
                let other = load_receipt_body(&self.conn, &project_s, &other_id)?
                    .ok_or_else(|| Error::Storage("usage call index corrupt".into()))?;
                if !other.same_call_content_public(receipt) {
                    return Err(map_usage(UsageError::Conflict));
                }
                // Same call already stored under another receipt id — treat as replay of that row.
                let ingest = record_ingest(
                    &self.conn,
                    &project_s,
                    request_key,
                    &other.receipt_id,
                    "ingest_receipt",
                )?;
                return Ok((other, ingest));
            }
        }
        let body = serde_json::to_string(receipt).map_err(|e| Error::Storage(e.to_string()))?;
        let (currency, micros) = cost_parts(&receipt.cost);
        let mainline = receipt.attribution.workstream_id.map(|id| id.to_string());
        let ingested_at_ms = now_millis()?;
        self.conn
            .execute(
                "INSERT INTO usage_receipts(
                    project_id, receipt_id, provider_namespace, provider, call_id, model, session_id,
                    occurred_at_ms, channel, work_id, execution_id, occurrence_mainline_id,
                    cost_kind, currency, cost_micros, body_json, ingested_at_ms)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
                 ON CONFLICT(project_id, receipt_id) DO UPDATE SET
                    body_json=excluded.body_json,
                    cost_kind=excluded.cost_kind,
                    currency=excluded.currency,
                    cost_micros=excluded.cost_micros,
                    ingested_at_ms=excluded.ingested_at_ms",
                params![
                    project_s,
                    receipt.receipt_id,
                    receipt.provider_namespace,
                    receipt.provider,
                    receipt.call_id,
                    receipt.model,
                    receipt.session_id,
                    receipt.occurred_at_ms as i64,
                    channel_str(receipt.channel),
                    receipt.attribution.work_id,
                    receipt.attribution.execution_id,
                    mainline,
                    cost_kind(&receipt.cost),
                    currency,
                    micros.map(|v| v as i64),
                    body,
                    ingested_at_ms,
                ],
            )
            .map_err(db_error)?;
        let ingest = record_ingest(
            &self.conn,
            &project_s,
            request_key,
            &receipt.receipt_id,
            "ingest_receipt",
        )?;
        Ok((receipt.clone(), ingest))
    }

    pub fn record_usage_correction(
        &mut self,
        project: Id,
        request_key: &str,
        correction: &UsageCorrection,
    ) -> Result<(UsageCorrection, UsageIngestReceipt)> {
        let project_s = project.to_string();
        if project_s != correction.project_id {
            return Err(Error::InvalidInput(
                "usage correction project must match store project".into(),
            ));
        }
        if let Some(existing) =
            load_ingest(&self.conn, &project_s, request_key, "record_correction")?
        {
            let body: UsageCorrection = self
                .conn
                .query_row(
                    "SELECT body_json FROM usage_corrections WHERE project_id=?1 AND correction_id=?2",
                    params![project_s, existing.subject_id],
                    |r| r.get::<_, String>(0),
                )
                .map_err(db_error)
                .and_then(|json| {
                    serde_json::from_str(&json)
                        .map_err(|e| Error::Storage(format!("corrupt usage correction: {e}")))
                })?;
            let body = require_same(body, correction)?;
            return Ok((
                body,
                UsageIngestReceipt {
                    replayed: true,
                    ..existing
                },
            ));
        }
        let existing_correction: Option<String> = self
            .conn
            .query_row(
                "SELECT correction_id FROM usage_corrections
                 WHERE project_id=?1 AND target_receipt_id=?2",
                params![project_s, correction.target_receipt_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if existing_correction.is_some() {
            return Err(map_usage(UsageError::CorrectionAudit));
        }
        let receipts = list_receipts(&self.conn, &project_s)?;
        workstream_usage::apply_usage_corrections(&project_s, &receipts, &[correction.clone()])
            .map_err(map_usage)?;
        let body = serde_json::to_string(correction).map_err(|e| Error::Storage(e.to_string()))?;
        self.conn
            .execute(
                "INSERT INTO usage_corrections(
                    project_id, correction_id, target_receipt_id, corrected_at_ms, reason, actor, body_json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    project_s,
                    correction.correction_id,
                    correction.target_receipt_id,
                    correction.corrected_at_ms as i64,
                    correction.reason,
                    correction.actor,
                    body
                ],
            )
            .map_err(db_error)?;
        let ingest = record_ingest(
            &self.conn,
            &project_s,
            request_key,
            &correction.correction_id,
            "record_correction",
        )?;
        Ok((correction.clone(), ingest))
    }

    pub fn record_usage_allocation(
        &mut self,
        project: Id,
        request_key: &str,
        record: &UsageAllocationRecord,
    ) -> Result<(UsageAllocationRecord, UsageIngestReceipt)> {
        let project_s = project.to_string();
        if project_s != record.project_id {
            return Err(Error::InvalidInput(
                "usage allocation project must match store project".into(),
            ));
        }
        if let Some(existing) =
            load_ingest(&self.conn, &project_s, request_key, "record_allocation")?
        {
            let body: UsageAllocationRecord = self
                .conn
                .query_row(
                    "SELECT body_json FROM usage_allocation_records WHERE project_id=?1 AND allocation_id=?2",
                    params![project_s, existing.subject_id],
                    |r| r.get::<_, String>(0),
                )
                .map_err(db_error)
                .and_then(|json| {
                    serde_json::from_str(&json)
                        .map_err(|e| Error::Storage(format!("corrupt usage allocation: {e}")))
                })?;
            let body = require_same(body, record)?;
            return Ok((
                body,
                UsageIngestReceipt {
                    replayed: true,
                    ..existing
                },
            ));
        }
        let prior_allocation: Option<String> = self
            .conn
            .query_row(
                "SELECT allocation_id FROM usage_allocation_records
                 WHERE project_id=?1 AND receipt_id=?2",
                params![project_s, record.receipt_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if prior_allocation.is_some() {
            return Err(map_usage(UsageError::Conflict));
        }
        let receipt = effective_receipt(&self.conn, &project_s, &record.receipt_id)?;
        record.validate_against(&receipt).map_err(map_usage)?;
        let body = serde_json::to_string(record).map_err(|e| Error::Storage(e.to_string()))?;
        self.conn
            .execute(
                "INSERT INTO usage_allocation_records(
                    project_id, allocation_id, receipt_id, recorded_at_ms, rule, body_json)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    project_s,
                    record.allocation_id,
                    record.receipt_id,
                    record.recorded_at_ms as i64,
                    record.rule,
                    body
                ],
            )
            .map_err(db_error)?;
        let ingest = record_ingest(
            &self.conn,
            &project_s,
            request_key,
            &record.allocation_id,
            "record_allocation",
        )?;
        Ok((record.clone(), ingest))
    }

    pub fn record_usage_counter_snapshot(
        &mut self,
        project: Id,
        request_key: &str,
        snapshot: &UsageCounterSnapshot,
    ) -> Result<(UsageCounterSnapshot, UsageIngestReceipt)> {
        let project_s = project.to_string();
        if project_s != snapshot.scope.project_id {
            return Err(Error::InvalidInput(
                "usage counter project must match store project".into(),
            ));
        }
        if let Some(existing) = load_ingest(&self.conn, &project_s, request_key, "record_counter")?
        {
            let body = load_counter_by_subject(&self.conn, &project_s, &existing.subject_id)?;
            let body = require_same(body, snapshot)?;
            return Ok((
                body,
                UsageIngestReceipt {
                    replayed: true,
                    ..existing
                },
            ));
        }
        workstream_usage::validate_usage_counter_snapshot(snapshot).map_err(map_usage)?;
        let body = serde_json::to_string(snapshot).map_err(|e| Error::Storage(e.to_string()))?;
        let prior: Option<UsageCounterSnapshot> = self
            .conn
            .query_row(
                "SELECT body_json FROM usage_counter_snapshots
                 WHERE project_id=?1 AND provider_namespace=?2 AND provider=?3 AND model=?4
                   AND session_id=?5 AND counter_epoch=?6
                 ORDER BY observed_at_ms DESC LIMIT 1",
                params![
                    project_s,
                    snapshot.scope.provider_namespace,
                    snapshot.scope.provider,
                    snapshot.scope.model,
                    snapshot.scope.session_id,
                    snapshot.scope.counter_epoch
                ],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?
            .map(|json| {
                serde_json::from_str(&json)
                    .map_err(|e| Error::Storage(format!("corrupt usage counter: {e}")))
            })
            .transpose()?;
        if let Some(prev) = prior.as_ref() {
            let _ = workstream_usage::usage_counter_delta(prev, snapshot).map_err(map_usage)?;
        }
        self.conn
            .execute(
                "INSERT INTO usage_counter_snapshots(
                    project_id, provider_namespace, provider, model, session_id, counter_epoch,
                    observed_at_ms, body_json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    project_s,
                    snapshot.scope.provider_namespace,
                    snapshot.scope.provider,
                    snapshot.scope.model,
                    snapshot.scope.session_id,
                    snapshot.scope.counter_epoch,
                    snapshot.observed_at_ms as i64,
                    body
                ],
            )
            .map_err(db_error)?;
        let subject = counter_subject(snapshot)?;
        let ingest = record_ingest(
            &self.conn,
            &project_s,
            request_key,
            &subject,
            "record_counter",
        )?;
        Ok((snapshot.clone(), ingest))
    }

    pub fn record_usage_execution_interval(
        &mut self,
        project: Id,
        request_key: &str,
        interval: &UsageExecutionInterval,
    ) -> Result<(UsageExecutionInterval, UsageIngestReceipt)> {
        let project_s = project.to_string();
        if let Some(existing) = load_ingest(&self.conn, &project_s, request_key, "record_interval")?
        {
            let body: UsageExecutionInterval = self
                .conn
                .query_row(
                    "SELECT body_json FROM usage_execution_intervals
                     WHERE project_id=?1 AND execution_id=?2",
                    params![project_s, existing.subject_id],
                    |r| r.get::<_, String>(0),
                )
                .map_err(db_error)
                .and_then(|json| {
                    serde_json::from_str(&json).map_err(|e| {
                        Error::Storage(format!("corrupt usage execution interval: {e}"))
                    })
                })?;
            let body = require_same(body, interval)?;
            return Ok((
                body,
                UsageIngestReceipt {
                    replayed: true,
                    ..existing
                },
            ));
        }
        workstream_usage::deduplicate_execution_intervals(&[interval.clone()])
            .map_err(map_usage)?;
        if let Some(prior_json) = self
            .conn
            .query_row(
                "SELECT body_json FROM usage_execution_intervals WHERE project_id=?1 AND execution_id=?2",
                params![project_s, interval.execution_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?
        {
            let prior: UsageExecutionInterval = serde_json::from_str(&prior_json)
                .map_err(|e| Error::Storage(format!("corrupt usage execution interval: {e}")))?;
            if &prior != interval {
                return Err(map_usage(UsageError::Conflict));
            }
        }
        let body = serde_json::to_string(interval).map_err(|e| Error::Storage(e.to_string()))?;
        self.conn
            .execute(
                "INSERT INTO usage_execution_intervals(
                    project_id, execution_id, start_ms, end_ms, body_json)
                 VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(project_id, execution_id) DO UPDATE SET
                    start_ms=excluded.start_ms,
                    end_ms=excluded.end_ms,
                    body_json=excluded.body_json",
                params![
                    project_s,
                    interval.execution_id,
                    interval.interval.start_ms as i64,
                    interval.interval.end_ms as i64,
                    body
                ],
            )
            .map_err(db_error)?;
        let ingest = record_ingest(
            &self.conn,
            &project_s,
            request_key,
            &interval.execution_id,
            "record_interval",
        )?;
        Ok((interval.clone(), ingest))
    }

    pub fn list_usage_receipts(&self, project: Id) -> Result<Vec<UsageReceipt>> {
        list_receipts(&self.conn, &project.to_string())
    }

    pub fn usage_occurrence_bindings(&self, project: Id) -> Result<Vec<UsageOccurrenceBinding>> {
        let project_s = project.to_string();
        let receipts = list_receipts(&self.conn, &project_s)?;
        workstream_usage::usage_occurrence_bindings(&project_s, &receipts).map_err(map_usage)
    }

    /// The receipts of a project with its append-only corrections applied; every receipt stays readable as stored.
    pub fn usage_receipts_corrected(&self, project: Id) -> Result<Vec<UsageReceipt>> {
        let project_s = project.to_string();
        let receipts = list_receipts(&self.conn, &project_s)?;
        let corrections = list_corrections(&self.conn, &project_s)?;
        let (corrected, _) =
            workstream_usage::apply_usage_corrections(&project_s, &receipts, &corrections)
                .map_err(map_usage)?;
        Ok(corrected)
    }

    pub fn usage_cost_totals(
        &self,
        project: Id,
        apply_corrections: bool,
    ) -> Result<UsageCostTotals> {
        let project_s = project.to_string();
        let receipts = list_receipts(&self.conn, &project_s)?;
        if apply_corrections {
            let corrections = list_corrections(&self.conn, &project_s)?;
            let (corrected, _) =
                workstream_usage::apply_usage_corrections(&project_s, &receipts, &corrections)
                    .map_err(map_usage)?;
            workstream_usage::usage_cost_totals(&project_s, &corrected).map_err(map_usage)
        } else {
            workstream_usage::usage_cost_totals(&project_s, &receipts).map_err(map_usage)
        }
    }

    pub fn usage_time_totals(&self, project: Id) -> Result<Option<UsageTimeTotals>> {
        let project_s = project.to_string();
        let intervals = list_intervals(&self.conn, &project_s)?;
        let dedup =
            workstream_usage::deduplicate_execution_intervals(&intervals).map_err(map_usage)?;
        workstream_usage::usage_time_totals(Some(&dedup)).map_err(map_usage)
    }

    pub fn usage_observation_handoff(
        &self,
        project: Id,
        coverage: &UsageCoverageObservation,
    ) -> Result<UsageTimeObservationHandoff> {
        let project_s = project.to_string();
        let receipts = list_receipts(&self.conn, &project_s)?;
        let corrections = list_corrections(&self.conn, &project_s)?;
        let intervals = list_intervals(&self.conn, &project_s)?;
        let dedup =
            workstream_usage::deduplicate_execution_intervals(&intervals).map_err(map_usage)?;
        workstream_usage::usage_observation_handoff(
            &project_s,
            &receipts,
            &corrections,
            Some(&dedup),
            coverage,
        )
        .map_err(map_usage)
    }
}
