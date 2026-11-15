//! Version observations share the live dependency gates used by execution admission.
use super::*;
use awr_team::DependencyAcceptanceMode;

const RECORDS: &str = "WITH edges AS (
    SELECT c.work_id,c.title,c.contract_json,c.contract_hash,o.workstream_id,
      dependency.provider,dependency.ordinality,
      row_number() OVER (ORDER BY c.work_id,dependency.ordinality) AS position
    FROM awr_team.work_contracts c
    JOIN awr_team.workstream_snapshot_ownership o
      USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
    CROSS JOIN LATERAL jsonb_array_elements_text(c.contract_json->'required_dependencies')
      WITH ORDINALITY dependency(provider,ordinality)
    WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main'
      AND (o.workstream_id=ANY($4) OR EXISTS (
        SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
        WHERE allowed.work_id=c.work_id AND allowed.workstream_id=o.workstream_id))
      AND ($7::text IS NULL OR c.work_id=$7) AND ($8::text IS NULL OR o.workstream_id=$8)
)";

fn unavailable(item: &mut Value, reason: &str) {
    item["validity"] = json!("unavailable");
    item["reason"] = json!(reason);
}

fn receipt_version(row: &tokio_postgres::Row) -> Value {
    json!({"receipt_id":row.get::<_,String>(0),"contract_hash":row.get::<_,String>(4),
        "source_snapshot_id":row.get::<_,String>(6)})
}

async fn consumed_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    contract_hash: &str,
    provider: &str,
) -> PgResult<Option<String>> {
    // The committed admission records the actual inputs. An available upstream
    // completion, or a later adoption, cannot substitute for those inputs.
    let row = tx.query_opt("SELECT o.result_json,e.id,e.input_digest,e.ownership_version,e.workstream_id,e.session_id
        FROM awr_team.executions e JOIN awr_team.operations o
          ON o.tenant_id=e.tenant_id AND o.project_id=e.project_id
          AND o.actor_id=e.executor_actor_id AND o.client_id=e.executor_client_id
          AND o.op='execution.start' AND o.state='committed'
          AND o.result_json->'data'->>'execution_id'=e.id
        WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.work_id=$3 AND e.contract_hash=$4 AND e.scope_id='main'
          AND o.committed_project_revision IS NOT NULL
        ORDER BY o.committed_project_revision DESC,e.id DESC LIMIT 1",
        &[&tenant,&project,&work,&contract_hash]).await?;
    let Some(row) = row else { return Ok(None) };
    let receipt: Value = row.get(0);
    if receipt["scope_id"] != "main"
        || receipt["work_id"] != work
        || receipt["contract_hash"] != contract_hash
        || receipt["ownership_version"] != row.get::<_, i64>(3).to_string()
        || receipt["workstream_id"] != json!(row.get::<_, Option<String>>(4))
        || receipt["data"]["input_digest"] != json!(row.get::<_, Option<String>>(2))
        || receipt["data"]["session_id"] != json!(row.get::<_, Option<String>>(5))
    {
        return Err(PgError::SourceDivergence);
    }
    let links: Vec<(String, String)> =
        serde_json::from_value(receipt["data"]["dependency_receipts"].clone())
            .map_err(|_| PgError::SourceDivergence)?;
    if links.iter().filter(|(work, _)| work == provider).count() > 1 {
        return Err(PgError::SourceDivergence);
    }
    Ok(links
        .into_iter()
        .find(|(work, _)| work == provider)
        .map(|(_, receipt)| receipt))
}

async fn recorded_version(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    provider: &str,
    receipt: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT contract_hash FROM awr_team.completion_receipts
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND id=$4 AND scope_id='main'",
            &[&tenant, &project, &provider, &receipt],
        )
        .await?;
    Ok(row
        .map(|row| {
            json!({"receipt_id":receipt,"contract_hash":row.get::<_,String>(0),
        "source_snapshot_id":null})
        })
        .unwrap_or(Value::Null))
}

async fn export_contract_changed(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    consumer: &str,
    provider: &str,
    adoption: &tokio_postgres::Row,
    current_hash: &str,
) -> PgResult<bool> {
    let row=tx.query_opt("SELECT manifest_json,disclosure_sha256,version FROM awr_team.workstream_artifact_exports
        WHERE tenant_id=$1 AND project_id=$2 AND consumer_work_id=$3 AND provider_work_id=$4 AND id=$5 AND status='active'",
        &[&tenant,&project,&consumer,&provider,&adoption.get::<_,String>(2)]).await?;
    let Some(row) = row else { return Ok(false) };
    let manifest: Value = row.get(0);
    // This explains invalidation only. It grants no historical metadata or
    // artifact access when the original live export proof is unavailable.
    Ok(row.get::<_, String>(1) == adoption.get::<_, String>(4)
        && row.get::<_, i64>(2) == adoption.get::<_, i64>(3)
        && hash(&manifest)? == row.get::<_, String>(1)
        && manifest["consumer_work_id"] == consumer
        && manifest["provider_work_id"] == provider
        && manifest["provider_contract_hash"]
            .as_str()
            .is_some_and(|old| old != current_hash))
}

pub(super) async fn edge(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
    contract: &WorkContract,
    provider: &str,
) -> PgResult<Value> {
    let mode = contract.dependency_acceptance.get(provider).copied();
    let cross = match mode {
        Some(DependencyAcceptanceMode::CrossWorkstream(policy)) => Some(policy),
        _ => None,
    };
    let mut item = json!({"work_id":work,"provider_work_id":provider,"provider_title":null,
        "version_policy":cross.map(|p|json!(p.version_policy)).unwrap_or(json!("current_contract")),
        "review_assurance":mode,"required_version":null,"available_version":null,
        "adopted_version":null,"consumed_version":null,"adoption_valid":null,
        "validity":"unavailable","reason":null,
        "missing":{"provider_title":null,"required_version":null,"available_version":null,
            "adopted_version":null,"consumed_version":null}});
    let upstream = tx.query_opt("SELECT c.contract_hash,c.title,o.workstream_id
        FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
          USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main' AND c.work_id=$4",
        &[&tenant,&project,&auth.snapshot,&provider]).await?;
    let readable = if let Some(row) = &upstream {
        let id: Id = row
            .get::<_, String>(2)
            .parse()
            .map_err(|_| PgError::SourceDivergence)?;
        auth.access
            .authorize(&auth.catalog, id, WorkstreamAction::Read)
            .is_ok()
            && crate::delegation_auth::authorize_navigation_action(
                auth,
                awr_team::Action::WorkRead,
                id,
                provider,
            )
            .is_ok()
    } else {
        false
    };
    if readable {
        let row = upstream.as_ref().unwrap();
        item["provider_title"] = json!(row.get::<_, String>(1));
        if cross.is_none_or(|policy| {
            policy.version_policy == awr_core::DeliveryVersionPolicy::CurrentContract
        }) {
            item["required_version"] = json!({"contract_hash":row.get::<_,String>(0),
                "receipt_id":null,"source_snapshot_id":auth.snapshot});
        }
    } else {
        item["missing"]["provider_title"] = json!("permission_restricted");
    }
    let consumed = consumed_receipt(
        tx,
        tenant,
        project,
        work,
        &contract.hash().map_err(|_| PgError::SourceDivergence)?,
        provider,
    )
    .await?;
    let consumed_recorded = consumed.is_some();
    let mut adoption_recorded = consumed_recorded;
    if let Some(policy) = cross {
        let adoption =
            crate::cross_workstream_adoption::selection(tx, tenant, project, work, provider)
                .await?;
        adoption_recorded = adoption.is_some();
        if let Some(adoption) = adoption {
            let checked = crate::cross_workstream_exports::validated_export(
                tx,
                tenant,
                project,
                &auth.snapshot,
                work,
                &adoption.get::<_, String>(2),
            )
            .await;
            match checked {
                Ok((manifest, version, disclosure)) => {
                    let accepted = crate::cross_workstream_adoption::adopted_receipt(
                        tx,
                        tenant,
                        project,
                        &auth.snapshot,
                        work,
                        provider,
                        policy,
                    )
                    .await?;
                    let valid = accepted.is_some()
                        && version == adoption.get::<_, i64>(3)
                        && disclosure == adoption.get::<_, String>(4);
                    item["adoption_valid"] = json!(valid);
                    if valid {
                        let version = json!({"receipt_id":manifest["receipt_id"],
                            "contract_hash":manifest["provider_contract_hash"],
                            "source_snapshot_id":manifest.get("provider_source_snapshot_id")});
                        item["required_version"] = version.clone();
                        item["adopted_version"] = version.clone();
                        item["available_version"] = version;
                        item["validity"] = json!("valid");
                        if consumed
                            .as_deref()
                            .is_some_and(|receipt| Some(receipt) != accepted.as_deref())
                        {
                            item["validity"] = json!("upstream_changed");
                            item["reason"] = json!("execution_inputs_changed");
                        }
                    } else {
                        unavailable(&mut item, "adoption_invalid");
                    }
                }
                Err(
                    PgError::Forbidden
                    | PgError::EvidenceInvalid
                    | PgError::ReviewRequired
                    | PgError::PreconditionsChanged
                    | PgError::MissingDependency,
                ) => {
                    item["adoption_valid"] = json!(false);
                    unavailable(&mut item, "adoption_unavailable");
                    if readable
                        && policy.version_policy == awr_core::DeliveryVersionPolicy::CurrentContract
                        && export_contract_changed(
                            tx,
                            tenant,
                            project,
                            work,
                            provider,
                            &adoption,
                            &upstream.as_ref().unwrap().get::<_, String>(0),
                        )
                        .await?
                    {
                        item["validity"] = json!("upstream_changed");
                        item["reason"] = json!("upstream_contract_changed");
                    }
                }
                Err(error) => return Err(error),
            }
        } else {
            unavailable(&mut item, "adoption_not_recorded");
        }
    } else if !readable {
        unavailable(&mut item, "permission_restricted");
    } else if upstream
        .as_ref()
        .is_some_and(|row| row.get::<_, String>(2) != stream)
    {
        unavailable(&mut item, "dependency_policy_mismatch");
    } else {
        let available =
            crate::review::current_dependency_receipt(tx, tenant, project, provider, "main", mode)
                .await?;
        if let Some(available) = available {
            let version = receipt_version(&available);
            let changed = consumed
                .as_deref()
                .is_some_and(|receipt| version["receipt_id"] != receipt);
            item["available_version"] = version;
            item["validity"] = json!(if changed { "upstream_changed" } else { "valid" });
            if changed {
                item["reason"] = json!("execution_inputs_changed");
            }
        } else {
            unavailable(&mut item, "completion_unavailable");
        }
    }
    if let Some(receipt) = consumed {
        // A validated disclosure may reveal its exact original version. It
        // does not authorize lookup of other upstream receipt metadata.
        if readable {
            item["consumed_version"] =
                recorded_version(tx, tenant, project, provider, &receipt).await?;
        } else if item["adopted_version"]["receipt_id"] == receipt {
            item["consumed_version"] = item["adopted_version"].clone();
        }
        if cross.is_none() {
            item["adopted_version"] = item["consumed_version"].clone();
            if !item["consumed_version"].is_null()
                && !item["required_version"].is_null()
                && item["consumed_version"]["contract_hash"]
                    != item["required_version"]["contract_hash"]
            {
                item["validity"] = json!("upstream_changed");
                item["reason"] = json!("upstream_contract_changed");
            }
        }
    }
    let invalid: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.dependency_bindings
        WHERE tenant_id=$1 AND project_id=$2 AND downstream_work_id=$3 AND upstream_work_id=$4 AND NOT valid)",
        &[&tenant,&project,&work,&provider]).await?.get(0);
    if invalid && item["validity"] == "valid" {
        item["validity"] = json!("upstream_changed");
        item["reason"] = json!("dependency_binding_invalidated");
    }
    for field in [
        "required_version",
        "available_version",
        "adopted_version",
        "consumed_version",
    ] {
        if item[field].is_null() {
            item["missing"][field] = json!(if !readable {
                "permission_restricted"
            } else {
                "not_recorded"
            });
        }
    }
    if !consumed_recorded {
        item["missing"]["consumed_version"] = json!("not_recorded");
    }
    if !adoption_recorded {
        item["missing"]["adopted_version"] = json!("not_recorded");
    } else if cross.is_some() && item["adopted_version"].is_null() {
        item["missing"]["adopted_version"] = json!("adoption_unavailable");
    }
    Ok(item)
}

pub(super) async fn for_work(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
    contract: &WorkContract,
) -> PgResult<Vec<Value>> {
    let mut result = Vec::new();
    for provider in &contract.required_dependencies {
        result.push(edge(tx, tenant, project, auth, work, stream, contract, provider).await?);
    }
    Ok(result)
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
        crate::delegation_auth::authorize_navigation_action(
            auth,
            awr_team::Action::WorkRead,
            binding.workstream_id,
            work,
        )?;
        if q.workstream_id
            .is_some_and(|id| id != binding.workstream_id)
        {
            return Err(PgError::Forbidden);
        }
        Some(binding.workstream_id)
    } else {
        q.workstream_id
    };
    if let Some(id) = bound {
        auth.access
            .authorize(&auth.catalog, id, WorkstreamAction::Read)
            .map_err(|_| PgError::Forbidden)?;
    }
    let (streams, tasks, task_streams) = navigation::visible_work_scope(auth);
    let stream = bound.map(|id| id.to_string());
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[
        &tenant,
        &project,
        &auth.snapshot,
        &streams,
        &tasks,
        &task_streams,
        &q.work_id,
        &stream,
    ];
    let stamp = tx.query_one(&format!("{RECORDS} SELECT count(*),(extract(epoch FROM transaction_timestamp())*1000)::bigint FROM edges"),params).await?;
    let total: i64 = stamp.get(0);
    let binding = hash(
        &json!({"domain":"awr-dependency-list-v1","reader":auth.binding,
        "snapshot":auth.snapshot,"epoch":auth.epoch,"catalog":auth.catalog,"grants":auth.grant_versions,
        "work":q.work_id,"stream":stream,"revision":auth.revision.to_string()}),
    )?;
    let c = cursor(q, &binding)?;
    let offset = i64::from(c.index) + 1;
    if !c.key.is_empty() || c.index < -1 || offset > total {
        return Err(PgError::CursorExpired);
    }
    let limit = i64::from(q.limit.unwrap_or(20));
    let page_limit = limit + 1;
    let mut page_params = params.to_vec();
    page_params.extend([
        &offset as &(dyn tokio_postgres::types::ToSql + Sync),
        &page_limit,
    ]);
    let rows = tx
        .query(
            &format!(
                "{RECORDS} SELECT * FROM edges ORDER BY work_id,ordinality OFFSET $9 LIMIT $10"
            ),
            &page_params,
        )
        .await?;
    let more = rows.len() > limit as usize;
    let mut items = Vec::new();
    for row in rows.iter().take(limit as usize) {
        let work: String = row.get("work_id");
        let contract: WorkContract = serde_json::from_value(row.get("contract_json"))
            .map_err(|_| PgError::SourceDivergence)?;
        if contract.work_id.as_str() != work
            || contract.hash().map_err(|_| PgError::SourceDivergence)?
                != row.get::<_, String>("contract_hash")
        {
            return Err(PgError::SourceDivergence);
        }
        let mut item = edge(
            tx,
            tenant,
            project,
            auth,
            &work,
            &row.get::<_, String>("workstream_id"),
            &contract,
            &row.get::<_, String>("provider"),
        )
        .await?;
        item["work_title"] = json!(row.get::<_, String>("title"));
        item["dependency_position"] = json!(row.get::<_, i64>("position"));
        items.push(item);
    }
    let next = if more {
        next_cursor(
            &binding,
            "",
            0,
            i32::try_from(offset + limit - 1).map_err(|_| PgError::ResponseTooLarge)?,
        )
    } else {
        Value::Null
    };
    let mut result = json!({"protocol_version":1,"scope_id":"main","coordinator_epoch":auth.epoch,
        "project_status":auth.project_status,"source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string(),
        "data":{"work_id":q.work_id,"items":items,"total":total,"next_cursor":next,"truncated":more,
            "count_scope":"authorized_matches","count_limited":true,"hidden_count":null,
            "execution_authorized":false,"as_of":{"at_unix_ms":stamp.get::<_,i64>(1),
                "source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string()}}});
    if let Some(id) = bound {
        let selected = auth
            .catalog
            .workstreams
            .iter()
            .find(|s| s.id == id)
            .ok_or(PgError::Forbidden)?;
        result["workstream_id"] = json!(id.to_string());
        result["authority_version"] = json!(selected.authority_version.to_string());
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
