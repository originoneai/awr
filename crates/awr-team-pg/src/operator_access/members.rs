use super::*;

impl ProjectAccessStore {
    /// Bounded keyset directory. A partially delegated administrator can inspect
    /// only clients whose complete active grant scope they manage.
    pub async fn members(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        after: Option<&str>,
        limit: u32,
    ) -> PgResult<Value> {
        if !(1..=100).contains(&limit) || after.is_some_and(|s| !identity(s)) {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate(&tx, tenant, project, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::AccessManageProject, None, None)?;
        require_project_manage_grant(&auth)?;
        let managed: Vec<String> = auth
            .access
            .grants
            .iter()
            .filter(|g| {
                auth.access
                    .authorize(&auth.catalog, g.workstream_id, WorkstreamAction::Manage)
                    .is_ok()
            })
            .map(|g| g.workstream_id.to_string())
            .collect();
        let rows = tx
            .query(
                "SELECT m.actor_id FROM awr_team.project_memberships m
             WHERE m.tenant_id=$1 AND m.project_id=$2 AND ($3::text IS NULL OR m.actor_id>$3)
               AND NOT EXISTS (SELECT 1 FROM awr_team.workstream_grants g
                 WHERE g.tenant_id=m.tenant_id AND g.project_id=m.project_id
                 AND g.actor_id=m.actor_id AND g.active AND NOT(g.workstream_id=ANY($4)))
             ORDER BY m.actor_id LIMIT $5",
                &[&tenant, &project, &after, &managed, &(i64::from(limit) + 1)],
            )
            .await?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for row in rows.iter().take(limit as usize) {
            let actor: String = row.get(0);
            let clients = tx
                .query(
                    "SELECT DISTINCT client_id FROM awr_team.workstream_grants
                 WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 ORDER BY client_id LIMIT 101",
                    &[&tenant, &project, &actor],
                )
                .await?;
            let mut bindings = Vec::new();
            for c in clients.iter().take(100) {
                let id: String = c.get(0);
                let mut state = snapshot(&tx, tenant, project, &actor, &id).await?;
                // Directory output is metadata, not a repeated project catalog.
                state.as_object_mut().unwrap().remove("catalog");
                bindings.push(state);
            }
            let a = tx.query_one(
                "SELECT a.display_name,a.kind,m.role,m.independent_review,m.agent_review,m.business_roles FROM awr_team.actors a
                 JOIN awr_team.project_memberships m ON m.tenant_id=a.tenant_id AND m.actor_id=a.id
                 WHERE a.tenant_id=$1 AND m.project_id=$2 AND a.id=$3",
                &[&tenant,&project,&actor],
            ).await?;
            let role: String = a.get(2);
            let kind: String = a.get(1);
            let roles = crate::workstream_auth::decode_business_roles(a.get(5))?;
            let template =
                crate::workstream_auth::map_membership_role(&role).ok_or(PgError::Forbidden)?;
            let mut item = json!({"actor_id":actor,"display_name":a.get::<_,String>(0),
                "kind":kind,"role":role,
                "membership_action_ceiling":crate::workstream_auth::membership_action_ceiling(
                    &role,template,&kind,a.get(3),a.get(4),roles.as_ref()),
                "independent_review":a.get::<_,bool>(3),"agent_review":a.get::<_,bool>(4),"clients":bindings,
                "clients_truncated":clients.len()>100});
            if let Some(roles) = a.get::<_, Option<Value>>(5) {
                item["business_roles"] = roles;
            }
            items.push(item);
        }
        let next = if more {
            items.last().map(|x| x["actor_id"].clone())
        } else {
            None
        };
        let streams: Vec<Value> = auth.catalog.workstreams.iter().filter(|s| managed.contains(&s.id.to_string()))
            .map(|s| json!({"id":s.id,"external_key":s.external_key,"authority_version":s.authority_version.to_string(),
                "write":caller_grant(&auth,s.id).is_some_and(|g|g.write)})).collect();
        tx.commit().await?;
        Ok(
            json!({"items":items,"next_cursor":next,"workstreams":streams,
            "actor_id":auth.actor_id,"raw_secrets_in_response":false}),
        )
    }
}

pub(super) async fn validate_credentials(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    plan: &AdminAccessPlan,
    state: &Value,
) -> PgResult<()> {
    if plan.remove_membership
        && (plan.credential.is_some() || !plan.revoke_project_credentials.is_empty())
    {
        return Err(invalid());
    }
    if let Some(c) = &plan.credential {
        let prior = tx.query_opt("SELECT project_id FROM awr_team.credentials WHERE tenant_id=$1 AND id=$2 FOR SHARE", &[&tenant,&c.id]).await?;
        if let Some(row) = prior {
            let scope: Option<String> = row.get(0);
            if scope.as_deref().is_some_and(|p| p != project)
                || plan.credential_project_scoped && scope.as_deref() != Some(project)
            {
                return Err(PgError::Forbidden);
            }
        }
    }
    for id in &plan.revoke_project_credentials {
        let valid = tx
            .query_opt(
                "SELECT id FROM awr_team.credentials WHERE tenant_id=$1 AND project_id=$2 AND id=$3
             AND actor_id=$4 AND client_id=$5 FOR SHARE",
                &[
                    &tenant,
                    &project,
                    id,
                    &plan.subject.id,
                    &plan.subject_client_id,
                ],
            )
            .await?
            .is_some();
        if !valid {
            return Err(PgError::Forbidden);
        }
    }
    let roles = effective_roles(plan, state)?;
    if state["membership"]["role"]
        .as_str()
        .is_some_and(|role| policy_can_manage(role, roles.as_ref()))
        && !plan.revoke_project_credentials.is_empty()
    {
        // A sole administrator must retain a usable credential during rotation.
        let other =
            other_live_manager_exists(tx, tenant, project, &plan.subject.id, &state["catalog"])
                .await?;
        let remaining: i64 = tx.query_one("SELECT count(*) FROM awr_team.credentials WHERE tenant_id=$1 AND actor_id=$2 AND (project_id IS NULL OR project_id=$3) AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) AND NOT(id=ANY($4)) AND EXISTS (SELECT 1 FROM awr_team.workstream_grants g WHERE g.tenant_id=credentials.tenant_id AND g.project_id=$3 AND g.actor_id=credentials.actor_id AND g.client_id=credentials.client_id AND g.active AND g.can_manage)", &[&tenant,&plan.subject.id,&project,&plan.revoke_project_credentials]).await?.get(0);
        if !other
            && remaining == 0
            && !(plan.credential.is_some() && plan.grants.iter().any(|g| g.manage))
        {
            return Err(PgError::Forbidden);
        }
    }
    Ok(())
}

pub(super) async fn apply_credentials(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    plan: &AdminAccessPlan,
) -> PgResult<()> {
    if plan.credential_project_scoped {
        let c = plan.credential.as_ref().ok_or_else(invalid)?;
        tx.execute("UPDATE awr_team.credentials SET project_id=$3 WHERE tenant_id=$1 AND id=$2 AND (project_id IS NULL OR project_id=$3)", &[&tenant,&c.id,&project]).await?;
    }
    for id in &plan.revoke_project_credentials {
        tx.execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND revoked_at IS NULL", &[&tenant,&project,id]).await?;
    }
    Ok(())
}
