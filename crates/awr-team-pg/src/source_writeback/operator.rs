//! Schema-owner recovery for a pending planning source writeback.
//! Never reachable through client HTTP/MCP.
//!
//! A pending journal (`validated`, `source_written`, `pg_activating`) fences every other source activation of the
//! project and the effects of the works it affects until the original request completes. Normally the original
//! client resumes it. When that cannot happen (the client is gone, its permission was revoked, the source was
//! changed from outside, or the journal predates the recorded intent) the project would stay fenced forever.
//!
//! Closing the journal here changes neither the source file nor the active snapshot. It only moves the journal to
//! `refused` with the owner's decision, so the fences end and the original request can no longer resume. If the
//! source holds bytes the server wrote (or anything else) the owner must say so with `retain_source`: they stay as
//! they are and have to be reconciled through the normal source pipeline or restored from version control.
use super::request_binding::Intent;
use super::{LockedSourceFile, Value, fingerprint, journal, json};
use crate::operator_access::require_owner_project;
use crate::{PgError, PgResult};
use tokio_postgres::{Client, Row};

const PROTOCOL: &str = journal::OPERATOR_DECISION_PROTOCOL;
const PENDING: [&str; 3] = journal::PENDING_PHASES;
const LIMIT: i64 = 50;
const MAX_REASON: usize = journal::MAX_OPERATOR_REASON;
const AFFECTED_SAMPLE: usize = 20;
const MALFORMED_HASH_BLOCK: &str =
    "the recorded intent hash is malformed and cannot be used as a safe close token";

fn invalid() -> PgError {
    PgError::Protocol("invalid operator planning writeback request".into())
}

fn identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && !s.chars().any(char::is_control)
}

fn hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn source_fingerprint(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(hex)
}

/// Where the physical source stands against the pending journal's recorded fingerprints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceState {
    /// The file still has the bytes the request started from: nothing was written.
    AtBefore,
    /// The file holds the bytes the request would install.
    AtAfter,
    /// The file differs from both recorded fingerprints (changed from outside).
    Drifted,
    /// The registered source could not be opened or read.
    Unreadable,
    /// A cooperating writer holds the source lock; the original request may still be running.
    WriterBusy,
    /// The journal has no verified source location because its intent is missing or inconsistent.
    Unknown,
}

impl SourceState {
    fn as_str(self) -> &'static str {
        match self {
            Self::AtBefore => "at_before",
            Self::AtAfter => "at_after",
            Self::Drifted => "drifted",
            Self::Unreadable => "unreadable",
            Self::WriterBusy => "writer_busy",
            Self::Unknown => "unknown",
        }
    }

    fn parse(s: &str) -> PgResult<Self> {
        Ok(match s {
            "at_before" => Self::AtBefore,
            "at_after" => Self::AtAfter,
            "drifted" => Self::Drifted,
            "unreadable" => Self::Unreadable,
            "writer_busy" => Self::WriterBusy,
            "unknown" => Self::Unknown,
            _ => return Err(invalid()),
        })
    }
}

/// What the owner looked at and decided; every field must still hold when the journal is closed.
#[derive(Clone, Debug)]
pub struct AbandonRequest {
    pub request_id: String,
    /// `validated`, `source_written` or `pg_activating`, as inspected.
    pub expected_phase: String,
    /// The inspected raw journal CAS hash, or `unbound` only when no hash was recorded.
    pub expected_intent_hash: String,
    /// The inspected `source_state` (`unknown` when the intent cannot establish a source).
    pub expected_source_state: String,
    /// The inspected source fingerprint, or `None` when no source bytes could be fingerprinted.
    pub expected_source_fingerprint: Option<String>,
    /// Required unless the source is `at_before`: the file keeps whatever bytes it has.
    pub retain_source: bool,
    pub reason: String,
}

struct View {
    request_id: String,
    phase: String,
    /// Raw journal CAS value. This is not proof that the duplicated intent is valid.
    recorded_hash: Option<String>,
    intent: Option<Intent>,
    candidate_id: String,
    publish_receipt_id: String,
    before: String,
    after: String,
    affected: Value,
    dependency: Value,
    created_at: String,
    updated_at: String,
    decision: Option<Value>,
}

const SELECT: &str = "SELECT phase,intent_hash,intent_json,audit_receipt_id,candidate_id,candidate_digest,publish_receipt_id,
        before_fingerprint,after_fingerprint,affected_work_ids,dependency_work_ids,body_json,
        request_id,created_at::text,updated_at::text
     FROM awr_team.planning_writeback_journals";

fn view(row: &Row, tenant: &str, project: &str) -> PgResult<View> {
    let request: String = row.get(12);
    let recorded_hash: Option<String> = row.get(1);
    // A journal whose intent is missing or inconsistent stays observable, never upgraded.
    let intent = match journal::parse(row, tenant, project, &request) {
        Ok(saved) => Some(saved.intent),
        Err(PgError::WritebackRefused(_) | PgError::SourceDivergence) => None,
        Err(error) => return Err(error),
    };
    let body: Value = row.get(11);
    Ok(View {
        request_id: request,
        phase: row.get(0),
        recorded_hash,
        intent,
        candidate_id: row.get(4),
        publish_receipt_id: row.get(6),
        before: row.get(7),
        after: row.get(8),
        affected: row.get(9),
        dependency: row.get(10),
        created_at: row.get(13),
        updated_at: row.get(14),
        decision: journal::recorded_operator_decision(&body).cloned(),
    })
}

struct Observed {
    /// Held until the journal is closed so no cooperating writer can change the file in between.
    guard: Option<LockedSourceFile>,
    state: SourceState,
    fingerprint: Option<String>,
}

fn observe(intent: Option<&Intent>) -> Observed {
    let none = |state| Observed {
        guard: None,
        state,
        fingerprint: None,
    };
    let Some(intent) = intent else {
        return none(SourceState::Unknown);
    };
    let guard = match LockedSourceFile::open(
        &intent.request.source_root,
        &intent.request.ledger_relative_path,
    ) {
        Ok(guard) => guard,
        Err(awr_source::Error::SourceConflict(message)) if message.contains("busy") => {
            return none(SourceState::WriterBusy);
        }
        Err(_) => return none(SourceState::Unreadable),
    };
    let Ok(bytes) = guard.read() else {
        return Observed {
            guard: Some(guard),
            state: SourceState::Unreadable,
            fingerprint: None,
        };
    };
    let observed = fingerprint(&bytes);
    let state = if observed == intent.before_fingerprint {
        SourceState::AtBefore
    } else if observed == intent.after_fingerprint {
        SourceState::AtAfter
    } else {
        SourceState::Drifted
    };
    Observed {
        guard: Some(guard),
        state,
        fingerprint: Some(observed),
    }
}

fn item(view: &View, observed: Option<&Observed>) -> Value {
    let pending = PENDING.contains(&view.phase.as_str());
    let hash_token = match view.recorded_hash.as_deref() {
        Some(hash) if hex(hash) => Some(hash),
        None => Some("unbound"),
        Some(_) => None,
    };
    let malformed_hash = view.recorded_hash.is_some() && hash_token.is_none();
    let affected: Vec<&Value> = view
        .affected
        .as_array()
        .map(|a| a.iter().take(AFFECTED_SAMPLE).collect())
        .unwrap_or_default();
    let mut item = json!({
        "request_id": view.request_id, "phase": view.phase, "pending": pending,
        "original_intent_bound": view.intent.is_some(),
        "intent_hash": view.recorded_hash.as_deref().filter(|hash| hex(hash)),
        "candidate_id": view.candidate_id, "publish_receipt_id": view.publish_receipt_id,
        "before_fingerprint": view.before, "after_fingerprint": view.after,
        "affected_work_count": view.affected.as_array().map_or(0, Vec::len),
        "affected_work_ids_sample": affected,
        "dependency_work_count": view.dependency.as_array().map_or(0, Vec::len),
        "created_at": view.created_at, "updated_at": view.updated_at,
        "operator_decision": view.decision,
    });
    if let (true, Some(observed)) = (pending, observed) {
        let state = observed.state;
        item["source_state"] = json!(state.as_str());
        item["source_fingerprint"] = json!(observed.fingerprint);
        item["abandon"] = json!({
            "allowed": state != SourceState::WriterBusy && !malformed_hash,
            "blocked_by": if state == SourceState::WriterBusy {
                json!("a cooperating writer holds the source lock")
            } else if malformed_hash {
                json!(MALFORMED_HASH_BLOCK)
            } else {
                Value::Null
            },
            "retain_source_required": state != SourceState::AtBefore,
            "expected_phase": view.phase,
            "expected_intent_hash": hash_token,
            "expected_source_state": state.as_str(),
            "expected_source_fingerprint": observed.fingerprint,
        });
    }
    item
}

pub struct OperatorWriteback;

impl OperatorWriteback {
    /// Owner-only, read-only. Lists the pending journals of a project, or shows one request in any phase,
    /// with the physical source state each one would need to be closed against.
    pub async fn inspect(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: Option<&str>,
    ) -> PgResult<Value> {
        if ![tenant, project].iter().all(|s| identity(s)) || request.is_some_and(|r| !identity(r)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let operator = require_owner_project(&tx, tenant, project, false).await?;
        let rows = match request {
            Some(request) => {
                tx.query(
                    &format!("{SELECT} WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3"),
                    &[&tenant, &project, &request],
                )
                .await?
            }
            None => {
                tx.query(
                    &format!(
                        "{SELECT} WHERE tenant_id=$1 AND project_id=$2
                         AND phase IN ('validated','source_written','pg_activating')
                         ORDER BY created_at,request_id LIMIT $3"
                    ),
                    &[&tenant, &project, &(LIMIT + 1)],
                )
                .await?
            }
        };
        let views = rows
            .iter()
            .map(|row| view(row, tenant, project))
            .collect::<PgResult<Vec<_>>>()?;
        tx.commit().await?;
        let truncated = views.len() as i64 > LIMIT;
        // The source is read after the database transaction, one file lock at a time.
        let items: Vec<Value> = views
            .iter()
            .take(LIMIT as usize)
            .map(|v| {
                let observed = PENDING
                    .contains(&v.phase.as_str())
                    .then(|| observe(v.intent.as_ref()));
                item(v, observed.as_ref())
            })
            .collect();
        Ok(json!({
            "protocol": PROTOCOL, "protocol_version": 1, "read_only": true, "mutation": false,
            "authorization": "schema_owner_postgresql_role", "operator_role": operator,
            "tenant_id": tenant, "project_id": project, "source_modified": false,
            "items": items, "truncated": truncated,
            "next_step": "Resume the original request through the planning tools when it can still complete. Otherwise close it with writeback-abandon, passing the expected_* values of the item exactly as shown."
        }))
    }

    /// Owner-only. Closes one pending journal against the facts the owner inspected. It never edits the source
    /// file and never changes the active snapshot; see the module comment.
    pub async fn abandon(
        client: &mut Client,
        tenant: &str,
        project: &str,
        req: &AbandonRequest,
    ) -> PgResult<Value> {
        let reason = req.reason.trim();
        if ![tenant, project, req.request_id.as_str()]
            .iter()
            .all(|s| identity(s))
            || !PENDING.contains(&req.expected_phase.as_str())
            || reason.is_empty()
            || reason.chars().count() > MAX_REASON
            || reason.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let expected_state = SourceState::parse(&req.expected_source_state)?;
        let expected_fingerprint = match req.expected_source_fingerprint.as_deref() {
            Some(value) if source_fingerprint(value) => Some(value),
            None => None,
            Some(_) => return Err(invalid()),
        };
        if matches!(
            expected_state,
            SourceState::AtBefore | SourceState::AtAfter | SourceState::Drifted
        ) != expected_fingerprint.is_some()
        {
            return Err(invalid());
        }
        let expected_hash = match req.expected_intent_hash.as_str() {
            "unbound" => None,
            hash if hex(hash) => Some(hash),
            _ => return Err(invalid()),
        };
        crate::check_schema(client).await?;
        // The journal names the source file, so it is read first and the source lock is taken before the
        // database locks: nothing waits on the filesystem while the project row is held.
        let first = {
            let tx = client.transaction().await?;
            require_owner_project(&tx, tenant, project, false).await?;
            let row = tx
                .query_opt(
                    &format!("{SELECT} WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3"),
                    &[&tenant, &project, &req.request_id],
                )
                .await?;
            let first = row.map(|r| view(&r, tenant, project)).transpose()?;
            tx.commit().await?;
            first
        };
        let observed = match &first {
            Some(v) if PENDING.contains(&v.phase.as_str()) => observe(v.intent.as_ref()),
            _ => Observed {
                guard: None,
                state: SourceState::Unknown,
                fingerprint: None,
            },
        };
        if observed.state == SourceState::WriterBusy {
            return Err(PgError::WritebackRefused(
                "a cooperating writer holds the source lock, so the original request may still be running; retry after it has finished".into(),
            ));
        }
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, tenant, project, true).await?;
        let row = tx
            .query_opt(
                &format!(
                    "{SELECT} WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3 FOR UPDATE"
                ),
                &[&tenant, &project, &req.request_id],
            )
            .await?
            .ok_or_else(|| {
                PgError::WritebackRefused("no writeback journal exists for this request".into())
            })?;
        let now = view(&row, tenant, project)?;
        if let Some(decision) = &now.decision {
            if now.phase != "refused" {
                return Err(PgError::WritebackRefused(
                    "the recorded operator decision is inconsistent with its journal phase; inspect the retained journal before recovery".into(),
                ));
            }
            if !journal::valid_operator_decision(
                decision,
                &now.phase,
                &now.request_id,
                now.recorded_hash.as_deref(),
            ) {
                return Err(PgError::WritebackRefused(
                    "the recorded operator decision is malformed; inspect the retained journal before recovery".into(),
                ));
            }
            // The owner already closed it: report the recorded decision, change nothing.
            tx.commit().await?;
            return Ok(json!({"protocol": PROTOCOL, "replayed": true, "decision": decision}));
        }
        if !PENDING.contains(&now.phase.as_str()) {
            return Err(PgError::WritebackRefused(format!(
                "the journal is {} and no longer pending",
                now.phase
            )));
        }
        let same_journal = first.as_ref().is_some_and(|before| {
            before.phase == now.phase
                && before.recorded_hash == now.recorded_hash
                && before.intent.is_some() == now.intent.is_some()
        });
        if now.phase != req.expected_phase
            || !same_journal
            || observed.state != expected_state
            || observed.fingerprint.as_deref() != expected_fingerprint
        {
            return Err(PgError::PreconditionsChanged);
        }
        if now.recorded_hash.as_deref().is_some_and(|hash| !hex(hash)) {
            return Err(PgError::WritebackRefused(MALFORMED_HASH_BLOCK.into()));
        }
        if now.recorded_hash.as_deref() != expected_hash {
            return Err(PgError::PreconditionsChanged);
        }
        if observed.state != SourceState::AtBefore && !req.retain_source {
            return Err(PgError::WritebackRefused(format!(
                "the source is {}, not in the state the request started from; pass retain_source to leave it exactly as it is",
                observed.state.as_str()
            )));
        }
        let revision: i64 = tx
            .query_opt(
                "UPDATE awr_team.projects SET project_revision=project_revision+1
                 WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807
                 RETURNING project_revision",
                &[&tenant, &project],
            )
            .await?
            .ok_or(PgError::PreconditionsChanged)?
            .get(0);
        let decision = json!({
            "protocol": PROTOCOL, "action": "abandon", "request_id": req.request_id,
            "previous_phase": now.phase, "intent_hash": now.recorded_hash,
            "source_state": observed.state.as_str(), "source_fingerprint": observed.fingerprint,
            "source_retained": observed.state != SourceState::AtBefore,
            "operator_role": operator, "reason": reason, "project_revision": revision.to_string(),
        });
        let closed = tx
            .execute(
                "UPDATE awr_team.planning_writeback_journals
                 SET phase='refused',refuse_reason=$5,body_json=$6,updated_at=clock_timestamp()
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3 AND phase=$4
                   AND intent_hash IS NOT DISTINCT FROM $7",
                &[
                    &tenant,
                    &project,
                    &req.request_id,
                    &now.phase,
                    &format!("operator closed a pending source writeback: {reason}"),
                    &json!({"phase": "refused", "operator_decision": decision}),
                    &now.recorded_hash,
                ],
            )
            .await?;
        if closed != 1 {
            return Err(PgError::PreconditionsChanged);
        }
        // Readers of the event history see that it happened, not the owner's free text.
        let event = json!({
            "operator_role": operator, "request_id": req.request_id, "previous_phase": now.phase,
            "source_state": observed.state.as_str(), "source_retained": observed.state != SourceState::AtBefore,
            "reason_sha256": crate::digest_of(&json!(reason)),
        });
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
             VALUES($1,$2,$3,$4,0,'operator.planning_writeback_abandon',$5,$6)",
            &[
                &tenant,
                &project,
                &crate::tx::new_id(),
                &revision,
                &format!("operator:{operator}"),
                &event,
            ],
        )
        .await?;
        tx.commit().await?;
        drop(observed.guard);
        Ok(json!({
            "protocol": PROTOCOL, "replayed": false, "decision": decision,
            "journal_phase_after": "refused", "source_modified": false,
            "active_snapshot_modified": false, "execution_authorized": false, "state_basis": "at_commit",
            "next_step": if observed.state == SourceState::AtBefore {
                "The source is unchanged. The publication stays pending: submit a new reviewed request for it."
            } else {
                "The source file keeps its current bytes and the active snapshot is unchanged. Restore the file from version control, or ingest, approve and activate its current content through the normal source pipeline."
            },
        }))
    }
}
