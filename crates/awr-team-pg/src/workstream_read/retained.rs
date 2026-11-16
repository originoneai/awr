//! Display-only metadata retained during personal-project migration.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn checked_json(row: &tokio_postgres::Row) -> PgResult<Value> {
    let id: String = row.get(0);
    let bytes: Option<Vec<u8>> = row.get(3);
    let bytes = bytes.ok_or_else(|| PgError::SnapshotDrift(id.clone()))?;
    if row.get::<_, String>(2) != "finalized"
        || row.get::<_, String>(1) != crate::source::sha256_hex(&bytes)
        || row.get::<_, i64>(4) != bytes.len() as i64
        || bytes.len() > WorkstreamQuery::MAX_ARTIFACT_BYTES
    {
        return Err(PgError::SnapshotDrift(id));
    }
    serde_json::from_slice(&bytes).map_err(|_| PgError::SnapshotDrift(id))
}

fn historical_owner<'a>(value: &'a Value, work: &str) -> Option<&'a str> {
    if value["format"] != "awr-personal-history-v1"
        || value["work_id"] != work
        || value["historical_only"] != true
        || value["live_claims_imported"] != false
    {
        return None;
    }
    value["original_source_row"]["owner"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Call only with the already-authorized page. A current responsibility row,
/// including an explicit unassignment, always takes precedence over history.
pub(super) async fn owners(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    works: &[String],
) -> PgResult<BTreeMap<String, Value>> {
    let rows = tx
        .query(
            "SELECT DISTINCT a.id,a.sha256,a.state,a.content,a.byte_length,e.work_id
         FROM awr_team.artifacts a JOIN awr_team.evidence e
           ON (e.tenant_id,e.project_id,e.artifact_id)=(a.tenant_id,a.project_id,a.id)
         WHERE a.tenant_id=$1 AND a.project_id=$2 AND e.work_id=ANY($3)
           AND a.id='personal-history-' || e.work_id
           AND NOT EXISTS(SELECT 1 FROM awr_team.task_responsibilities t
             WHERE (t.tenant_id,t.project_id,t.work_id)=(e.tenant_id,e.project_id,e.work_id))",
            &[&tenant, &project, &works],
        )
        .await?;
    let mut result = BTreeMap::new();
    for row in rows {
        let work: String = row.get(5);
        let value = checked_json(&row)?;
        if let Some(name) = historical_owner(&value, &work) {
            result.insert(
                work,
                json!({"name":name,"provenance":"historical",
                "artifact_id":row.get::<_,String>(0),"sha256":row.get::<_,String>(1)}),
            );
        }
    }
    Ok(result)
}

/// The global retained catalog has its own evidence/work authorization. Only
/// names referenced by this authorized graph page are returned to its caller.
pub(super) async fn goal_titles(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    goals: &BTreeSet<String>,
) -> PgResult<BTreeMap<String, String>> {
    if goals.is_empty() {
        return Ok(BTreeMap::new());
    }
    let streams: Vec<String> = auth
        .catalog
        .workstreams
        .iter()
        .filter(|s| {
            auth.access
                .authorize(&auth.catalog, s.id, WorkstreamAction::Read)
                .is_ok()
        })
        .map(|s| s.id.to_string())
        .collect();
    let row = tx
        .query_opt(
            "SELECT DISTINCT a.id,a.sha256,a.state,a.content,a.byte_length
         FROM awr_team.artifacts a JOIN awr_team.evidence e
           ON (e.tenant_id,e.project_id,e.artifact_id)=(a.tenant_id,a.project_id,a.id)
         JOIN awr_team.workstream_snapshot_ownership o
           ON (o.tenant_id,o.project_id,o.work_id)=(e.tenant_id,e.project_id,e.work_id)
         WHERE a.tenant_id=$1 AND a.project_id=$2 AND a.id='personal-project-goals'
           AND o.snapshot_id=$3 AND o.workstream_id=ANY($4)",
            &[&tenant, &project, &auth.snapshot, &streams],
        )
        .await?;
    let Some(row) = row else {
        return Ok(BTreeMap::new());
    };
    let value = checked_json(&row)?;
    let mut result = BTreeMap::new();
    if let Some(entries) = value.as_array() {
        for entry in entries {
            if entry["project_id"] != project {
                continue;
            }
            let Some(title) = entry["title"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            for key in ["id", "external_key"] {
                if let Some(id) = entry[key].as_str().filter(|id| goals.contains(*id)) {
                    result.insert(id.to_owned(), title.to_owned());
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_only_display_attribution_for_its_exact_work() {
        let mut value = json!({"format":"awr-personal-history-v1","work_id":"a",
            "historical_only":true,"live_claims_imported":false,
            "original_source_row":{"owner":" historic-developer "}});
        assert_eq!(historical_owner(&value, "a"), Some("historic-developer"));
        assert_eq!(historical_owner(&value, "other"), None);
        value["live_claims_imported"] = json!(true);
        assert_eq!(historical_owner(&value, "a"), None);
        value["live_claims_imported"] = json!(false);
        value["original_source_row"]["owner"] = json!("  ");
        assert_eq!(historical_owner(&value, "a"), None);
    }
}
