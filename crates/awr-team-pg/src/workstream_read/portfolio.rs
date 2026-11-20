//! One member-scoped portfolio summary across every readable project.
//!
//! Observer pages that compare projects need one read instead of one
//! project-scoped read per project. This module evaluates the same admission
//! as `authorize_project_entry` for every member project in one batched
//! statement, then aggregates works, waits, completions and the latest period
//! update for the whole page in a second batched statement. Field rules and
//! disclosure boundaries are documented in
//! `docs/dev/reference/portfolio-overview.md`.
//!
//! Round-trip budget: `BEGIN`, admission statement, facts statement, `COMMIT`
//! — four client-server exchanges regardless of project count. Per-project
//! query loops are a defect; the counting test pins this. Unlike the locking
//! read paths, admission takes no row locks here: a read-only repeatable-read
//! snapshot already linearizes a concurrent grant or membership change before
//! or after the whole read.
use super::*;
use awr_core::WorkstreamAction;
use std::collections::{BTreeMap, BTreeSet};

/// Maximum projects one page may carry (acceptance bound).
pub const PORTFOLIO_PAGE_MAX: u16 = 200;

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortfolioOverviewRequest {
    pub protocol_version: u32,
    pub limit: Option<u16>,
    pub cursor: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PortfolioCursor {
    binding: String,
    project_id: String,
}

/// The admission statement resolves credential, actor, tenant, memberships,
/// projects, workstream modes, catalogs, grants and Agent authorizations in
/// one exchange. It requires the transaction scope markers, which the read
/// binds in the statement before it; row-level policies read those settings
/// at execution time, so folding the binding into this statement's own select
/// list would leave the policies evaluating the unset values. Every statement
/// of this read filters the project list itself; the markers only release the
/// single-project pin of the isolation policies.
const ADMISSION_QUERY: &str = "SELECT
    (SELECT version FROM awr_team.schema_state WHERE component='awr_team') AS schema_version,
    c.actor_id, a.kind AS actor_kind, c.client_id,
    (SELECT COALESCE(jsonb_agg(jsonb_build_object(
            'project_id',p.id,'project_key',p.key,'status',p.status,
            'coordinator_epoch',p.coordinator_epoch,'project_revision',p.project_revision,
            'active_snapshot_id',p.active_snapshot_id,
            'role',m.role,
            'business_roles',m.business_roles,
            'independent_review',m.independent_review,
            'agent_review',m.agent_review,'assignment_grant',m.assignment_grant,
            'mode_enabled',(SELECT wm.enabled FROM awr_team.workstream_modes wm
                WHERE wm.tenant_id=p.tenant_id AND wm.project_id=p.id),
            'catalog_json',(SELECT wc.catalog_json FROM awr_team.workstream_catalogs wc
                WHERE wc.tenant_id=p.tenant_id AND wc.project_id=p.id
                  AND wc.snapshot_id=p.active_snapshot_id),
            'grants',(SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'workstream_id',g.workstream_id,'authority_version',g.authority_version,
                    'can_read',g.can_read,'can_write',g.can_write,'can_manage',g.can_manage,
                    'grant_version',g.grant_version) ORDER BY g.workstream_id),'[]'::jsonb)
                FROM awr_team.workstream_grants g
                WHERE g.tenant_id=p.tenant_id AND g.project_id=p.id
                  AND g.actor_id=c.actor_id AND g.client_id=c.client_id AND g.active),
            'authorizations',(SELECT COALESCE(jsonb_agg(az.body_json
                    ORDER BY az.created_at_ms, az.id),'[]'::jsonb)
                FROM awr_team.agent_authorizations az
                WHERE az.tenant_id=p.tenant_id AND az.project_id=p.id
                  AND az.subject_id=c.actor_id AND az.status='active'),
            'bindings',(SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'id',b.id,'binding_status',b.status,'person_id',b.person_id,
                    'agent_id',b.agent_id,'person_status',ps.status)),'[]'::jsonb)
                FROM awr_team.person_agent_bindings b
                LEFT JOIN awr_team.persons ps ON ps.tenant_id=b.tenant_id
                  AND ps.project_id=b.project_id AND ps.id=b.person_id
                WHERE b.tenant_id=p.tenant_id AND b.project_id=p.id)
        ) ORDER BY p.id),'[]'::jsonb)
      FROM awr_team.project_memberships m
      JOIN awr_team.projects p ON p.tenant_id=m.tenant_id AND p.id=m.project_id
      WHERE m.tenant_id=$1 AND m.actor_id=c.actor_id
        AND (c.project_id IS NULL OR m.project_id=c.project_id)
        AND p.id=ANY($4)) AS projects
FROM awr_team.credentials c
JOIN awr_team.actors a ON a.tenant_id=c.tenant_id AND a.id=c.actor_id
JOIN awr_team.tenants t ON t.id=c.tenant_id
WHERE c.tenant_id=$1 AND c.id=$2 AND c.secret_hash=$3
  AND c.revoked_at IS NULL
  AND (c.expires_at IS NULL OR c.expires_at>clock_timestamp())
  AND t.status='active' AND a.status='active'";

/// The facts statement aggregates per-work rows for the whole page at once.
/// `scope` carries each project's active snapshot. Per-work objects use short
/// keys because they are internal aggregates consumed here, never reply
/// fields: `w` work id, `s` workstream id, `d` definition state, `r` runtime
/// state, `sel` selected completion id, `p` latest progress phase, `dep` the
/// current dependency-wait run start, `cmp` the selected completion's
/// independence basis, `wd` open-decision start, `wr` open-review start.
const FACTS_QUERY: &str = "WITH scope AS (
    SELECT project_id,snapshot_id FROM jsonb_to_recordset($2) AS s(project_id text,snapshot_id text)
),
works AS (
    SELECT o.project_id,o.work_id,o.workstream_id,c.definition_state,r.state,r.selected_completion_id
    FROM scope sc
    JOIN awr_team.workstream_snapshot_ownership o ON o.tenant_id=$1
      AND o.project_id=sc.project_id AND o.snapshot_id=sc.snapshot_id AND o.scope_id='main'
    JOIN awr_team.work_contracts c ON c.tenant_id=o.tenant_id AND c.project_id=o.project_id
      AND c.snapshot_id=o.snapshot_id AND c.scope_id=o.scope_id AND c.work_id=o.work_id
    LEFT JOIN awr_team.work_runtime r ON r.tenant_id=o.tenant_id AND r.project_id=o.project_id
      AND r.scope_id=o.scope_id AND r.work_id=o.work_id
),
ordered_checkpoints AS (
    SELECT o.project_id,o.work_id,cp.progress_json->>'phase' AS phase,
        (extract(epoch FROM cp.created_at)*1000)::bigint AS at_ms,
        lag(cp.progress_json->>'phase') OVER (
            PARTITION BY o.tenant_id,o.project_id,o.work_id
            ORDER BY cp.observed_revision,cp.created_at,cp.id) AS previous_phase,
        row_number() OVER (
            PARTITION BY o.tenant_id,o.project_id,o.work_id
            ORDER BY cp.observed_revision DESC,cp.created_at DESC,cp.id DESC) AS recency
    FROM scope sc
    JOIN awr_team.workstream_snapshot_ownership o ON o.tenant_id=$1
      AND o.project_id=sc.project_id AND o.snapshot_id=sc.snapshot_id AND o.scope_id='main'
    JOIN awr_team.sessions s ON s.tenant_id=o.tenant_id AND s.project_id=o.project_id
      AND s.scope_id=o.scope_id AND s.work_id=o.work_id
      AND s.workstream_id=o.workstream_id AND s.ownership_version=o.ownership_version
    JOIN awr_team.checkpoints cp ON cp.tenant_id=s.tenant_id AND cp.project_id=s.project_id
      AND cp.session_id=s.id AND cp.progress_json IS NOT NULL
),
latest_phase AS (
    SELECT project_id,work_id,phase FROM ordered_checkpoints WHERE recency=1
),
dependency_open AS (
    SELECT project_id,work_id,MAX(at_ms) AS opened_ms
    FROM ordered_checkpoints
    WHERE phase='waiting_dependency'
      AND previous_phase IS DISTINCT FROM 'waiting_dependency'
    GROUP BY project_id,work_id
),
decisions AS (
    SELECT w.project_id,w.work_id,(extract(epoch FROM wi.created_at)*1000)::bigint AS opened_ms
    FROM works w
    JOIN awr_team.wait_items wi ON wi.tenant_id=$1 AND wi.project_id=w.project_id
      AND wi.work_id=w.work_id
    WHERE wi.decision_key IS NOT NULL AND wi.state='open'
),
reviews AS (
    SELECT w.project_id,w.work_id,(extract(epoch FROM rr.created_at)*1000)::bigint AS opened_ms
    FROM works w
    JOIN awr_team.review_rounds rr ON rr.tenant_id=$1 AND rr.project_id=w.project_id
      AND rr.work_id=w.work_id AND rr.state='open'
),
completion_basis AS (
    SELECT w.project_id,w.work_id,
        e.payload_json->>'independence_kind' AS independence_kind,
        (e.payload_json->>'team_independent_acceptance')::boolean AS team_independent_acceptance,
        e.payload_json->>'policy' AS policy
    FROM works w
    JOIN LATERAL (
        SELECT payload_json FROM awr_team.events e
        WHERE e.tenant_id=$1 AND e.project_id=w.project_id AND e.work_id=w.work_id
          AND e.event_type='work.completed'
          AND (w.selected_completion_id IS NULL
               OR e.payload_json->>'receipt_id'=w.selected_completion_id)
        ORDER BY (w.selected_completion_id IS NOT NULL
              AND e.payload_json->>'receipt_id'=w.selected_completion_id) DESC,
                 e.created_at DESC
        LIMIT 1
    ) e ON true
    WHERE w.state='completed'
),
latest_updates AS (
    SELECT DISTINCT ON (c.project_id) c.project_id,u.id,u.period_kind,u.period_key,
        u.starts_on::text,u.ends_on::text,u.status,u.author_person_id,u.author_display_name,
        (extract(epoch FROM u.recorded_at)*1000)::bigint AS recorded_ms
    FROM awr_team.project_period_update_current c
    JOIN awr_team.project_period_updates u ON u.tenant_id=c.tenant_id
      AND u.project_id=c.project_id AND u.id=c.update_id
    WHERE c.tenant_id=$1 AND c.project_id=ANY($3)
    ORDER BY c.project_id,u.starts_on DESC,u.ends_on DESC,u.period_key DESC
),
directories AS (
    SELECT project_id,plan_json FROM awr_team.project_directories
    WHERE tenant_id=$1 AND project_id=ANY($3)
)
SELECT sc.project_id,
    (SELECT COALESCE(jsonb_agg(jsonb_build_object(
            'w',w.work_id,'s',w.workstream_id,'d',w.definition_state,
            'r',w.state,'sel',w.selected_completion_id,
            'p',lp.phase,'dep',do_.opened_ms,
            'cmp',CASE WHEN cb.work_id IS NULL THEN NULL ELSE jsonb_build_object(
                'k',cb.independence_kind,'a',cb.team_independent_acceptance,
                'policy',cb.policy) END,
            'wd',de.opened_ms,'wr',rv.opened_ms) ORDER BY w.work_id),'[]'::jsonb)
        FROM works w
        LEFT JOIN latest_phase lp ON lp.project_id=w.project_id AND lp.work_id=w.work_id
        LEFT JOIN dependency_open do_ ON do_.project_id=w.project_id AND do_.work_id=w.work_id
        LEFT JOIN decisions de ON de.project_id=w.project_id AND de.work_id=w.work_id
        LEFT JOIN reviews rv ON rv.project_id=w.project_id AND rv.work_id=w.work_id
        LEFT JOIN completion_basis cb ON cb.project_id=w.project_id AND cb.work_id=w.work_id
        WHERE w.project_id=sc.project_id),
    (SELECT jsonb_build_object('update_id',lu.id,'period_kind',lu.period_kind,
            'period_key',lu.period_key,'starts_on',lu.starts_on,'ends_on',lu.ends_on,
            'status',lu.status,
            'author',jsonb_build_object('person_id',lu.author_person_id,
                'display_name',lu.author_display_name),
            'recorded_at_unix_ms',lu.recorded_ms)
        FROM latest_updates lu WHERE lu.project_id=sc.project_id),
    (SELECT plan_json FROM directories d WHERE d.project_id=sc.project_id),
    (extract(epoch FROM transaction_timestamp())*1000)::bigint AS as_of_ms
FROM scope sc";

/// One member project as resolved by the admission statement.
struct ProjectAdmission {
    id: String,
    key: String,
    status: String,
    epoch: String,
    revision: i64,
    #[allow(dead_code)]
    snapshot: Option<String>,
    /// `Some(reason)` marks the item `not_read`; `None` means admitted.
    denial: Option<&'static str>,
    /// Streams whose grants currently authorize a read.
    grant_streams: BTreeSet<String>,
    /// Grant version per stream, so authority changes expire cursors.
    grant_versions: BTreeMap<String, i64>,
    /// Streams a live Agent read delegation additionally covers; equals
    /// `grant_streams` for human and system actors.
    streams: BTreeSet<String>,
    /// Task allowances for Agent readers: work ids with a Task-scope read
    /// delegation. Their works count only inside grant-readable streams.
    tasks: BTreeSet<String>,
}

impl ProjectAdmission {
    fn partial(project: &Value, denial: &'static str) -> Self {
        Self {
            id: project["project_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            key: project["project_key"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            status: project["status"].as_str().unwrap_or_default().to_owned(),
            epoch: project["coordinator_epoch"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            revision: project["project_revision"].as_i64().unwrap_or_default(),
            snapshot: None,
            denial: Some(denial),
            grant_streams: BTreeSet::new(),
            grant_versions: BTreeMap::new(),
            streams: BTreeSet::new(),
            tasks: BTreeSet::new(),
        }
    }
}

/// Evaluate the same entry admission as `authorize_project_entry`, on data the
/// admission statement already loaded. A failure degrades this project to a
/// `not_read` item instead of refusing the whole portfolio; a project without
/// the member's membership row never reached this code.
fn admit(
    actor_kind: &str,
    actor: &str,
    client: &str,
    project: &Value,
    now_ms: i64,
) -> PgResult<ProjectAdmission> {
    let mut admission = ProjectAdmission {
        id: project["project_id"]
            .as_str()
            .ok_or(PgError::SourceDivergence)?
            .to_owned(),
        key: project["project_key"]
            .as_str()
            .ok_or(PgError::SourceDivergence)?
            .to_owned(),
        status: project["status"]
            .as_str()
            .ok_or(PgError::SourceDivergence)?
            .to_owned(),
        epoch: project["coordinator_epoch"]
            .as_str()
            .ok_or(PgError::SourceDivergence)?
            .to_owned(),
        revision: project["project_revision"]
            .as_i64()
            .ok_or(PgError::SourceDivergence)?,
        snapshot: project["active_snapshot_id"].as_str().map(str::to_owned),
        denial: None,
        grant_streams: BTreeSet::new(),
        grant_versions: BTreeMap::new(),
        streams: BTreeSet::new(),
        tasks: BTreeSet::new(),
    };
    if project["mode_enabled"] != json!(true) || admission.snapshot.is_none() {
        admission.denial = Some("no_active_snapshot");
        return Ok(admission);
    }
    let catalog: awr_core::WorkstreamCatalog =
        serde_json::from_value(project["catalog_json"].clone())
            .map_err(|_| PgError::SourceDivergence)?;
    catalog.validate().map_err(|_| PgError::SourceDivergence)?;
    if catalog.project_id != admission.id {
        return Err(PgError::SourceDivergence);
    }
    // Grants are rebuilt exactly as authentication loads them; only the read
    // decision is ever taken on them here.
    let mut grants = Vec::new();
    let mut grant_versions: BTreeMap<String, i64> = BTreeMap::new();
    for grant in project["grants"]
        .as_array()
        .ok_or(PgError::SourceDivergence)?
    {
        let id: awr_core::Id = grant["workstream_id"]
            .as_str()
            .and_then(|raw| raw.parse().ok())
            .ok_or(PgError::SourceDivergence)?;
        grants.push(awr_core::WorkstreamGrant {
            workstream_id: id,
            authority_version: grant["authority_version"].as_i64().unwrap_or_default()
                as awr_core::Revision,
            read: grant["can_read"].as_bool().unwrap_or(false),
            write: grant["can_write"].as_bool().unwrap_or(false),
            manage: grant["can_manage"].as_bool().unwrap_or(false),
        });
        grant_versions.insert(
            id.to_string(),
            grant["grant_version"].as_i64().unwrap_or_default(),
        );
    }
    let access = awr_core::WorkstreamAccess {
        project_id: admission.id.clone(),
        subject: client.to_owned(),
        grants,
    };
    access.validate().map_err(|_| PgError::SourceDivergence)?;
    let grant_streams: BTreeSet<String> = catalog
        .workstreams
        .iter()
        .filter(|stream| {
            access
                .authorize(&catalog, stream.id, WorkstreamAction::Read)
                .is_ok()
        })
        .map(|stream| stream.id.to_string())
        .collect();
    let role = project["role"].as_str().ok_or(PgError::SourceDivergence)?;
    let business_roles = crate::workstream_auth::decode_business_roles(
        project
            .get("business_roles")
            .filter(|value| !value.is_null())
            .cloned(),
    )?;
    let template =
        crate::workstream_auth::map_membership_role(role).ok_or(PgError::SourceDivergence)?;
    let ceiling = crate::workstream_auth::membership_action_ceiling(
        role,
        template,
        actor_kind,
        project["independent_review"].as_bool().unwrap_or(false),
        project["agent_review"].as_bool().unwrap_or(false),
        project["assignment_grant"].as_bool().unwrap_or(false),
        business_roles.as_ref(),
    );
    if !ceiling.contains(&awr_team::Action::WorkRead) || grant_streams.is_empty() {
        admission.denial = Some("no_read_authority");
        return Ok(admission);
    }
    let mut streams = grant_streams.clone();
    let mut tasks: BTreeSet<String> = BTreeSet::new();
    if crate::delegation_auth::actor_requires_explicit_delegation(actor_kind) {
        // Same live-delegation resolution as the entry probe: filter, narrow
        // children out of parents, then map Project/Workstream readers onto
        // streams and Task readers onto their works' grant-readable streams.
        let mut bindings: BTreeMap<String, (String, String, String, bool)> = BTreeMap::new();
        for row in project["bindings"]
            .as_array()
            .ok_or(PgError::SourceDivergence)?
        {
            if let (Some(id), Some(status), Some(agent), Some(person)) = (
                row["id"].as_str(),
                row["binding_status"].as_str(),
                row["agent_id"].as_str(),
                row["person_id"].as_str(),
            ) {
                bindings.insert(
                    id.to_owned(),
                    (
                        status.to_owned(),
                        agent.to_owned(),
                        person.to_owned(),
                        row["person_status"] == json!("active"),
                    ),
                );
            }
        }
        let mut candidates = Vec::new();
        for row in project["authorizations"]
            .as_array()
            .ok_or(PgError::SourceDivergence)?
        {
            let grant: awr_core::AgentAuthorization =
                serde_json::from_value(row.clone()).map_err(|_| PgError::SourceDivergence)?;
            if !matches!(grant.subject_kind, awr_core::ExecutionSubjectKind::Agent)
                || grant.subject_id != actor
                || !matches!(grant.status, awr_core::AuthorizationStatus::Active)
                || !grant.is_effective_at(now_ms)
                || awr_core::bind_runtime_identity(&grant, client, None, None, now_ms).is_err()
                || grant.scope.project_id() != admission.id
            {
                continue;
            }
            let covered = grant.binding_id.as_deref().is_some_and(|binding| {
                bindings
                    .get(binding)
                    .is_some_and(|(status, agent, person, person_active)| {
                        status == "active"
                            && *person_active
                            && agent == actor
                            && *person == grant.responsible_person_id.as_str()
                    })
            });
            if !covered {
                continue;
            }
            let mapped = crate::delegation_auth::tmcp_actions_for_authorized_set(&grant.actions)
                .intersection(&ceiling)
                .copied()
                .collect::<BTreeSet<_>>();
            candidates.push((grant, mapped));
        }
        let narrowed: BTreeSet<String> = candidates
            .iter()
            .filter_map(|(child, child_actions)| {
                let parent_id = child.parent_authorization_id.as_deref()?;
                candidates.iter().find_map(|(parent, parent_actions)| {
                    (parent.id == parent_id
                        && child_actions.is_subset(parent_actions)
                        && child.scope.is_within(&parent.scope)
                        && (child_actions.len() < parent_actions.len()
                            || child.scope != parent.scope))
                        .then(|| parent_id.to_owned())
                })
            })
            .collect();
        let live: Vec<(&awr_core::AgentAuthorization, &BTreeSet<awr_team::Action>)> = candidates
            .iter()
            .filter(|(grant, _)| !narrowed.contains(&grant.id))
            .map(|(grant, actions)| (grant, actions))
            .collect();
        let readers: Vec<&awr_core::AgentAuthorization> = live
            .iter()
            .filter(|(grant, actions)| {
                actions.contains(&awr_team::Action::WorkRead)
                    && matches!(
                        grant.scope,
                        awr_core::AuthorizationScope::Project { .. }
                            | awr_core::AuthorizationScope::Workstream { .. }
                    )
            })
            .map(|(grant, _)| *grant)
            .collect();
        streams.retain(|stream| {
            stream.parse::<awr_core::Id>().is_ok_and(|id| {
                readers.iter().any(|grant| {
                    crate::delegation_auth::covers_stream(grant, id)
                        && access
                            .authorize(&catalog, id, WorkstreamAction::Read)
                            .is_ok()
                })
            })
        });
        for (grant, actions) in live {
            if !actions.contains(&awr_team::Action::WorkRead) {
                continue;
            }
            if let awr_core::AuthorizationScope::Task { work_item_id, .. } = &grant.scope {
                tasks.insert(work_item_id.clone());
            }
        }
        if streams.is_empty() && tasks.is_empty() {
            admission.denial = Some("no_read_authority");
            return Ok(admission);
        }
    }
    admission.grant_streams = grant_streams;
    admission.grant_versions = grant_versions;
    admission.streams = streams;
    admission.tasks = tasks;
    Ok(admission)
}

#[derive(Default)]
struct WorkCounts {
    total: i64,
    not_started: i64,
    in_progress: i64,
    delivered_unverified: i64,
    verified: i64,
    completed_without_independent_review: i64,
    retired: i64,
    hidden: i64,
    decision: i64,
    dependency: i64,
    review: i64,
    oldest_open: Option<i64>,
    /// Whether a task allowance landed inside a grant-readable stream.
    task_covered: bool,
}

fn fold_work(
    counts: &mut WorkCounts,
    work: &Value,
    visible: bool,
    task_allowed: bool,
) -> PgResult<()> {
    if !visible {
        counts.hidden += 1;
        return Ok(());
    }
    if task_allowed {
        counts.task_covered = true;
    }
    counts.total += 1;
    let definition = work["d"].as_str().unwrap_or_default();
    let runtime = work["r"].as_str();
    let phase = work["p"].as_str();
    let mut opened: Vec<i64> = Vec::new();
    if let Some(at) = work["wd"].as_i64() {
        counts.decision += 1;
        opened.push(at);
    }
    if let Some(at) = work["wr"].as_i64() {
        counts.review += 1;
        opened.push(at);
    }
    if phase == Some("waiting_dependency") {
        counts.dependency += 1;
        if let Some(at) = work["dep"].as_i64() {
            opened.push(at);
        }
    }
    counts.oldest_open = [counts.oldest_open, opened.iter().copied().min()]
        .into_iter()
        .flatten()
        .min();
    if matches!(definition, "archived" | "cancelled") {
        counts.retired += 1;
    } else if runtime == Some("completed") {
        let basis = &work["cmp"];
        if crate::completion_independence::independently_accepted(
            basis["k"].as_str(),
            basis["a"].as_bool(),
            basis["policy"].as_str(),
        ) {
            counts.verified += 1;
        } else {
            counts.completed_without_independent_review += 1;
        }
    } else if phase == Some("delivered") {
        counts.delivered_unverified += 1;
    } else if runtime.is_some() {
        counts.in_progress += 1;
    } else {
        counts.not_started += 1;
    }
    Ok(())
}

/// Leaf departments are groups; the ancestor chain above the group lists the
/// departments outermost first. Plans stored before hierarchy support carry
/// no `parent_id`, so every department is a leaf and the chain stays empty.
fn directory_summary(plan_json: Option<&Value>) -> Value {
    let Some(plan) = plan_json else {
        return json!({"registered":false,"group":null,"departments":null});
    };
    let departments = plan["departments"].as_array().cloned().unwrap_or_default();
    let parents: BTreeSet<&str> = departments
        .iter()
        .filter_map(|unit| unit["parent_id"].as_str())
        .collect();
    let by_id: BTreeMap<&str, &Value> = departments
        .iter()
        .filter_map(|unit| unit["id"].as_str().map(|id| (id, unit)))
        .collect();
    let attached_ids: Vec<String> = plan["project_department_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    let attached: Vec<&str> = attached_ids.iter().map(String::as_str).collect();
    let leaves: Vec<&str> = attached
        .iter()
        .filter(|id| !parents.contains(**id) && by_id.contains_key(**id))
        .copied()
        .collect();
    let group = leaves
        .first()
        .map(|id| json!({"id":id,"name":by_id[id]["name"].as_str().unwrap_or_default()}));
    let mut chain = Vec::new();
    if let Some(group_id) = leaves.first().copied() {
        let mut seen = BTreeSet::from([group_id]);
        let mut current = by_id
            .get(group_id)
            .and_then(|unit| unit["parent_id"].as_str());
        while let Some(parent) = current {
            if !seen.insert(parent) {
                chain.clear();
                break;
            }
            match by_id.get(parent) {
                Some(unit) => {
                    chain.push(json!({
                        "id":parent,
                        "name":unit["name"].as_str().unwrap_or_default()}));
                    current = unit["parent_id"].as_str();
                }
                None => break,
            }
        }
        chain.reverse();
    }
    json!({"registered":true,"group":group,"departments":chain})
}

impl WorkstreamReadStore {
    /// One member-scoped portfolio summary. `registered` names the projects
    /// the serving instance exposes for this tenant; the tenant comes from the
    /// transport and the bearer identifies the member.
    pub async fn portfolio_overview(
        &self,
        tenant: &str,
        bearer: &str,
        registered: &[String],
        request: PortfolioOverviewRequest,
    ) -> PgResult<Value> {
        let limit = usize::from(request.limit.unwrap_or(PORTFOLIO_PAGE_MAX));
        if request.protocol_version != 1 {
            return Err(PgError::Unsupported(
                "portfolio overview protocol version".into(),
            ));
        }
        if limit == 0 || limit > usize::from(PORTFOLIO_PAGE_MAX) {
            return Err(PgError::query_field("limit", "limit"));
        }
        if request
            .cursor
            .as_deref()
            .is_some_and(|cursor| cursor.len() > 4096)
        {
            return Err(PgError::query_field("cursor", "cursor"));
        }
        let reference = crate::workstream_auth::credential_reference(bearer)?;
        let mut client = self.pool.get().await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .await?;
        // Scope markers first: one small exchange. Policies evaluate these at
        // execution time, so they must be bound before any guarded read. Both
        // are transaction-local and only this read sets portfolio_read.
        tx.execute(
            "SELECT set_config('awr.tenant_id',$1,true),
                set_config('awr.portfolio_read','on',true)",
            &[&tenant],
        )
        .await?;
        // Admission exchange: every member project's security facts, plus the
        // schema guard other read entries enforce separately.
        let admission_row = tx
            .query_opt(
                ADMISSION_QUERY,
                &[
                    &tenant,
                    &reference.credential_id,
                    &reference.secret_hash,
                    &registered,
                ],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        let schema: i32 = admission_row.get(0);
        if schema != crate::migrate::EXPECTED_SCHEMA_VERSION {
            return Err(PgError::SchemaIncompatible(format!(
                "schema version {schema}, expected {}",
                crate::migrate::EXPECTED_SCHEMA_VERSION
            )));
        }
        let actor: String = admission_row.get(1);
        let actor_kind: String = admission_row.get(2);
        let client_id: String = admission_row.get(3);
        let projects: Vec<Value> = admission_row
            .get::<_, Value>(4)
            .as_array()
            .ok_or(PgError::SourceDivergence)?
            .clone();
        if projects.is_empty() {
            // A credential without a single membership has no member scope;
            // the web entry refuses it the same way.
            return Err(PgError::Forbidden);
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|span| span.as_millis() as i64)
            .unwrap_or_default();
        // One admission per member project; a divergent row degrades to a
        // `not_read` item while the remaining projects stay readable.
        let mut admissions: Vec<ProjectAdmission> = Vec::with_capacity(projects.len());
        for project in &projects {
            match admit(&actor_kind, &actor, &client_id, project, now_ms) {
                Ok(admission) => admissions.push(admission),
                Err(PgError::SourceDivergence) => {
                    admissions.push(ProjectAdmission::partial(project, "read_failed"));
                }
                Err(error) => return Err(error),
            }
        }
        admissions.sort_by(|left, right| left.id.cmp(&right.id));
        let binding = hash(&json!({
            "domain":"awr-team-portfolio-overview-v1",
            "reader":{"actor":actor,"client":client_id,"actor_kind":actor_kind,
                "credential":reference.credential_id},
            "page":{"limit":limit},
            "projects":admissions.iter().map(|project| json!({
                "project_id":project.id,"status":project.status,
                "revision":project.revision,"epoch":project.epoch,
                "snapshot":project.snapshot,
                "streams":project.streams.iter().collect::<Vec<_>>(),
                "grant_versions":project.grant_versions,
                "tasks":project.tasks.iter().collect::<Vec<_>>()}))
                .collect::<Vec<_>>()}))?;
        let start_key = match &request.cursor {
            None => String::new(),
            Some(raw) => {
                let cursor: PortfolioCursor =
                    serde_json::from_str(raw).map_err(|_| PgError::CursorExpired)?;
                if cursor.binding != binding || cursor.project_id.is_empty() {
                    return Err(PgError::CursorExpired);
                }
                cursor.project_id
            }
        };
        let total_projects = admissions.len();
        let page: Vec<&ProjectAdmission> = admissions
            .iter()
            .filter(|project| project.id > start_key)
            .take(limit)
            .collect();
        let truncated = page
            .last()
            .is_some_and(|last| admissions.iter().any(|project| project.id > last.id));
        let next_cursor = if truncated {
            Value::String(
                serde_json::to_string(&PortfolioCursor {
                    binding,
                    project_id: page.last().expect("truncated page is nonempty").id.clone(),
                })
                .map_err(|_| PgError::SourceDivergence)?,
            )
        } else {
            Value::Null
        };
        // Facts exchange: one batched statement for the whole page. Projects
        // admitted only through task allowances are fetched too; the item
        // downgrades to `not_read` when no allowed task lands in a
        // grant-readable stream.
        let readable: Vec<&&ProjectAdmission> = page
            .iter()
            .filter(|project| project.denial.is_none())
            .collect();
        let scope: Vec<Value> = readable
            .iter()
            .map(|project| {
                json!({"project_id":project.id,
                    "snapshot_id":project
                        .snapshot
                        .clone()
                        .expect("admitted projects carry a snapshot")})
            })
            .collect();
        let scope_ids: Vec<String> = readable.iter().map(|project| project.id.clone()).collect();
        let mut facts: BTreeMap<String, (Value, Option<Value>, Option<Value>)> = BTreeMap::new();
        let mut as_of_ms: Option<i64> = None;
        if !scope.is_empty() {
            for row in tx
                .query(FACTS_QUERY, &[&tenant, &json!(scope), &scope_ids])
                .await?
            {
                let project: String = row.get(0);
                facts.insert(
                    project,
                    (
                        row.get::<_, Option<Value>>(1).unwrap_or_else(|| json!([])),
                        row.get(2),
                        row.get(3),
                    ),
                );
                as_of_ms = Some(row.get::<_, i64>(4));
            }
        }
        tx.commit().await?;
        let mut items = Vec::with_capacity(page.len());
        for project in page {
            let Some(_) = project.denial else {
                let (works_json, update_json, directory_json) = facts
                    .get(&project.id)
                    .cloned()
                    .ok_or(PgError::SourceDivergence)?;
                let mut counts = WorkCounts::default();
                for work in works_json.as_array().ok_or(PgError::SourceDivergence)? {
                    let stream = work["s"].as_str().unwrap_or_default();
                    let task_allowed = project
                        .tasks
                        .contains(work["w"].as_str().unwrap_or_default());
                    let visible = project.streams.contains(stream)
                        || (task_allowed && project.grant_streams.contains(stream));
                    fold_work(&mut counts, work, visible, task_allowed)?;
                }
                if !project.streams.is_empty() || !project.tasks.is_empty() && counts.task_covered {
                    items.push(json!({
                        "project_id":project.id,"project_key":project.key,
                        "project_status":project.status,"coordinator_epoch":project.epoch,
                        "project_revision":project.revision.to_string(),
                        "directory":directory_summary(directory_json.as_ref()),
                        "responsible_person":Value::Null,
                        "works":{"total":counts.total,"not_started":counts.not_started,
                            "in_progress":counts.in_progress,
                            "delivered_unverified":counts.delivered_unverified,
                            "verified":counts.verified,
                            "completed_without_independent_review":
                                counts.completed_without_independent_review,
                            "retired":counts.retired,
                            "hidden_work_count":counts.hidden},
                        "waits":{"decision":counts.decision,
                            "dependency":counts.dependency,"review":counts.review,
                            "oldest_open_unix_ms":counts.oldest_open},
                        "latest_update":update_json.clone().unwrap_or(Value::Null),
                        "read":{"status":"complete"}}));
                    continue;
                }
                items.push(json!({
                    "project_id":project.id,"project_key":project.key,
                    "project_status":project.status,"coordinator_epoch":project.epoch,
                    "project_revision":project.revision.to_string(),
                    "read":{"status":"not_read","reason":"no_read_authority"}}));
                continue;
            };
            items.push(json!({
                "project_id":project.id,"project_key":project.key,
                "project_status":project.status,"coordinator_epoch":project.epoch,
                "project_revision":project.revision.to_string(),
                "read":{"status":"not_read","reason":project.denial.expect("checked above")}}));
        }
        let result = json!({
            "protocol_version":1,"op":"portfolio.overview","selection_basis":"member",
            "data":{"projects":items,"total_projects":total_projects,
                "truncated":truncated,"next_cursor":next_cursor,
                "as_of":{"at_unix_ms":as_of_ms},"execution_authorized":false}});
        if serde_json::to_vec(&result)
            .map_err(|_| PgError::SourceDivergence)?
            .len()
            > 1_048_576
        {
            return Err(PgError::ResponseTooLarge);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_departments_are_groups_and_chains_stop_before_the_hierarchy() {
        let flat = json!({"organization":{"id":"o","name":"O"},
            "departments":[{"id":"eng","name":"Engineering"}],
            "project_department_ids":["eng"],"members":[]});
        assert_eq!(
            directory_summary(Some(&flat)),
            json!({"registered":true,
                "group":{"id":"eng","name":"Engineering"},
                "departments":[]})
        );
        let hierarchical = json!({"organization":{"id":"o","name":"O"},
            "departments":[{"id":"eng","name":"Engineering"},
                {"id":"platform","name":"Platform","parent_id":"eng"}],
            "project_department_ids":["platform"],"members":[]});
        assert_eq!(
            directory_summary(Some(&hierarchical)),
            json!({"registered":true,
                "group":{"id":"platform","name":"Platform"},
                "departments":[{"id":"eng","name":"Engineering"}]})
        );
        assert_eq!(
            directory_summary(None),
            json!({"registered":false,"group":null,"departments":null})
        );
    }

    #[test]
    fn work_buckets_partition_the_visible_set() {
        let mut counts = WorkCounts::default();
        let works = [
            json!({"w":"a","s":"1","d":"enabled","r":null,"p":null}),
            json!({"w":"b","s":"1","d":"enabled","r":"claimed","p":"implementing"}),
            json!({"w":"c","s":"1","d":"enabled","r":"claimed","p":"delivered"}),
            json!({"w":"d","s":"1","d":"enabled","r":"completed","p":"delivered",
                "cmp":{"k":"team_independent","a":true}}),
            json!({"w":"e","s":"1","d":"enabled","r":"completed","p":null,"cmp":null}),
            json!({"w":"f","s":"1","d":"cancelled","r":"claimed","p":null}),
            json!({"w":"g","s":"2","d":"enabled","r":"claimed","p":null}),
        ];
        for work in &works {
            fold_work(&mut counts, work, work["s"] != "2", false).unwrap();
        }
        assert_eq!(counts.total, 6);
        assert_eq!(counts.not_started, 1);
        assert_eq!(counts.in_progress, 1);
        assert_eq!(counts.delivered_unverified, 1);
        assert_eq!(counts.verified, 1);
        assert_eq!(counts.completed_without_independent_review, 1);
        assert_eq!(counts.retired, 1);
        assert_eq!(counts.hidden, 1);
        assert_eq!(
            counts.total,
            counts.not_started
                + counts.in_progress
                + counts.delivered_unverified
                + counts.verified
                + counts.completed_without_independent_review
                + counts.retired
        );
    }

    #[test]
    fn waits_count_only_visible_works_with_their_oldest_open_time() {
        let mut counts = WorkCounts::default();
        fold_work(
            &mut counts,
            &json!({"w":"a","s":"1","d":"enabled","r":"claimed","p":"waiting_dependency",
                "dep":100,"wd":50}),
            true,
            false,
        )
        .unwrap();
        fold_work(
            &mut counts,
            &json!({"w":"b","s":"1","d":"enabled","r":"claimed","p":"implementing","wr":10}),
            true,
            false,
        )
        .unwrap();
        fold_work(
            &mut counts,
            &json!({"w":"c","s":"2","d":"enabled","r":"claimed","wd":1}),
            false,
            false,
        )
        .unwrap();
        assert_eq!(counts.decision, 1);
        assert_eq!(counts.dependency, 1);
        assert_eq!(counts.review, 1);
        assert_eq!(counts.oldest_open, Some(10));
        assert_eq!(counts.hidden, 1);
    }
}
