//! SQLite persistence for deduped usage receipts and measured time (WS-041).
use crate::{Store, db_error};
use awr_core::workstream_usage::{
    self, UsageAllocationRecord, UsageChannel, UsageCorrection, UsageCost, UsageCostTotals,
    UsageCounterSnapshot, UsageCoverageObservation, UsageError, UsageExecutionInterval,
    UsageOccurrenceBinding, UsageReceipt, UsageTimeObservationHandoff, UsageTimeTotals,
};
use awr_core::{Error, Event, EventDraft, Id, Result, Revision, now_millis};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
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

/// An immutable observation or adjustment in the existing local usage ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum UsageRecord {
    Receipt(UsageReceipt),
    Correction(UsageCorrection),
    Allocation(UsageAllocationRecord),
    Counter(UsageCounterSnapshot),
    Interval(UsageExecutionInterval),
}

impl UsageRecord {
    fn operation(&self) -> &'static str {
        match self {
            Self::Receipt(_) => "ingest_receipt",
            Self::Correction(_) => "record_correction",
            Self::Allocation(_) => "record_allocation",
            Self::Counter(_) => "record_counter",
            Self::Interval(_) => "record_interval",
        }
    }
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

fn storage_integer(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| map_usage(UsageError::Overflow))
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
                storage_integer(subject.observed_at_ms)?
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

struct UsageWriter<'a> {
    conn: &'a rusqlite::Connection,
}

impl UsageWriter<'_> {
    fn apply(
        &self,
        project: Id,
        request_key: &str,
        record: &UsageRecord,
    ) -> Result<(UsageRecord, UsageIngestReceipt)> {
        match record {
            UsageRecord::Receipt(value) => self
                .ingest_usage_receipt(project, request_key, value)
                .map(|(saved, receipt)| (UsageRecord::Receipt(saved), receipt)),
            UsageRecord::Correction(value) => self
                .record_usage_correction(project, request_key, value)
                .map(|(saved, receipt)| (UsageRecord::Correction(saved), receipt)),
            UsageRecord::Allocation(value) => self
                .record_usage_allocation(project, request_key, value)
                .map(|(saved, receipt)| (UsageRecord::Allocation(saved), receipt)),
            UsageRecord::Counter(value) => self
                .record_usage_counter_snapshot(project, request_key, value)
                .map(|(saved, receipt)| (UsageRecord::Counter(saved), receipt)),
            UsageRecord::Interval(value) => self
                .record_usage_execution_interval(project, request_key, value)
                .map(|(saved, receipt)| (UsageRecord::Interval(saved), receipt)),
        }
    }

    fn ingest_usage_receipt(
        &self,
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
            let stored = if stored.receipt_id != receipt.receipt_id
                && stored.same_call_content_public(receipt)
            {
                stored
            } else {
                require_same(stored, receipt)?
            };
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
                    storage_integer(receipt.occurred_at_ms)?,
                    channel_str(receipt.channel),
                    receipt.attribution.work_id,
                    receipt.attribution.execution_id,
                    mainline,
                    cost_kind(&receipt.cost),
                    currency,
                    micros.map(storage_integer).transpose()?,
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

    fn record_usage_correction(
        &self,
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
                    storage_integer(correction.corrected_at_ms)?,
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

    fn record_usage_allocation(
        &self,
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
                    storage_integer(record.recorded_at_ms)?,
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

    fn record_usage_counter_snapshot(
        &self,
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
                    storage_integer(snapshot.observed_at_ms)?,
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

    fn record_usage_execution_interval(
        &self,
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
                    storage_integer(interval.interval.start_ms)?,
                    storage_integer(interval.interval.end_ms)?,
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
}

impl Store {
    fn usage_transaction<T>(
        &mut self,
        apply: impl FnOnce(&UsageWriter<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let value = apply(&UsageWriter { conn: &tx })?;
        tx.commit().map_err(db_error)?;
        Ok(value)
    }

    pub fn ingest_usage_receipt(
        &mut self,
        project: Id,
        request_key: &str,
        receipt: &UsageReceipt,
    ) -> Result<(UsageReceipt, UsageIngestReceipt)> {
        self.usage_transaction(|writer| writer.ingest_usage_receipt(project, request_key, receipt))
    }

    pub fn record_usage_correction(
        &mut self,
        project: Id,
        request_key: &str,
        correction: &UsageCorrection,
    ) -> Result<(UsageCorrection, UsageIngestReceipt)> {
        self.usage_transaction(|writer| {
            writer.record_usage_correction(project, request_key, correction)
        })
    }

    pub fn record_usage_allocation(
        &mut self,
        project: Id,
        request_key: &str,
        allocation: &UsageAllocationRecord,
    ) -> Result<(UsageAllocationRecord, UsageIngestReceipt)> {
        self.usage_transaction(|writer| {
            writer.record_usage_allocation(project, request_key, allocation)
        })
    }

    pub fn record_usage_counter_snapshot(
        &mut self,
        project: Id,
        request_key: &str,
        snapshot: &UsageCounterSnapshot,
    ) -> Result<(UsageCounterSnapshot, UsageIngestReceipt)> {
        self.usage_transaction(|writer| {
            writer.record_usage_counter_snapshot(project, request_key, snapshot)
        })
    }

    pub fn record_usage_execution_interval(
        &mut self,
        project: Id,
        request_key: &str,
        interval: &UsageExecutionInterval,
    ) -> Result<(UsageExecutionInterval, UsageIngestReceipt)> {
        self.usage_transaction(|writer| {
            writer.record_usage_execution_interval(project, request_key, interval)
        })
    }

    /// Commit the observation, its intake receipt and an attributable runtime event together.
    /// Exact intake replays preserve their original observation and tolerate a newer revision.
    pub fn record_usage_at_revision(
        &mut self,
        project: Id,
        expected_revision: Revision,
        request_key: &str,
        record: &UsageRecord,
    ) -> Result<(UsageRecord, UsageIngestReceipt, Option<Event>)> {
        if request_key.trim().is_empty()
            || request_key.len() > 256
            || request_key.chars().any(char::is_control)
        {
            return Err(Error::InvalidInput(
                "usage request_key requires 1..256 printable bytes".into(),
            ));
        }
        let operation = record.operation();
        let mut caller = serde_json::json!({});
        crate::mcp::tag_operation(project, &mut caller);
        if load_ingest(&self.conn, &project.to_string(), request_key, operation)?.is_some()
            && caller.get("mcp_operation_id").is_none()
        {
            let (saved, receipt) =
                UsageWriter { conn: &self.conn }.apply(project, request_key, record)?;
            return Ok((saved, receipt, None));
        }
        let ((saved, receipt), event) = self.runtime_transaction_with_event(
            project,
            expected_revision,
            EventDraft::new("usage.recorded", "Usage observation recorded"),
            |tx, _, draft| {
                let (saved, receipt) =
                    UsageWriter { conn: tx }.apply(project, request_key, record)?;
                draft.payload = serde_json::json!({"version":1,"request_key":request_key,
                    "usage_op":operation,"usage_subject":receipt.subject_id});
                Ok((saved, receipt))
            },
        )?;
        Ok((saved, receipt, Some(event)))
    }

    pub fn list_usage_receipts(&self, project: Id) -> Result<Vec<UsageReceipt>> {
        list_receipts(&self.conn, &project.to_string())
    }

    pub fn list_usage_corrections(&self, project: Id) -> Result<Vec<UsageCorrection>> {
        list_corrections(&self.conn, &project.to_string())
    }

    pub fn list_usage_allocations(&self, project: Id) -> Result<Vec<UsageAllocationRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT body_json FROM usage_allocation_records WHERE project_id=?1 ORDER BY recorded_at_ms,allocation_id"
        ).map_err(db_error)?;
        let rows = statement
            .query_map([project.to_string()], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| {
            serde_json::from_str(&row.map_err(db_error)?)
                .map_err(|_| Error::Storage("stored usage allocation is invalid".into()))
        })
        .collect()
    }

    pub fn list_usage_counter_snapshots(&self, project: Id) -> Result<Vec<UsageCounterSnapshot>> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT body_json FROM usage_counter_snapshots WHERE project_id=?1
             ORDER BY observed_at_ms,provider_namespace,provider,model,session_id,counter_epoch",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([project.to_string()], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| {
            serde_json::from_str(&row.map_err(db_error)?)
                .map_err(|_| Error::Storage("stored usage counter is invalid".into()))
        })
        .collect()
    }

    pub fn list_usage_execution_intervals(
        &self,
        project: Id,
    ) -> Result<Vec<UsageExecutionInterval>> {
        list_intervals(&self.conn, &project.to_string())
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
        if intervals.is_empty() {
            return Ok(None);
        }
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
            (!dedup.is_empty()).then_some(dedup.as_slice()),
            coverage,
        )
        .map_err(map_usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::workstream_usage::*;
    use std::sync::{Arc, Barrier};

    fn fixture() -> (Store, awr_core::Project, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("awr-usage-store-{}", Id::new()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let mut store = Store::open(&root.join("state.db")).unwrap();
        let project = store
            .register_project(&root, "usage", "Usage records")
            .unwrap();
        (store, project, root)
    }

    fn receipt(project: Id, id: &str, call: &str) -> UsageReceipt {
        UsageReceipt {
            receipt_id: id.into(),
            project_id: project.to_string(),
            provider_namespace: "fixture-account".into(),
            provider: "fixture-provider".into(),
            call_id: call.into(),
            model: "fixture-model".into(),
            session_id: "fixture-session".into(),
            occurred_at_ms: 1,
            channel: UsageChannel::Model,
            attribution: UsageAttribution {
                work_id: "W".into(),
                execution_id: "E".into(),
                workstream_id: None,
            },
            tokens: Some(UsageTokens {
                input: 10,
                output: 3,
                cached_input: 2,
            }),
            cost: UsageCost::Actual(UsageMoney {
                currency: "USD".into(),
                micros: 100,
            }),
        }
    }

    fn record_legacy(
        store: &mut Store,
        project: Id,
        key: &str,
        record: &UsageRecord,
    ) -> Result<()> {
        match record {
            UsageRecord::Receipt(value) => {
                store.ingest_usage_receipt(project, key, value).map(|_| ())
            }
            UsageRecord::Correction(value) => store
                .record_usage_correction(project, key, value)
                .map(|_| ()),
            UsageRecord::Allocation(value) => store
                .record_usage_allocation(project, key, value)
                .map(|_| ()),
            UsageRecord::Counter(value) => store
                .record_usage_counter_snapshot(project, key, value)
                .map(|_| ()),
            UsageRecord::Interval(value) => store
                .record_usage_execution_interval(project, key, value)
                .map(|_| ()),
        }
    }

    #[test]
    fn usage_intake_failure_rolls_back_every_record_kind_and_can_be_retried() {
        let (mut store, project, root) = fixture();
        let original = receipt(project.id, "original", "first-call");
        let allocated = receipt(project.id, "allocated", "second-call");
        store
            .ingest_usage_receipt(project.id, "original", &original)
            .unwrap();
        store
            .ingest_usage_receipt(project.id, "allocated", &allocated)
            .unwrap();
        let records = [
            UsageRecord::Receipt(receipt(project.id, "new", "third-call")),
            UsageRecord::Correction(UsageCorrection {
                correction_id: "adjustment".into(),
                project_id: project.id.to_string(),
                target_receipt_id: original.receipt_id.clone(),
                corrected_at_ms: 2,
                reason: "Correct the observed amount".into(),
                prior_cost: original.cost.clone(),
                new_cost: UsageCost::Actual(UsageMoney {
                    currency: "USD".into(),
                    micros: 80,
                }),
                actor: "fixture-owner".into(),
            }),
            UsageRecord::Allocation(UsageAllocationRecord {
                allocation_id: "allocation".into(),
                project_id: project.id.to_string(),
                receipt_id: allocated.receipt_id.clone(),
                rule: Some("Documented cost sharing".into()),
                recorded_at_ms: 2,
                shares: vec![
                    UsageAllocation {
                        workstream_id: Some(Id::from(7u128)),
                        micros: 60,
                    },
                    UsageAllocation {
                        workstream_id: Some(Id::from(8u128)),
                        micros: 40,
                    },
                ],
            }),
            UsageRecord::Counter(UsageCounterSnapshot {
                scope: UsageCounterScope {
                    project_id: project.id.to_string(),
                    provider_namespace: "fixture-account".into(),
                    provider: "fixture-provider".into(),
                    model: "fixture-model".into(),
                    session_id: "fixture-session".into(),
                    counter_epoch: "first".into(),
                },
                observed_at_ms: 2,
                tokens: UsageTokens {
                    input: 10,
                    output: 3,
                    cached_input: 2,
                },
            }),
            UsageRecord::Interval(UsageExecutionInterval {
                execution_id: "E".into(),
                interval: UsageTimeInterval {
                    start_ms: 1,
                    end_ms: 11,
                },
            }),
        ];
        store
            .conn
            .execute_batch(
                "CREATE TEMP TRIGGER reject_usage_intake BEFORE INSERT ON usage_ingest_receipts
            BEGIN SELECT RAISE(ABORT, 'synthetic intake failure'); END;",
            )
            .unwrap();
        for (index, record) in records.iter().enumerate() {
            assert!(
                record_legacy(&mut store, project.id, &format!("record-{index}"), record).is_err()
            );
        }
        assert_eq!(store.list_usage_receipts(project.id).unwrap().len(), 2);
        assert!(store.list_usage_corrections(project.id).unwrap().is_empty());
        assert!(store.list_usage_allocations(project.id).unwrap().is_empty());
        assert!(
            store
                .list_usage_counter_snapshots(project.id)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .list_usage_execution_intervals(project.id)
                .unwrap()
                .is_empty()
        );
        store
            .conn
            .execute_batch("DROP TRIGGER reject_usage_intake;")
            .unwrap();
        for (index, record) in records.iter().enumerate() {
            record_legacy(&mut store, project.id, &format!("record-{index}"), record).unwrap();
            record_legacy(&mut store, project.id, &format!("record-{index}"), record).unwrap();
        }
        assert_eq!(store.list_usage_receipts(project.id).unwrap().len(), 3);
        assert_eq!(
            store
                .usage_cost_totals(project.id, true)
                .unwrap()
                .actual_micros["USD"],
            280
        );
        assert_eq!(store.list_usage_corrections(project.id).unwrap().len(), 1);
        assert_eq!(store.list_usage_allocations(project.id).unwrap().len(), 1);
        assert_eq!(
            store
                .list_usage_counter_snapshots(project.id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .list_usage_execution_intervals(project.id)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_aliases_keep_one_call_and_replay_both_intake_keys() {
        let (store, project, root) = fixture();
        drop(store);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut store = Store::open_existing(&root.join("state.db")).unwrap();
                    let mut input =
                        receipt(project.id, &format!("observation-{index}"), "shared-call");
                    if index == 1 {
                        input.channel = UsageChannel::Compaction;
                    }
                    let key = format!("client-{index}");
                    barrier.wait();
                    let first = store
                        .ingest_usage_receipt(project.id, &key, &input)
                        .unwrap();
                    let replay = store
                        .ingest_usage_receipt(project.id, &key, &input)
                        .unwrap();
                    assert_eq!(first.0, replay.0);
                    assert!(replay.1.replayed);
                    first.0.receipt_id
                })
            })
            .collect();
        let ids: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(ids[0], ids[1]);
        let store = Store::open_existing(&root.join("state.db")).unwrap();
        assert_eq!(store.list_usage_receipts(project.id).unwrap().len(), 1);
        assert_eq!(
            store
                .usage_cost_totals(project.id, false)
                .unwrap()
                .actual_micros["USD"],
            100
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_storage_range_errors_preserve_records_and_revision() {
        let (mut store, project, root) = fixture();
        let mut invalid_time = receipt(project.id, "invalid-time", "first-call");
        invalid_time.occurred_at_ms = u64::MAX;
        let mut invalid_cost = receipt(project.id, "invalid-cost", "second-call");
        invalid_cost.cost = UsageCost::Actual(UsageMoney {
            currency: "USD".into(),
            micros: u64::MAX,
        });
        let records = [
            UsageRecord::Receipt(invalid_time),
            UsageRecord::Receipt(invalid_cost),
            UsageRecord::Interval(UsageExecutionInterval {
                execution_id: "large-interval".into(),
                interval: UsageTimeInterval {
                    start_ms: u64::MAX - 1,
                    end_ms: u64::MAX,
                },
            }),
        ];
        for (index, record) in records.iter().enumerate() {
            assert!(
                store
                    .record_usage_at_revision(
                        project.id,
                        project.project_revision,
                        &format!("invalid-{index}"),
                        record,
                    )
                    .is_err()
            );
            assert_eq!(
                store.project(project.id).unwrap().project_revision,
                project.project_revision
            );
        }
        assert!(store.list_usage_receipts(project.id).unwrap().is_empty());
        assert!(
            store
                .list_usage_execution_intervals(project.id)
                .unwrap()
                .is_empty()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn revision_bound_usage_preserves_replays_and_missing_time() {
        let (mut store, project, root) = fixture();
        assert!(store.usage_time_totals(project.id).unwrap().is_none());
        let record = UsageRecord::Receipt(receipt(project.id, "observed", "one-call"));
        let first = store
            .record_usage_at_revision(project.id, project.project_revision, "one", &record)
            .unwrap();
        assert!(first.2.is_some());
        let revision = store.project(project.id).unwrap().project_revision;
        let replay = store
            .record_usage_at_revision(project.id, project.project_revision, "one", &record)
            .unwrap();
        assert!(replay.1.replayed);
        assert!(replay.2.is_none());
        assert_eq!(
            store.project(project.id).unwrap().project_revision,
            revision
        );
        assert!(matches!(
            store.record_usage_at_revision(
                project.id,
                project.project_revision,
                "different",
                &record
            ),
            Err(Error::RevisionConflict { .. })
        ));
        let other = UsageRecord::Receipt(receipt(Id::new(), "foreign", "foreign-call"));
        assert!(
            store
                .record_usage_at_revision(project.id, revision, "foreign", &other)
                .is_err()
        );
        assert_eq!(
            store.project(project.id).unwrap().project_revision,
            revision
        );
        assert_eq!(store.list_usage_receipts(project.id).unwrap().len(), 1);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
