//! Optional, caller-declared observations; never execution or billing authority.
use crate::{PgError, PgResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

pub(crate) const STALE_AFTER_MS: i64 = 15 * 60 * 1000;
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

/// Read independently timed summaries; an ordinary checkpoint must not erase
/// an earlier report or make an old usage measurement look freshly collected.
pub(crate) async fn observe(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    session: &str,
    contract: &str,
    observed: i64,
    data: &mut Value,
) -> PgResult<()> {
    let row = tx
        .query_one(
            "SELECT client_info_json,(extract(epoch FROM client_info_at)*1000)::bigint
        FROM awr_team.sessions WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &session],
        )
        .await?;
    let client: Value = row.get::<_, Option<Value>>(0).unwrap_or(Value::Null);
    let at: Option<i64> = row.get(1);
    data["client"] = client.clone();
    data["model"] = client["model"].clone();
    data["reporting"] = json!({"version":1,"provenance":"caller_declared",
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
    // Both column names are compile-time constants, never caller-provided SQL.
    for (field, column) in [("progress", "progress_json"), ("usage", "usage_json")] {
        let row = tx
            .query_opt(
                &format!(
                    "SELECT id,{column},contract_hash,next_action,
            (extract(epoch FROM created_at)*1000)::bigint,observed_revision FROM awr_team.checkpoints
            WHERE tenant_id=$1 AND project_id=$2 AND session_id=$3 AND {column} IS NOT NULL
            ORDER BY observed_revision DESC,created_at DESC,id DESC LIMIT 1"
                ),
                &[&tenant, &project, &session],
            )
            .await?;
        if let Some(row) = row {
            let reported: i64 = row.get(4);
            let current = row.get::<_, String>(2) == contract;
            let mut value: Value = row.get(1);
            let measured = value["observed_at_unix_ms"].as_i64().unwrap_or(reported);
            value["checkpoint_id"] = json!(row.get::<_, String>(0));
            value["reported_at_unix_ms"] = json!(reported);
            value["reported_contract_hash"] = json!(row.get::<_, String>(2));
            value["recorded_project_revision"] = json!(row.get::<_, i64>(5).to_string());
            // A server receive time is not a client observation. Progress v1
            // does not collect that time; usage preserves its supplied host time.
            value["client_observed_at_unix_ms"] = value["observed_at_unix_ms"].clone();
            value["report_order_basis"] = json!("recorded_project_revision");
            value["contract_matches_current"] = json!(current);
            value["stale"] = json!(!current || observed.saturating_sub(measured) > STALE_AFTER_MS);
            value["provenance"] = json!("caller_declared");
            if field == "progress" {
                value["next_action"] = json!(row.get::<_, String>(3));
            }
            data[field] = value;
            data["missing"][field] = Value::Null;
        }
    }
    Ok(())
}
