//! Integrity observations over the registered required source set, never a filesystem scan.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn recovery(reason: &str, position: usize) -> Value {
    let action = match reason {
        "required_spec_missing" => {
            Some("Restore the current required document through source publishing.")
        }
        "required_spec_forbidden" => Some(
            "Ask an authorized administrator to review source access or provide an authorized replacement.",
        ),
        "spec_digest_mismatch" => Some(
            "Verify the document against its registered source, then publish the intended version.",
        ),
        "spec_path_rejected" => {
            Some("Correct the required document reference through reviewed planning.")
        }
        _ => None,
    };
    json!({"reason":reason,"recognized":action.is_some(),"source_position":position,"next_action":action})
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let bound = if let Some(work) = q.work_id.as_deref() {
        let (binding, _) = work_binding(tx, tenant, project, auth, work).await?;
        if q.workstream_id
            .is_some_and(|id| id != binding.workstream_id)
        {
            return Err(PgError::Forbidden);
        }
        Some(binding.workstream_id)
    } else {
        q.workstream_id
    };
    let streams: Vec<_> = auth
        .catalog
        .workstreams
        .iter()
        .filter(|s| {
            auth.access
                .authorize(&auth.catalog, s.id, WorkstreamAction::Read)
                .is_ok()
                && bound.is_none_or(|id| id == s.id)
        })
        .collect();
    if streams.is_empty() {
        return Err(PgError::Forbidden);
    }
    let required: BTreeSet<_> = streams
        .iter()
        .flat_map(|s| s.acceptance_contracts.iter().cloned())
        .collect();
    let files = active_source_files(tx, tenant, project).await?;
    let by_path: BTreeMap<_, _> = files
        .iter()
        .filter_map(|f| f["path"].as_str().map(|path| (path, f)))
        .collect();
    let checked: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);
    let mut records = Vec::new();
    for path in required {
        let mut row = json!({"path":path,"expected_sha256":null,"current_sha256":null,
            "status":"unverified","reason":null,"checked_at_unix_ms":checked,
            "missing":{"expected_sha256":null,"current_sha256":null,"path":null}});
        if safe_relative_source_path(&path).is_err() {
            row["path"] = Value::Null;
            row["reason"] = json!("spec_path_rejected");
            row["missing"] = json!({"path":"spec_path_rejected","expected_sha256":"spec_path_rejected","current_sha256":"spec_path_rejected"});
        } else if let Err(error) = authorize_source_file_streams(auth, &path) {
            if !matches!(error, PgError::Forbidden) {
                return Err(error);
            }
            row["path"] = Value::Null;
            row["status"] = json!("forbidden");
            row["reason"] = json!("required_spec_forbidden");
            row["missing"] = json!({"path":"permission_restricted","expected_sha256":"permission_restricted","current_sha256":"permission_restricted"});
        } else if let Some(file) = by_path.get(path.as_str()) {
            let text = file["text"].as_str();
            let expected = file["sha256"].as_str().filter(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
            row["expected_sha256"] = json!(expected);
            if text.is_none() {
                row["reason"] = json!("source_text_not_recorded");
                row["missing"]["current_sha256"] = json!("not_recorded");
                if expected.is_none() {
                    row["missing"]["expected_sha256"] = json!("not_recorded");
                }
                records.push(row);
                continue;
            }
            let text = text.unwrap();
            let current = crate::source::sha256_hex(text.as_bytes());
            row["current_sha256"] = json!(current);
            match expected {
                Some(expected) if expected == current => row["status"] = json!("consistent"),
                Some(_) => {
                    row["status"] = json!("changed");
                    row["reason"] = json!("spec_digest_mismatch");
                }
                None => {
                    row["reason"] = json!("expected_digest_not_recorded");
                    row["missing"]["expected_sha256"] = json!("not_recorded");
                }
            }
        } else {
            row["status"] = json!("missing");
            row["reason"] = json!("required_spec_missing");
            row["missing"]["expected_sha256"] = json!("not_recorded");
            row["missing"]["current_sha256"] = json!("required_spec_missing");
        }
        records.push(row);
    }
    // Hidden source names cannot affect visible ordering or escape through a cursor.
    records.sort_by(|a, b| {
        a["path"]
            .as_str()
            .unwrap_or("\u{10ffff}")
            .cmp(b["path"].as_str().unwrap_or("\u{10ffff}"))
            .then_with(|| a["status"].as_str().cmp(&b["status"].as_str()))
    });
    let complete = records.iter().all(|r| r["status"] == "consistent");
    let mut recoveries = Vec::new();
    for (index, row) in records.iter_mut().enumerate() {
        row["source_position"] = json!(index + 1);
        if let Some(reason) = row["reason"].as_str() {
            recoveries.push(recovery(reason, index + 1));
        }
    }
    let basis: Vec<_> = records
        .iter()
        .map(|r| {
            json!({"path":r["path"],"expected":r["expected_sha256"],
        "current":r["current_sha256"],"status":r["status"],"reason":r["reason"]})
        })
        .collect();
    let binding = hash(
        &json!({"auth":auth.binding,"snapshot":auth.snapshot,"epoch":auth.epoch,
        "work":q.work_id,"stream":bound,"op":q.op,"sources":basis}),
    )?;
    let cursor = cursor(q, &binding)?;
    let start = cursor
        .index
        .checked_add(1)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(PgError::CursorExpired)?;
    if start > records.len() {
        return Err(PgError::CursorExpired);
    }
    let end = start
        .saturating_add(usize::from(q.limit.unwrap_or(20)))
        .min(records.len());
    let reasons: BTreeSet<_> = recoveries
        .iter()
        .filter_map(|r| r["reason"].as_str())
        .map(str::to_owned)
        .collect();
    let recoveries: Vec<_> = recoveries
        .into_iter()
        .filter(|r| {
            r["source_position"]
                .as_u64()
                .is_some_and(|p| p > start as u64 && p <= end as u64)
        })
        .collect();
    let more = end < records.len();
    let next = if more {
        next_cursor(
            &binding,
            "",
            0,
            i32::try_from(end - 1).map_err(|_| PgError::ResponseTooLarge)?,
        )
    } else {
        Value::Null
    };
    let mut result = json!({"protocol_version":1,"scope_id":"main","selection_basis":"project",
        "source_snapshot_id":auth.snapshot,"coordinator_epoch":auth.epoch,"project_revision":auth.revision.to_string(),
        "project_status":auth.project_status,
        "data":{"work_id":q.work_id,"items":&records[start..end],"total":records.len(),
            "next_cursor":next,"truncated":more,"execution_authorized":false,
            "collection":{"complete":complete,"completeness":if complete{"complete"}else{"incomplete"},
                "scope":if q.work_id.is_some(){"work"}else{"authorized_workstreams"},
                "verification_basis":"active_source_snapshot","required_total":records.len(),
                "checked_at_unix_ms":checked,"reasons":reasons,"recovery_scope":"page","recovery":recoveries},
            "as_of":{"at_unix_ms":checked,"source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string()}}});
    if let Some(id) = bound {
        let stream = auth.catalog.get(id)?;
        result["workstream_id"] = json!(id.to_string());
        result["authority_version"] = json!(stream.authority_version.to_string());
        result["selection_basis"] = json!(if q.work_id.is_some() {
            "work"
        } else {
            "explicit"
        });
    }
    if serde_json::to_vec(&result)
        .map_err(|_| PgError::SourceDivergence)?
        .len()
        > q.max_context_bytes.unwrap_or(262144)
    {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_recovery_reasons_remain_explicit_without_inventing_an_action() {
        let value = recovery("future_source_reason", 2);
        assert_eq!(value["reason"], "future_source_reason");
        assert_eq!(value["recognized"], false);
        assert!(value["next_action"].is_null());
    }
}
