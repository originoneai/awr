//! Bounded document discovery over the same authorized sources used by work context.
use super::*;
use std::collections::BTreeMap;

const CONTENT_READ_LIMIT: usize = 262_144;

fn description(path: &str, text: &str) -> (String, Option<String>) {
    let prefix: String = text.chars().take(4096).collect();
    let bytes = prefix.into_bytes();
    let snapshot = awr_source::SourceSnapshot {
        locator: format!("project://{path}"),
        fingerprint: awr_source::fingerprint(&bytes),
        bytes,
        content_review: None,
    };
    // Display metadata is optional. Parsing never changes source-read admission.
    let sections = awr_source::markdown_sections(&snapshot).unwrap_or_default();
    let primary = sections
        .iter()
        .find(|section| section.level > 0)
        .or_else(|| sections.first());
    let name = primary
        .filter(|section| section.level > 0)
        .map(|section| section.title.as_str())
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path))
        .chars()
        .take(256)
        .collect();
    let summary = primary
        .filter(|section| !section.body.trim().is_empty())
        .map(|section| {
            section
                .body
                .split("\n\n")
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .take(80)
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(512)
                .collect::<String>()
        })
        .filter(|summary| !summary.is_empty());
    (name, summary)
}

fn document(auth: &ReaderAuthority, path: &str, file: Option<&Value>) -> PgResult<Value> {
    let mut row = json!({
        "path":null,"name":null,"summary":null,"summary_kind":"source_excerpt",
        "sha256":null,"byte_length":null,"source_kind":"registered_source",
        "readable":false,"status":"unverified","reason":null
    });
    if safe_relative_source_path(path).is_err() {
        row["reason"] = json!("source_path_rejected");
        return Ok(row);
    }
    if let Err(error) = authorize_source_file_streams(auth, path) {
        if !matches!(error, PgError::Forbidden) {
            return Err(error);
        }
        row["status"] = json!("restricted");
        row["reason"] = json!("permission_restricted");
        return Ok(row);
    }
    row["path"] = json!(path);
    let Some(file) = file else {
        row["status"] = json!("missing");
        row["reason"] = json!("required_document_missing");
        return Ok(row);
    };
    let Some(text) = file["text"].as_str() else {
        row["reason"] = json!("source_text_not_recorded");
        return Ok(row);
    };
    let Some(expected) = file["sha256"].as_str().filter(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) else {
        row["reason"] = json!("source_digest_not_recorded");
        return Ok(row);
    };
    if crate::source::sha256_hex(text.as_bytes()) != expected {
        row["status"] = json!("changed");
        row["reason"] = json!("source_digest_mismatch");
        return Ok(row);
    }
    if file["bytes"].as_u64() != Some(text.len() as u64) {
        row["reason"] = json!("source_size_not_verified");
        return Ok(row);
    }
    let (name, summary) = description(path, text);
    row["name"] = json!(name);
    row["summary"] = json!(summary);
    row["sha256"] = json!(expected);
    row["byte_length"] = json!(text.len());
    if text.len() > CONTENT_READ_LIMIT {
        row["status"] = json!("too_large");
        row["reason"] = json!("content_exceeds_read_budget");
    } else {
        row["status"] = json!("available");
        row["readable"] = json!(true);
    }
    Ok(row)
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
        .filter(|stream| {
            bound.is_none_or(|id| stream.id == id)
                && auth
                    .access
                    .authorize(&auth.catalog, stream.id, WorkstreamAction::Read)
                    .is_ok()
        })
        .collect();
    if streams.is_empty() {
        return Err(PgError::Forbidden);
    }
    let mut paths = BTreeMap::<String, Option<String>>::new();
    for stream in &streams {
        let content_work = if let Some(work) = &q.work_id {
            Some(work.clone())
        } else if let Some(scope) = &auth.navigation_read_scope {
            if scope.streams.contains(&stream.id) {
                None
            } else {
                let Some((work, _)) = scope.tasks.iter().find(|(_, owner)| **owner == stream.id)
                else {
                    continue;
                };
                Some(work.clone())
            }
        } else {
            None
        };
        for path in &stream.acceptance_contracts {
            paths
                .entry(path.clone())
                .and_modify(|current| {
                    if content_work.is_none() {
                        *current = None;
                    }
                })
                .or_insert_with(|| content_work.clone());
        }
    }
    let files = active_source_files(tx, tenant, project).await?;
    let files_by_path: BTreeMap<_, _> = files
        .iter()
        .filter_map(|file| file["path"].as_str().map(|path| (path, file)))
        .collect();
    // Match the controlled content read's own selected delegation. A task's
    // stream does not confer the other tasks' authority, and independent read
    // grants cannot synthesize permission for a shared source.
    let now_ms: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);
    let mut content_authorities = BTreeMap::<Option<String>, Option<ReaderAuthority>>::new();
    let mut rows = Vec::new();
    for (path, content_work) in &paths {
        if !content_authorities.contains_key(content_work) {
            let mut selected = auth.clone();
            let mut content_query = q.clone();
            content_query.op = "source.content".into();
            content_query.work_id = content_work.clone();
            content_query.workstream_id = None;
            content_query.session_id = None;
            let authority = match crate::delegation_auth::resolve_agent_read_delegation(
                tx,
                &mut selected,
                project,
                &content_query,
                now_ms,
            )
            .await
            {
                Ok(()) => Some(selected),
                Err(PgError::Forbidden) => None,
                Err(error) => return Err(error),
            };
            content_authorities.insert(content_work.clone(), authority);
        }
        let mut row = match content_authorities
            .get(content_work)
            .and_then(Option::as_ref)
        {
            Some(selected) => document(selected, path, files_by_path.get(path.as_str()).copied())?,
            None => json!({"path":null,"name":null,"summary":null,"summary_kind":"source_excerpt",
                "sha256":null,"byte_length":null,"source_kind":"registered_source","readable":false,
                "status":"restricted","reason":"permission_restricted"}),
        };
        if row["path"].is_string() {
            if let Some(work) = content_work {
                row["content_work_id"] = json!(work);
            }
        }
        rows.push(row);
    }
    if let Some(search) = &q.search {
        let needle = search.to_lowercase();
        rows.retain(|row| {
            ["name", "summary", "path"].iter().any(|field| {
                row[*field]
                    .as_str()
                    .is_some_and(|value| value.to_lowercase().contains(&needle))
            })
        });
    }
    // Redacted names never participate in visible sorting or cursor identities.
    rows.sort_by(|a, b| {
        a["path"]
            .as_str()
            .unwrap_or("\u{10ffff}")
            .cmp(b["path"].as_str().unwrap_or("\u{10ffff}"))
    });
    for (index, row) in rows.iter_mut().enumerate() {
        row["document_position"] = json!(index + 1);
    }
    let binding = hash(&json!({
        "auth":auth.binding,"snapshot":auth.snapshot,"epoch":auth.epoch,
        "op":q.op,"work":q.work_id,"stream":bound,"search":q.search,"documents":rows
    }))?;
    let current = cursor(q, &binding)?;
    let start = current
        .index
        .checked_add(1)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or(PgError::CursorExpired)?;
    if start > rows.len() {
        return Err(PgError::CursorExpired);
    }
    let end = start
        .saturating_add(usize::from(q.limit.unwrap_or(20)))
        .min(rows.len());
    let more = end < rows.len();
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
    let observed_at: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);
    let mut result = json!({
        "protocol_version":1,"scope_id":"main","selection_basis":"project",
        "source_snapshot_id":auth.snapshot,"coordinator_epoch":auth.epoch,
        "project_revision":auth.revision.to_string(),"project_status":auth.project_status,
        "data":{"work_id":q.work_id,"items":&rows[start..end],"total":rows.len(),
            "next_cursor":next,"truncated":more,"read_only":true,"execution_authorized":false,
            "scope":if q.work_id.is_some(){"work"}else if auth.navigation_read_scope.as_ref().is_some_and(|scope| !scope.tasks.is_empty()) {"authorized_works"} else {"authorized_workstreams"},
            "content_read_limit_bytes":CONTENT_READ_LIMIT,
            "as_of":{"at_unix_ms":observed_at,"source_snapshot_id":auth.snapshot,
                "project_revision":auth.revision.to_string()}}
    });
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
    fn descriptions_reuse_markdown_headings_and_bound_multibyte_excerpts() {
        let (name, summary) = description(
            "docs/guide.md",
            "---\nstatus: draft\n---\n# Delivery guide\n\nCheck the outcome before sharing it.\n",
        );
        assert_eq!(name, "Delivery guide");
        assert_eq!(
            summary.as_deref(),
            Some("Check the outcome before sharing it.")
        );
        let text = format!("# {}\n\n{}", "例".repeat(300), "文".repeat(1000));
        let (name, summary) = description("docs/guide.md", &text);
        assert_eq!(name.chars().count(), 256);
        assert_eq!(summary.unwrap().chars().count(), 512);
    }
}
