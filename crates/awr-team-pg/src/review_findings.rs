//! Findings of a Team review, recorded under the verified identity of the reviewer. The severity
//! comes from the record (see `awr_team::judge_finding`), never from the reviewer's say-so: only a
//! blocking finding returns the work, every other finding is a deferred record. For governed work a
//! return needs a blocking finding, an approval resolves the blocking findings that were open, and
//! a completion needs none open. Other work keeps its review semantics and may still record them.
use crate::{PgError, PgResult};
use awr_core::DeliveryTier;
use awr_team::{JudgedFinding, ReviewFinding, WorkContract, judge_finding};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio_postgres::Transaction;

/// How an approval deals with a blocking finding that was open.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Resolution {
    pub finding_key: String,
    pub resolution: String,
}

pub(crate) struct Recorded {
    pub recorded: Vec<JudgedFinding>,
    pub resolved: Vec<String>,
    pub open_blocking: Vec<String>,
}

impl Recorded {
    pub(crate) fn summary(&self) -> Value {
        json!({
            "recorded": self.recorded.iter().map(|j| json!({
                "finding_key": j.finding.finding_key, "severity": j.severity,
                "disposition": j.disposition, "basis": j.basis,
            })).collect::<Vec<_>>(),
            "resolved": self.resolved,
            "open_blocking": self.open_blocking,
        })
    }
}

fn invalid() -> PgError {
    PgError::invalid_command_fields()
}

/// The keys of the blocking findings of `work` that no decision has resolved yet.
pub(crate) async fn open_blocking(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<BTreeSet<String>> {
    Ok(tx
        .query(
            "SELECT f.finding_key FROM awr_team.review_findings f
            WHERE f.tenant_id=$1 AND f.project_id=$2 AND f.work_id=$3
              AND f.action='record' AND f.severity='blocking'
              AND NOT EXISTS(SELECT 1 FROM awr_team.review_findings r
                WHERE r.tenant_id=f.tenant_id AND r.project_id=f.project_id
                  AND r.work_id=f.work_id AND r.finding_key=f.finding_key AND r.action='resolve')",
            &[&tenant, &project, &work],
        )
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect())
}

/// The tier adopted for the criteria of `work` when it is governed, or none when it is not.
async fn governed_tier(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
) -> PgResult<Option<DeliveryTier>> {
    let Some((status, _, governed)) =
        crate::governance::status(tx, tenant, project, snapshot, work).await?
    else {
        return Ok(None);
    };
    if !status.governed {
        return Ok(None);
    }
    Ok(Some(
        governed
            .work
            .definition
            .as_ref()
            .and_then(|d| d.tier)
            .unwrap_or(DeliveryTier::Required),
    ))
}

/// Records the findings and resolutions of one decision of round `round_index` and enforces what
/// governed work requires of a return or an approval.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
    round_id: &str,
    round_index: i32,
    contract: &WorkContract,
    decision: &str,
    reviewer_actor: &str,
    reviewer_person: &str,
    findings: &[ReviewFinding],
    resolutions: &[Resolution],
) -> PgResult<Recorded> {
    let tier = governed_tier(tx, tenant, project, snapshot, work).await?;
    let unresolved = open_blocking(tx, tenant, project, work).await?;
    let round = u32::try_from(round_index).map_err(|_| invalid())?;
    let mut keys = BTreeSet::new();
    let mut recorded = Vec::new();
    for finding in findings {
        if !keys.insert(finding.finding_key.clone()) {
            return Err(invalid());
        }
        let taken: bool = tx
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM awr_team.review_findings
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND finding_key=$4 AND action='record')",
                &[&tenant, &project, &work, &finding.finding_key],
            )
            .await?
            .get(0);
        if taken {
            // A finding key names one finding of the work for good.
            return Err(invalid());
        }
        let judged = judge_finding(
            finding,
            round,
            &contract.acceptance,
            &contract.hard_rules,
            tier.unwrap_or(DeliveryTier::Required),
            &unresolved,
        )
        .map_err(|_| invalid())?;
        tx.execute(
            "INSERT INTO awr_team.review_findings(tenant_id,project_id,id,work_id,review_round_id,round_index,
                finding_key,action,severity,disposition,basis,input_json,reviewer_actor_id,reviewer_person_id)
            VALUES($1,$2,$3,$4,$5,$6,$7,'record',$8,$9,$10,$11,$12,$13)",
            &[
                &tenant,
                &project,
                &crate::tx::new_id(),
                &work,
                &round_id,
                &round_index,
                &finding.finding_key,
                &judged.severity,
                &judged.disposition,
                &judged.basis,
                &json!(finding),
                &reviewer_actor,
                &reviewer_person,
            ],
        )
        .await?;
        recorded.push(judged);
    }
    let mut resolved = Vec::new();
    if !resolutions.is_empty() && decision != "approve" {
        // Only an approval can say that a blocking finding was dealt with.
        return Err(invalid());
    }
    for item in resolutions {
        let text = item.resolution.as_str();
        if !unresolved.contains(&item.finding_key)
            || resolved.contains(&item.finding_key)
            || text.trim().is_empty()
            || text.len() > 2048
            || text.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        tx.execute(
            "INSERT INTO awr_team.review_findings(tenant_id,project_id,id,work_id,review_round_id,round_index,
                finding_key,action,input_json,resolution,reviewer_actor_id,reviewer_person_id)
            VALUES($1,$2,$3,$4,$5,$6,$7,'resolve',$8,$9,$10,$11)",
            &[
                &tenant,
                &project,
                &crate::tx::new_id(),
                &work,
                &round_id,
                &round_index,
                &item.finding_key,
                &json!({"finding_key": item.finding_key}),
                &item.resolution,
                &reviewer_actor,
                &reviewer_person,
            ],
        )
        .await?;
        resolved.push(item.finding_key.clone());
    }
    let mut open: BTreeSet<String> = unresolved;
    open.extend(
        recorded
            .iter()
            .filter(|j| j.severity == "blocking")
            .map(|j| j.finding.finding_key.clone()),
    );
    for key in &resolved {
        open.remove(key);
    }
    if tier.is_some() {
        if decision == "approve" && !open.is_empty() {
            return Err(PgError::FindingsBlock {
                detail: format!(
                    "an approval resolves every open blocking finding; still open: {}",
                    open.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            });
        }
        if decision != "approve" && open.is_empty() {
            return Err(PgError::FindingsBlock {
                detail: "a return needs a blocking finding: a required criterion or a hard rule broken and shown with a counterexample, or a stated security or data-loss risk; other findings are deferred records".into(),
            });
        }
    }
    Ok(Recorded {
        recorded,
        resolved,
        open_blocking: open.into_iter().collect(),
    })
}

/// A completion of governed work needs no blocking finding open.
pub(crate) async fn require_none_open(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
) -> PgResult<()> {
    if governed_tier(tx, tenant, project, snapshot, work)
        .await?
        .is_none()
    {
        return Ok(());
    }
    let open = open_blocking(tx, tenant, project, work).await?;
    if open.is_empty() {
        return Ok(());
    }
    Err(PgError::FindingsBlock {
        detail: format!(
            "blocking findings are open: {}",
            open.into_iter().collect::<Vec<_>>().join(", ")
        ),
    })
}

/// What a re-review covers: the blocking findings still open and whether the bundle changed since
/// the previous round. None for the first round.
pub(crate) async fn re_review(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    round_index: i32,
    bundle_hash: &str,
) -> PgResult<Option<Value>> {
    let Some(previous) = tx
        .query_opt(
            "SELECT id, bundle_hash FROM awr_team.review_rounds
            WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND round_index < $4
            ORDER BY round_index DESC LIMIT 1",
            &[&tenant, &project, &work, &round_index],
        )
        .await?
    else {
        return Ok(None);
    };
    let previous_bundle: String = previous.get(1);
    Ok(Some(json!({
        "previous_round_id": previous.get::<_, String>(0),
        "previous_bundle_hash": previous_bundle,
        "bundle_changed": previous_bundle != bundle_hash,
        "unresolved_blocking": open_blocking(tx, tenant, project, work).await?,
        "covers": "the unresolved blocking findings and the delta since the previous bundle",
    })))
}

/// The findings of a work as the record leaves them, for inspection; none when there are none.
pub(crate) async fn block(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<Option<Value>> {
    let rows = tx
        .query(
            "SELECT finding_key, action, severity, disposition, basis, input_json, resolution,
                    round_index, reviewer_actor_id
            FROM awr_team.review_findings
            WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
            ORDER BY created_at, id",
            &[&tenant, &project, &work],
        )
        .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let resolved: BTreeSet<String> = rows
        .iter()
        .filter(|r| r.get::<_, String>(1) == "resolve")
        .map(|r| r.get(0))
        .collect();
    let mut findings = Vec::new();
    let mut open_blocking = Vec::new();
    let mut deferred = Vec::new();
    for row in rows.iter().filter(|r| r.get::<_, String>(1) == "record") {
        let key: String = row.get(0);
        let severity: String = row.get(2);
        let state = if resolved.contains(&key) {
            "resolved"
        } else {
            "open"
        };
        if state == "open" && severity == "blocking" {
            open_blocking.push(key.clone());
        }
        if severity != "blocking" {
            deferred.push(key.clone());
        }
        let input: Value = row.get(5);
        findings.push(json!({
            "finding_key": key, "round": row.get::<_, i32>(7), "severity": severity,
            "disposition": row.get::<_, String>(3), "basis": row.get::<_, String>(4),
            "state": state, "summary": input["summary"], "criterion": input["criterion"],
            "rule": input["rule"], "counterexample": input["counterexample"], "risk": input["risk"],
            "recorded_by": row.get::<_, String>(8),
        }));
    }
    Ok(Some(json!({
        "findings": findings, "open_blocking": open_blocking, "deferred": deferred,
        "basis": "A finding's severity is derived from the record: only a blocking finding returns the work, the rest are deferred records that block nothing.",
    })))
}
