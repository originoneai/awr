use super::*;
use crate::workstream_auth::authorize_domain_action;

/// Request metadata retention is capacity bounded, independent of business history.
const REQUEST_CAPACITY: i64 = 10_000;

impl WorkstreamReadStore {
    /// Start before dispatch. An interrupted dispatch keeps `unknown`, never a
    /// fabricated failure or success. Invalid credentials are not attributed.
    pub async fn request_audit_begin(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        action: &str,
        work: Option<&str>,
    ) -> PgResult<String> {
        if action.is_empty()
            || action.len() > 100
            || !action
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._".contains(&b))
        {
            return Err(PgError::Protocol("invalid audit action".into()));
        }
        let mut c = self.pool.get().await?;
        crate::check_schema(&c).await?;
        let tx = c.transaction().await?;
        let auth = authenticate(&tx, tenant, project, bearer).await?;
        let visible = super::navigation::visible_streams(&auth);
        let work = if let Some(work) = work {
            tx.query_opt("SELECT work_id FROM awr_team.workstream_snapshot_ownership WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND work_id=$4 AND workstream_id=ANY($5)", &[&tenant,&project,&auth.snapshot,&work,&visible]).await?.map(|r|r.get::<_,String>(0))
        } else {
            None
        };
        let id = crate::tx::new_id();
        let credential = bearer.split('.').nth(1).ok_or(PgError::Forbidden)?;
        tx.execute("INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,credential_id,action,work_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)", &[&tenant,&project,&id,&auth.actor_id,&auth.client_id,&credential,&action,&work]).await?;
        tx.execute("DELETE FROM awr_team.request_audit WHERE tenant_id=$1 AND project_id=$2 AND id IN (SELECT id FROM awr_team.request_audit WHERE tenant_id=$1 AND project_id=$2 ORDER BY created_at DESC,id DESC OFFSET $3)", &[&tenant,&project,&REQUEST_CAPACITY]).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn request_audit_finish(
        &self,
        tenant: &str,
        project: &str,
        id: &str,
        result: &str,
    ) -> PgResult<()> {
        if !matches!(result, "succeeded" | "denied" | "failed" | "unknown") {
            return Err(PgError::Protocol("invalid audit result".into()));
        }
        let mut c = self.pool.get().await?;
        let tx = c.transaction().await?;
        crate::tx::bind_workstream_scope(&tx, tenant, project).await?;
        tx.execute("UPDATE awr_team.request_audit SET result=$4,finished_at=clock_timestamp() WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND result='unknown'", &[&tenant,&project,&id,&result]).await?;
        tx.commit().await?;
        Ok(())
    }
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    directory_auth: Option<&ReaderAuthority>,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let project_wide =
        authorize_domain_action(auth, awr_team::Action::AuditReadProject, None, None).is_ok();
    if !project_wide
        && q.member_actor_id
            .as_ref()
            .is_some_and(|m| m != &auth.actor_id)
    {
        return Err(PgError::Forbidden);
    }
    let actor = if project_wide {
        q.member_actor_id.clone()
    } else {
        Some(auth.actor_id.clone())
    };
    let streams = super::navigation::visible_streams(auth);
    let binding = hash(
        &json!({"reader":auth.binding,"grants":auth.grant_versions,"streams":streams,"op":q.op,
        "actor":actor,"work":q.work_id,"project_wide":project_wide}),
    )?;
    let c = cursor(q, &binding)?;
    let before = c
        .revision
        .parse::<i64>()
        .map_err(|_| PgError::CursorExpired)?;
    let limit = i64::from(q.limit.unwrap_or(50));
    // Use exact microsecond timestamps + id; no offset pagination and no payload
    // projection. Access-only metadata has no arbitrary body, chat or tool I/O.
    let sql = if q.op == "audit.requests" {
        r#"
        SELECT a.id,a.actor_id,a.client_id,a.action,a.work_id,a.result,
          (extract(epoch FROM a.created_at)*1000000)::bigint AS time_us,a.credential_id
        FROM awr_team.request_audit a
        WHERE a.tenant_id=$1 AND a.project_id=$2 AND ($3::text IS NULL OR a.actor_id=$3)
          AND ($4::text IS NULL OR a.work_id=$4)
          AND (a.work_id IS NULL OR EXISTS(SELECT 1 FROM awr_team.workstream_snapshot_ownership o WHERE o.tenant_id=$1 AND o.project_id=$2 AND o.snapshot_id=$5 AND o.work_id=a.work_id AND o.workstream_id=ANY($6)))
          AND ($7::bigint=0 OR ((extract(epoch FROM a.created_at)*1000000)::bigint,a.id)<($7,$8))
        ORDER BY a.created_at DESC,a.id DESC LIMIT $9
    "#
    } else {
        r#"
        SELECT e.id,e.actor_id,o.client_id,e.event_type AS action,e.work_id,
          'committed'::text AS result,(extract(epoch FROM e.created_at)*1000000)::bigint AS time_us,NULL::text AS credential_id
        FROM awr_team.events e
        LEFT JOIN awr_team.operations o ON o.tenant_id=e.tenant_id AND o.project_id=e.project_id AND o.actor_id=e.actor_id AND o.committed_project_revision=e.project_revision
        WHERE e.tenant_id=$1 AND e.project_id=$2 AND ($3::text IS NULL OR e.actor_id=$3)
          AND ($4::text IS NULL OR e.work_id=$4)
          AND (e.work_id IS NULL OR EXISTS(SELECT 1 FROM awr_team.workstream_snapshot_ownership s WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.snapshot_id=$5 AND s.work_id=e.work_id AND s.workstream_id=ANY($6)))
          AND ($7::bigint=0 OR ((extract(epoch FROM e.created_at)*1000000)::bigint,e.id)<($7,$8))
        ORDER BY e.created_at DESC,e.id DESC LIMIT $9
    "#
    };
    let rows = tx
        .query(
            sql,
            &[
                &tenant,
                &project,
                &actor,
                &q.work_id,
                &auth.snapshot,
                &streams,
                &before,
                &c.key,
                &(limit + 1),
            ],
        )
        .await?;
    let page = rows.iter().take(limit as usize).collect::<Vec<_>>();
    let actor_ids = page
        .iter()
        .map(|row| row.get::<_, String>("actor_id"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    // The directory.read gate for identity disclosure must see the reader's
    // unrestricted workstream access. The query authority `auth` may already be
    // narrowed to one delegated workstream for row visibility; re-deriving the
    // gate from the pre-narrowing authority keeps disclosure exactly as broad
    // (and no broader) than directory.read itself.
    let disclose_names = if page.is_empty() {
        false
    } else {
        match directory_auth {
            Some(authority) => {
                let mut gate = authority.clone();
                match crate::directory::authorize_read(tx, &mut gate, project).await {
                    Ok(()) => true,
                    Err(PgError::Forbidden) => false,
                    Err(error) => return Err(error),
                }
            }
            None => false,
        }
    };
    let people = if disclose_names {
        crate::review::resolve_person_ids(tx, tenant, project, &actor_ids).await?
    } else {
        std::collections::BTreeMap::new()
    };
    let items: Vec<_> = page
        .into_iter()
        .map(|r| {
            let actor_id = r.get::<_, String>("actor_id");
            let (person_id, display_name) = people
                .get(&actor_id)
                .and_then(|person| person.active_person())
                .map_or((None, None), |(id, name)| (Some(id), Some(name)));
            json!({"id":r.get::<_,String>("id"),
        "actor_id":actor_id,"client_id":r.get::<_,Option<String>>("client_id"),
        "actor":{"person_id":person_id,"display_name":display_name},
        // ConnectionStore has no registered per-client display-name source.
        "client":{"display_name":Value::Null},
        "action":r.get::<_,String>("action"),"work_id":r.get::<_,Option<String>>("work_id"),
        "result":r.get::<_,String>("result"),"created_at_unix_ms":r.get::<_,i64>("time_us")/1000,
        "credential_id":r.get::<_,Option<String>>("credential_id")})
        })
        .collect();
    let next = if rows.len() > limit as usize {
        let r = &rows[limit as usize - 1];
        next_cursor(&binding, &r.get::<_, String>("id"), r.get("time_us"), -1)
    } else {
        Value::Null
    };
    Ok(
        json!({"data":{"scope":if project_wide {"project"} else {"self"},"items":items,"next_cursor":next,
        "request_retention_capacity":REQUEST_CAPACITY,"request_retention":"oldest_pruned_soft_capacity",
        "unknown_means":"completion_not_recorded; inspect business request before retrying",
        "development_source":"committed_events_with_operation_attribution; historical client may be unavailable",
        "chat_text_collected":false,"tool_io_collected":false}}),
    )
}
