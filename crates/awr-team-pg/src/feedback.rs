//! Optional, caller-declared observations; never execution or billing authority.
use crate::{PgError, PgResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::{Row, Transaction};

pub(crate) const STALE_AFTER_MS: i64 = 15 * 60 * 1000;
pub(crate) const MAX_STRUCTURED_WAITS: usize = 8;
pub(crate) const MAX_WAIT_PARTY_NAME_CHARS: usize = 80;
pub(crate) const MAX_WAIT_TEXT_CHARS: usize = 200;
pub(crate) const WAIT_PARTY_KINDS: &[&str] = &["person", "group", "external_party"];
pub(crate) const PROGRESS_PHASES: &[&str] = &[
    "starting",
    "implementing",
    "testing",
    "waiting_user",
    "blocked",
    "ready_for_review",
    "waiting_dependency",
    "reviewing",
    "reworking",
    "integrating",
    "delivered",
];

#[derive(Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Support {
    Supported,
    Unsupported,
    #[default]
    Unknown,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Capabilities {
    #[serde(default)]
    model: Support,
    #[serde(default)]
    usage: Support,
    #[serde(default)]
    progress: Support,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClientInfo {
    product: String,
    version: Option<String>,
    model: Option<Model>,
    #[serde(default)]
    capabilities: Capabilities,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Model {
    id: String,
    provider: Option<String>,
    source: ModelSource,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ModelSource {
    HostMetadata,
    ClientConfiguration,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Starting,
    Implementing,
    Testing,
    WaitingUser,
    Blocked,
    ReadyForReview,
    WaitingDependency,
    Reviewing,
    Reworking,
    Integrating,
    Delivered,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WaitPartyKind {
    Person,
    Group,
    ExternalParty,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitParty {
    pub(crate) kind: WaitPartyKind,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct StructuredWait {
    pub(crate) waiting_on: WaitParty,
    pub(crate) what: String,
    pub(crate) since_unix_ms: i64,
    pub(crate) release_condition: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Progress {
    phase: Phase,
    summary: String,
    #[serde(default)]
    completed: Vec<String>,
    #[serde(default)]
    blockers: Vec<String>,
    #[serde(default)]
    artifacts: Vec<Reference>,
    #[serde(default)]
    tests: Vec<TestReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    waits: Option<Vec<StructuredWait>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    label: String,
    reference: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TestReport {
    name: String,
    outcome: TestOutcome,
    reference: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TestOutcome {
    Passed,
    Failed,
    NotRun,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Usage {
    source: String,
    source_ref: String,
    counter_id: String,
    scope: UsageScope,
    coverage: Coverage,
    observed_at_unix_ms: i64,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
}

// Only an explicitly identified host session counter is accepted in v1. It may
// include other work: no task allocation, counter summation or billing is implied.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum UsageScope {
    HostSession,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Coverage {
    Complete,
    Partial,
    Unknown,
}

fn invalid() -> PgError {
    PgError::Protocol("invalid feedback: check field bounds, capability declarations and host counter scope; omit unavailable observations".into())
}
fn text(s: &str, max: usize) -> bool {
    !s.trim().is_empty()
        && s.len() <= max
        && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}
fn bounded_chars(s: &str, max_chars: usize) -> bool {
    !s.trim().is_empty() && s.chars().count() <= max_chars && !s.chars().any(char::is_control)
}
fn public<T: Serialize>(v: &T) -> PgResult<()> {
    awr_core::ensure_public_data(v).map_err(|_| {
        PgError::Protocol("feedback must not contain credentials or private data; submit a redacted business summary".into())
    })
}

impl ClientInfo {
    pub(crate) fn validate(&self) -> PgResult<()> {
        if !text(&self.product, 128)
            || self.version.as_ref().is_some_and(|v| !text(v, 128))
            || self.model.as_ref().is_some_and(|m| {
                !text(&m.id, 128)
                    || m.provider.as_ref().is_some_and(|p| !text(p, 128))
                    || self.capabilities.model == Support::Unsupported
            })
        {
            return Err(invalid());
        }
        public(self)
    }
}
impl StructuredWait {
    pub(crate) fn is_valid(&self) -> bool {
        bounded_chars(&self.waiting_on.name, MAX_WAIT_PARTY_NAME_CHARS)
            && bounded_chars(&self.what, MAX_WAIT_TEXT_CHARS)
            && self.since_unix_ms > 0
            && self.since_unix_ms <= 9_007_199_254_740_991
            && bounded_chars(&self.release_condition, MAX_WAIT_TEXT_CHARS)
    }
}
impl Progress {
    pub(crate) fn validate(&self) -> PgResult<()> {
        if !text(&self.summary, 2048)
            || [&self.completed, &self.blockers]
                .iter()
                .any(|items| items.len() > 8 || items.iter().any(|s| !text(s, 1024)))
            || self.artifacts.len() > 8
            || self
                .artifacts
                .iter()
                .any(|r| !text(&r.label, 256) || !text(&r.reference, 1024))
            || self.tests.len() > 8
            || self.tests.iter().any(|t| {
                !text(&t.name, 256) || t.reference.as_ref().is_some_and(|r| !text(r, 1024))
            })
            || self.waits.as_ref().is_some_and(|items| {
                items.len() > MAX_STRUCTURED_WAITS || items.iter().any(|w| !w.is_valid())
            })
        {
            return Err(invalid());
        }
        public(self)
    }
}
impl Usage {
    pub(crate) fn validate(&self) -> PgResult<()> {
        let counters = [
            self.input_tokens,
            self.output_tokens,
            self.cached_input_tokens,
        ];
        if !text(&self.source, 128)
            || !text(&self.source_ref, 1024)
            || !text(&self.counter_id, 128)
            || self.observed_at_unix_ms <= 0
            || self.observed_at_unix_ms > 9_007_199_254_740_991
            || (self.input_tokens.is_none() && self.output_tokens.is_none())
            || counters
                .iter()
                .flatten()
                .any(|n| *n > 9_007_199_254_740_991)
            || self
                .cached_input_tokens
                .is_some_and(|n| self.input_tokens.is_none_or(|i| n > i))
        {
            return Err(invalid());
        }
        public(self)
    }

    pub(crate) async fn check_order(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        session: &str,
    ) -> PgResult<()> {
        let now: i64 = tx
            .query_one(
                "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
                &[],
            )
            .await?
            .get(0);
        if self.observed_at_unix_ms > now + 300_000 {
            return Err(invalid());
        }
        let last = tx
            .query_opt(
                "SELECT usage_json FROM awr_team.checkpoints
            WHERE tenant_id=$1 AND project_id=$2 AND session_id=$3 AND usage_json IS NOT NULL
            ORDER BY observed_revision DESC,created_at DESC,id DESC LIMIT 1",
                &[&tenant, &project, &session],
            )
            .await?;
        if let Some(row) = last {
            let old: Value = row.get(0);
            let old_time = old["observed_at_unix_ms"].as_i64().unwrap_or(i64::MAX);
            if self.observed_at_unix_ms < old_time
                || (self.observed_at_unix_ms == old_time && json!(self) != old)
            {
                return Err(PgError::Protocol(
                    "out-of-order host observation; retain the latest measured snapshot".into(),
                ));
            }
        }
        // Null dimensions are missing observations, not counter resets. Compare
        // against every prior reported value so omission cannot bridge a decrease.
        let previous = tx.query_one("SELECT max((usage_json->>'input_tokens')::bigint),
            max((usage_json->>'output_tokens')::bigint),max((usage_json->>'cached_input_tokens')::bigint)
            FROM awr_team.checkpoints WHERE tenant_id=$1 AND project_id=$2 AND session_id=$3
              AND usage_json IS NOT NULL AND usage_json->>'source'=$4 AND usage_json->>'counter_id'=$5",
            &[&tenant,&project,&session,&self.source,&self.counter_id]).await?;
        for (index, current) in [
            self.input_tokens,
            self.output_tokens,
            self.cached_input_tokens,
        ]
        .iter()
        .enumerate()
        {
            if current
                .zip(previous.get::<_, Option<i64>>(index))
                .is_some_and(|(new, old)| new < old as u64)
            {
                return Err(PgError::Protocol("decreasing host counter; refresh the observation or identify the reset with a new counter_id".into()));
            }
        }
        Ok(())
    }
}

pub(crate) async fn validate_support(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    session: &str,
    info: Option<&ClientInfo>,
    progress: bool,
    usage: bool,
) -> PgResult<()> {
    let client = if let Some(info) = info {
        json!(info)
    } else {
        tx.query_one(
            "SELECT client_info_json FROM awr_team.sessions
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &session],
        )
        .await?
        .get::<_, Option<Value>>(0)
        .unwrap_or(Value::Null)
    };
    if (progress && client["capabilities"]["progress"] == "unsupported")
        || (usage && client["capabilities"]["usage"] == "unsupported")
    {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn missing(client: &Value, capability: &str) -> &'static str {
    match client["capabilities"][capability].as_str() {
        Some("unsupported") => "client_collection_unsupported",
        Some("supported") => "not_reported_by_client",
        _ => "client_capability_unknown",
    }
}

/// Select each work's latest actual report, independently of its latest session.
/// Callers supply only already-authorized current work/ownership pairs.
pub(crate) async fn report_rows(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    stream: &str,
    works: &[(String, i64)],
    requested_session: Option<&str>,
    field: &str,
) -> PgResult<Vec<Row>> {
    let column = match field {
        "progress" => "progress_json",
        "usage" => "usage_json",
        _ => return Err(invalid()),
    };
    let bindings = json!(
        works
            .iter()
            .map(|(work, ownership)| json!({"work_id":work,"ownership_version":ownership}))
            .collect::<Vec<_>>()
    );
    tx.query(
        &format!(
            "SELECT DISTINCT ON (s.work_id) c.id,c.{column},c.contract_hash,c.next_action,
        (extract(epoch FROM c.created_at)*1000)::bigint,c.observed_revision,
        s.id,s.actor_id,s.client_id,a.display_name,s.work_id
        FROM awr_team.checkpoints c JOIN awr_team.sessions s
          ON (s.tenant_id,s.project_id,s.id)=(c.tenant_id,c.project_id,c.session_id)
        JOIN jsonb_to_recordset($4) AS b(work_id text,ownership_version bigint)
          ON b.work_id=s.work_id AND b.ownership_version=s.ownership_version
        LEFT JOIN awr_team.actors a ON a.tenant_id=s.tenant_id AND a.id=s.actor_id
        WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.scope_id='main'
          AND s.workstream_id=$3 AND ($5::text IS NULL OR s.id=$5)
          AND c.{column} IS NOT NULL
        ORDER BY s.work_id,c.observed_revision DESC,c.created_at DESC,c.id DESC"
        ),
        &[&tenant, &project, &stream, &bindings, &requested_session],
    )
    .await
    .map_err(Into::into)
}

pub(crate) fn report_value(row: &Row, field: &str, contract: &str, observed: i64) -> Value {
    let reported: i64 = row.get(4);
    let current = row.get::<_, String>(2) == contract;
    let mut value: Value = row.get(1);
    let measured = value["observed_at_unix_ms"].as_i64().unwrap_or(reported);
    value["checkpoint_id"] = json!(row.get::<_, String>(0));
    value["reported_at_unix_ms"] = json!(reported);
    value["reported_contract_hash"] = json!(row.get::<_, String>(2));
    value["recorded_project_revision"] = json!(row.get::<_, i64>(5).to_string());
    value["client_observed_at_unix_ms"] = value["observed_at_unix_ms"].clone();
    value["report_order_basis"] = json!("recorded_project_revision");
    value["contract_matches_current"] = json!(current);
    value["stale"] = json!(!current || observed.saturating_sub(measured) > STALE_AFTER_MS);
    value["provenance"] = json!("caller_declared");
    value["origin"] = json!({"session_id":row.get::<_,String>(6),
        "actor_id":row.get::<_,String>(7),"client_id":row.get::<_,String>(8),
        "actor_name":row.get::<_,Option<String>>(9)});
    if field == "progress" {
        value["next_action"] = json!(row.get::<_, String>(3));
    }
    value
}

pub(crate) fn structured_waits_not_provided() -> Value {
    json!({
        "status": "not_provided",
        "items": Value::Null,
        "provenance": "caller_declared",
        "grants_authority": false,
        "checkpoint_id": Value::Null,
        "reported_at_unix_ms": Value::Null,
        "contract_matches_current": Value::Null,
    })
}

pub(crate) fn structured_waits_from_progress_json(
    progress: &Value,
    checkpoint_id: &str,
    reported_at_unix_ms: i64,
    contract_matches_current: bool,
) -> Value {
    let items = match progress.get("waits") {
        Some(Value::Array(raw)) => {
            serde_json::from_value::<Vec<StructuredWait>>(Value::Array(raw.clone()))
                .ok()
                .filter(|waits| {
                    waits.len() <= MAX_STRUCTURED_WAITS
                        && waits.iter().all(StructuredWait::is_valid)
                })
        }
        _ => None,
    };
    match items {
        Some(waits) => json!({
            "status": "provided",
            "items": waits,
            "provenance": "caller_declared",
            "grants_authority": false,
            "checkpoint_id": checkpoint_id,
            "reported_at_unix_ms": reported_at_unix_ms,
            "contract_matches_current": contract_matches_current,
        }),
        None => json!({
            "status": "not_provided",
            "items": Value::Null,
            "provenance": "caller_declared",
            "grants_authority": false,
            "checkpoint_id": checkpoint_id,
            "reported_at_unix_ms": reported_at_unix_ms,
            "contract_matches_current": contract_matches_current,
        }),
    }
}

pub(crate) fn structured_waits_from_row(row: &Row, contract: &str) -> Value {
    let checkpoint_id: String = row.get(0);
    let progress: Value = row.get(1);
    let reported_contract: String = row.get(2);
    let reported_at_unix_ms: i64 = row.get(4);
    structured_waits_from_progress_json(
        &progress,
        &checkpoint_id,
        reported_at_unix_ms,
        reported_contract == contract,
    )
}

/// Read independently timed summaries; an ordinary checkpoint must not erase
/// an earlier report or make an old usage measurement look freshly collected.
pub(crate) async fn observe(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    session: &str,
    work: &str,
    stream: &str,
    ownership: i64,
    requested_session: Option<&str>,
    contract: &str,
    observed: i64,
    data: &mut Value,
) -> PgResult<()> {
    let row = tx
        .query_one(
            "SELECT s.client_info_json,(extract(epoch FROM s.client_info_at)*1000)::bigint,
                s.actor_id,s.client_id,a.display_name
        FROM awr_team.sessions s LEFT JOIN awr_team.actors a ON a.tenant_id=s.tenant_id AND a.id=s.actor_id
        WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.id=$3",
            &[&tenant, &project, &session],
        )
        .await?;
    let client: Value = row.get::<_, Option<Value>>(0).unwrap_or(Value::Null);
    let at: Option<i64> = row.get(1);
    data["client"] = client.clone();
    data["model"] = client["model"].clone();
    data["reporting"] = json!({"version":1,"provenance":"caller_declared",
        "client_origin":{"session_id":session,"actor_id":row.get::<_,String>(2),
            "client_id":row.get::<_,String>(3),"actor_name":row.get::<_,Option<String>>(4)},
        "model_basis":"latest_session_declaration; not execution-time attribution",
        "client_reported_at_unix_ms":at,"stale_after_ms":STALE_AFTER_MS,
        "client_stale":at.is_some_and(|at| observed.saturating_sub(at)>STALE_AFTER_MS),
        "billing_collected":false,"usage_aggregation":"none"});
    for field in ["model", "usage", "progress"] {
        data["missing"][field] = if data[field].is_null() {
            json!(missing(&client, field))
        } else {
            Value::Null
        };
    }
    for field in ["progress", "usage"] {
        if let Some(row) = report_rows(
            tx,
            tenant,
            project,
            stream,
            &[(work.into(), ownership)],
            requested_session,
            field,
        )
        .await?
        .first()
        {
            data[field] = report_value(row, field, contract, observed);
            data["missing"][field] = Value::Null;
            if field == "progress" {
                data["structured_waits"] = structured_waits_from_row(row, contract);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_progress(waits: Value) -> Value {
        json!({
            "phase": "blocked",
            "summary": "Waiting on external confirmation before continuing.",
            "waits": waits,
        })
    }

    fn wait(kind: &str, name: &str, what: &str, since_unix_ms: i64, release: &str) -> Value {
        json!({
            "waiting_on": { "kind": kind, "name": name },
            "what": what,
            "since_unix_ms": since_unix_ms,
            "release_condition": release,
        })
    }

    #[test]
    fn structured_wait_bounds_and_party_kinds_are_enforced() {
        let name_80 = "名".repeat(MAX_WAIT_PARTY_NAME_CHARS);
        let desc_200 = "说".repeat(MAX_WAIT_TEXT_CHARS);
        for kind in ["person", "group", "external_party"] {
            let p: Progress = serde_json::from_value(valid_progress(json!([wait(
                kind,
                &name_80,
                &desc_200,
                1_731_542_400_000,
                &desc_200,
            )])))
            .unwrap();
            assert!(p.validate().is_ok(), "kind {kind} at max bounds must pass");
        }

        // Explicit empty array clears waits cleanly.
        let cleared: Progress = serde_json::from_value(valid_progress(json!([]))).unwrap();
        assert!(cleared.validate().is_ok());
        assert_eq!(json!(cleared)["waits"], json!([]));

        // Omitted waits stays absent in serialized JSON so legacy reports remain distinct.
        let omitted: Progress = serde_json::from_value(json!({
            "phase": "implementing",
            "summary": "Working on the storage module."
        }))
        .unwrap();
        assert!(omitted.validate().is_ok());
        assert!(json!(omitted).get("waits").is_none());

        // Unknown waiting_on.kind must fail deserialization.
        for bad_kind in ["team", "organization", "agent", "vendor", ""] {
            let raw = valid_progress(json!([wait(
                bad_kind,
                "External Ops",
                "Provision sandbox account",
                1_731_542_400_000,
                "Sandbox account credentials shared via secure channel",
            )]));
            assert!(
                serde_json::from_value::<Progress>(raw).is_err(),
                "unknown kind {bad_kind} must be rejected"
            );
        }

        // Oversized name (81 chars), oversized descriptions (201 chars), blank strings,
        // control characters, invalid timestamps, credential leaks, and > 8 items must fail.
        let name_81 = "名".repeat(MAX_WAIT_PARTY_NAME_CHARS + 1);
        let desc_201 = "说".repeat(MAX_WAIT_TEXT_CHARS + 1);
        let bad_cases = [
            json!([wait("external_party", &name_81, "What", 1000, "Release")]),
            json!([wait("external_party", "Vendor", &desc_201, 1000, "Release")]),
            json!([wait("external_party", "Vendor", "What", 1000, &desc_201)]),
            json!([wait("person", "   ", "What", 1000, "Release")]),
            json!([wait("group", "Ops", "   ", 1000, "Release")]),
            json!([wait("group", "Ops", "What", 1000, "   ")]),
            json!([wait("person", "Alex\nNext", "What", 1000, "Release")]),
            json!([wait("person", "Alex", "Line1\nLine2", 1000, "Release")]),
            json!([wait("person", "Alex", "What", 1000, "Cond\t1")]),
            json!([wait("person", "Alex", "What", 0, "Release")]),
            json!([wait("person", "Alex", "What", -1, "Release")]),
            json!([wait(
                "person",
                "Alex",
                "What",
                9_007_199_254_740_992,
                "Release"
            )]),
            json!([wait(
                "external_party",
                "Vendor",
                &format!("token ghp_{}", "a".repeat(36)),
                1000,
                "Release"
            )]),
            json!(
                (0..=MAX_STRUCTURED_WAITS)
                    .map(|i| wait(
                        "external_party",
                        &format!("Party {i}"),
                        "What",
                        1000 + i as i64,
                        "Release"
                    ))
                    .collect::<Vec<_>>()
            ),
        ];
        for case in bad_cases {
            let p: Progress = serde_json::from_value(valid_progress(case.clone())).unwrap();
            assert!(p.validate().is_err(), "invalid wait must fail: {case}");
        }
    }

    #[test]
    fn legacy_report_reads_as_not_provided_distinct_from_empty_waits() {
        let legacy = json!({
            "phase": "testing",
            "summary": "Legacy report without waits field."
        });
        let legacy_view = structured_waits_from_progress_json(&legacy, "cp-1", 1000, true);
        assert_eq!(legacy_view["status"], "not_provided");
        assert!(legacy_view["items"].is_null());
        assert_eq!(legacy_view["grants_authority"], false);
        assert_eq!(legacy_view["checkpoint_id"], "cp-1");

        let explicit_empty = json!({
            "phase": "testing",
            "summary": "Waits cleared.",
            "waits": []
        });
        let empty_view = structured_waits_from_progress_json(&explicit_empty, "cp-2", 2000, true);
        assert_eq!(empty_view["status"], "provided");
        assert_eq!(empty_view["items"], json!([]));
        assert_eq!(empty_view["grants_authority"], false);
    }
}
