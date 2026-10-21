//! Read-only, version-bound pages of a fully validated persisted artifact.
//! Cursors are navigation, never authority or proof of complete consumption.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentCursor {
    version: u8,
    binding: String,
    offset: usize,
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
    work: &str,
    authority_version: u64,
) -> PgResult<Value> {
    let (work_scope, ownership) = work_binding(tx, tenant, project, auth, work).await?;
    // Re-run the same full-content authority/integrity checks for every page.
    // Export revocation, current approval and adoption are checked by its loader.
    let (bytes, metadata) = if q.export_id.is_some() {
        crate::cross_workstream_exports::content_bytes(tx, tenant, project, auth, work, q).await?
    } else {
        controlled_artifact_bytes(
            tx,
            tenant,
            project,
            work,
            q.artifact_id.as_deref().ok_or(PgError::Forbidden)?,
            q.expected_sha256.as_deref(),
        )
        .await?
    };
    let contract: String = tx.query_one(
        "SELECT contract_hash FROM awr_team.work_contracts
         WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
        &[&tenant, &project, &auth.snapshot, &work],
    ).await?.get(0);
    let cursor_basis = hash(&json!({
        "op":"artifact.segment", "tenant":tenant, "project":project,
        "actor":auth.actor_id, "client":auth.client_id, "auth":auth.binding,
        "membership":auth.membership_version, "epoch":auth.epoch,
        "source":auth.snapshot, "stream":work_scope.workstream_id,
        "authority":authority_version, "grant":auth.grant_versions[&work_scope.workstream_id],
        "work":work, "ownership":ownership, "contract":contract,
        "work_selector":q.work_id, "session_selector":q.session_id,
        "stream_selector":q.workstream_id, "artifact":q.artifact_id, "export":q.export_id,
        "sha256":metadata["sha256"], "export_version":metadata["export_version"],
        "disclosure":metadata["disclosure_sha256"], "adopted":metadata["adopted"]
    }))?;
    let offset = match &q.cursor {
        Some(raw) => {
            let cursor: SegmentCursor =
                serde_json::from_str(raw).map_err(|_| PgError::CursorExpired)?;
            let expected_binding = hash(&json!({
                "artifact_binding":cursor_basis, "offset":cursor.offset
            }))?;
            if cursor.version != 1
                || cursor.binding != expected_binding
                || (cursor.offset >= bytes.len() && !(cursor.offset == 0 && bytes.is_empty()))
            {
                return Err(PgError::CursorExpired);
            }
            cursor.offset
        }
        None => 0,
    };
    let end = offset
        .saturating_add(q.max_context_bytes.unwrap_or(2048))
        .min(bytes.len());
    let segment = &bytes[offset..end];
    let next_cursor = if end < bytes.len() {
        Some(
            serde_json::to_string(&SegmentCursor {
                version: 1,
                binding: hash(&json!({
                    "artifact_binding":cursor_basis, "offset":end
                }))?,
                offset: end,
            })
            .map_err(|_| PgError::SourceDivergence)?,
        )
    } else {
        None
    };
    let next_query = next_cursor.as_ref().map(|cursor| {
        let mut query = json!({"protocol_version":1, "op":"artifact.segment",
            "expected_sha256":q.expected_sha256, "cursor":cursor});
        for (field, value) in [
            ("work_id", json!(q.work_id)),
            ("session_id", json!(q.session_id)),
            ("workstream_id", json!(q.workstream_id)),
            ("artifact_id", json!(q.artifact_id)),
            ("export_id", json!(q.export_id)),
            ("max_context_bytes", json!(q.max_context_bytes)),
        ] {
            if !value.is_null() {
                query[field] = value;
            }
        }
        query
    });
    Ok(json!({
        "kind":"artifact_segment", "artifact_id":q.artifact_id, "export_id":q.export_id,
        "work_id":work, "sha256":metadata["sha256"], "byte_length":bytes.len(),
        "range":{"start":offset,"end_exclusive":end}, "encoding":"base64",
        "content_base64":base64_encode(segment), "segment_sha256":crate::source::sha256_hex(segment),
        "end_of_artifact":end==bytes.len(), "next_cursor":next_cursor, "next_query":next_query,
        "export_version":metadata["export_version"], "disclosure_sha256":metadata["disclosure_sha256"],
        "adopted":metadata["adopted"], "execution_authorized":false,
        "path_bypass":false,"url_bypass":false,"upstream_source_access":false,
        "consumption":"Concatenate contiguous ranges from byte 0, then verify byte_length and sha256. The final page alone does not prove complete consumption or review."
    }))
}
