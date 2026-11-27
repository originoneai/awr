//! Read-only Team usage and Agent session duration summary (`usage.summary`).
//!
//! Team PostgreSQL does not collect the personal-edition SQLite usage ledger
//! (per-call receipts, corrections, allocations, billing cost or execution
//! intervals). It stores optional caller-declared `host_session` token counters
//! on checkpoints (`checkpoints.usage_json`) and session lifecycle timestamps
//! on committed events (`events`). Missing observations always report
//! `"not_collected"` with `null` values, never zero. Member-granularity rows
//! stay ordered by stable identifier without personal comparison ranking and
//! are visible only to self and members holding `audit.read_project`.
use super::*;
use crate::workstream_auth::authorize_domain_action;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GroupRef {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GroupCoverage {
    pub(crate) registered: bool,
    pub(crate) collected: Vec<GroupRef>,
    pub(crate) missing: Vec<GroupRef>,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionUsageRow {
    pub(crate) work_id: String,
    pub(crate) workstream_id: String,
    pub(crate) actor_id: String,
    pub(crate) state: String,
    pub(crate) start_ms: Option<i64>,
    pub(crate) end_ms: Option<i64>,
    pub(crate) has_usage: bool,
    pub(crate) usage_complete: bool,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) cached_input_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UsageRollup {
    pub(crate) session_count: usize,
    pub(crate) active_session_count: usize,
    pub(crate) intervals: Vec<(i64, i64)>,
    pub(crate) sessions_without_duration: usize,
    pub(crate) sessions_with_usage: usize,
    pub(crate) sessions_without_usage: usize,
    pub(crate) any_partial_usage_coverage: bool,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) cached_input_tokens: Option<u64>,
}

impl UsageRollup {
    pub(crate) fn add(&mut self, session: &SessionUsageRow) {
        self.session_count += 1;
        if session.state == "active" {
            self.active_session_count += 1;
        }
        match (session.start_ms, session.end_ms) {
            (Some(start), Some(end)) if end >= start => self.intervals.push((start, end)),
            _ => self.sessions_without_duration += 1,
        }
        if session.has_usage {
            self.sessions_with_usage += 1;
            if !session.usage_complete {
                self.any_partial_usage_coverage = true;
            }
            add_opt_u64(&mut self.input_tokens, session.input_tokens);
            add_opt_u64(&mut self.output_tokens, session.output_tokens);
            add_opt_u64(&mut self.cached_input_tokens, session.cached_input_tokens);
        } else {
            self.sessions_without_usage += 1;
        }
    }

    pub(crate) fn duration_status(&self) -> &'static str {
        if self.intervals.is_empty() {
            "not_collected"
        } else if self.sessions_without_duration == 0 {
            "collected"
        } else {
            "partial"
        }
    }

    pub(crate) fn tokens_status(&self) -> &'static str {
        if self.sessions_with_usage == 0 {
            "not_collected"
        } else if self.sessions_without_usage == 0 && !self.any_partial_usage_coverage {
            "collected"
        } else {
            "partial"
        }
    }

    pub(crate) fn duration_json(&self) -> Value {
        if self.intervals.is_empty() {
            return json!({
                "status": "not_collected",
                "summed_ms": Value::Null,
                "wall_clock_ms": Value::Null,
                "sessions_with_duration": 0,
                "sessions_without_duration": self.sessions_without_duration
            });
        }
        let summed_ms = self.intervals.iter().fold(0_i64, |acc, (s, e)| {
            acc.saturating_add(e.saturating_sub(*s))
        });
        let wall_clock_ms = merged_wall_clock_ms(&self.intervals);
        json!({
            "status": self.duration_status(),
            "summed_ms": summed_ms,
            "wall_clock_ms": wall_clock_ms,
            "sessions_with_duration": self.intervals.len(),
            "sessions_without_duration": self.sessions_without_duration
        })
    }

    pub(crate) fn tokens_json(&self) -> Value {
        if self.sessions_with_usage == 0 {
            return json!({
                "status": "not_collected",
                "input_tokens": Value::Null,
                "output_tokens": Value::Null,
                "cached_input_tokens": Value::Null,
                "sessions_with_usage": 0,
                "sessions_without_usage": self.sessions_without_usage
            });
        }
        json!({
            "status": self.tokens_status(),
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "cached_input_tokens": self.cached_input_tokens,
            "sessions_with_usage": self.sessions_with_usage,
            "sessions_without_usage": self.sessions_without_usage
        })
    }

    pub(crate) fn cost_json() -> Value {
        json!({
            "status": "not_collected",
            "amount_micros": Value::Null,
            "currency": Value::Null
        })
    }

    pub(crate) fn summary_json(&self) -> Value {
        json!({
            "session_count": self.session_count,
            "active_session_count": self.active_session_count,
            "duration": self.duration_json(),
            "tokens": self.tokens_json(),
            "cost": Self::cost_json()
        })
    }
}

fn add_opt_u64(target: &mut Option<u64>, value: Option<u64>) {
    if let Some(v) = value {
        *target = Some(target.unwrap_or(0).saturating_add(v));
    }
}

/// Union length of closed `[start_ms, end_ms]` intervals so parallel sessions
/// never collapse wall-clock time into summed execution duration.
pub(crate) fn merged_wall_clock_ms(intervals: &[(i64, i64)]) -> i64 {
    if intervals.is_empty() {
        return 0;
    }
    let mut sorted = intervals.to_vec();
    sorted.sort_unstable();
    let mut total: i64 = 0;
    let (mut cur_start, mut cur_end) = sorted[0];
    for &(start, end) in &sorted[1..] {
        if start <= cur_end {
            cur_end = cur_end.max(end);
        } else {
            total = total.saturating_add(cur_end.saturating_sub(cur_start));
            cur_start = start;
            cur_end = end;
        }
    }
    total.saturating_add(cur_end.saturating_sub(cur_start))
}

/// Partition registered project groups (leaf departments, falling back to
/// referenced departments when flat) into collected vs missing groups.
pub(crate) fn summarize_group_coverage(
    plan: Option<&awr_team::ProjectDirectoryPlan>,
    collected_person_ids: &BTreeSet<String>,
    any_unmapped_usage: bool,
) -> GroupCoverage {
    let Some(plan) = plan else {
        return GroupCoverage::default();
    };
    let parents: BTreeSet<&str> = plan
        .departments
        .iter()
        .filter_map(|unit| unit.parent_id.as_deref())
        .collect();
    let by_id: BTreeMap<&str, &awr_team::DirectoryDepartment> = plan
        .departments
        .iter()
        .map(|unit| (unit.id.as_str(), unit))
        .collect();
    fn leaf_or_all<'a>(
        ids: &'a [String],
        parents: &BTreeSet<&str>,
        by_id: &BTreeMap<&str, &awr_team::DirectoryDepartment>,
    ) -> Vec<&'a str> {
        let leaves: Vec<&'a str> = ids
            .iter()
            .filter(|id| !parents.contains(id.as_str()) && by_id.contains_key(id.as_str()))
            .map(String::as_str)
            .collect();
        if !leaves.is_empty() {
            leaves
        } else {
            ids.iter()
                .filter(|id| by_id.contains_key(id.as_str()))
                .map(String::as_str)
                .collect()
        }
    }
    let project_groups = leaf_or_all(&plan.project_department_ids, &parents, &by_id);
    let mut all_group_ids: BTreeSet<&str> = project_groups.iter().copied().collect();
    let mut collected_ids: BTreeSet<&str> = BTreeSet::new();
    let mut mapped_people: BTreeSet<&str> = BTreeSet::new();
    for member in &plan.members {
        let groups = leaf_or_all(&member.department_ids, &parents, &by_id);
        if !groups.is_empty() {
            mapped_people.insert(member.person_id.as_str());
        }
        for group_id in groups {
            all_group_ids.insert(group_id);
            if collected_person_ids.contains(&member.person_id) {
                collected_ids.insert(group_id);
            }
        }
    }
    let has_unmapped_person_usage = collected_person_ids
        .iter()
        .any(|person_id| !mapped_people.contains(person_id.as_str()));
    if (any_unmapped_usage || has_unmapped_person_usage) && project_groups.len() == 1 {
        collected_ids.insert(project_groups[0]);
    }
    let mut collected = Vec::new();
    let mut missing = Vec::new();
    for group_id in all_group_ids {
        let group = GroupRef {
            id: group_id.to_owned(),
            name: by_id[group_id].name.clone(),
        };
        if collected_ids.contains(group_id) {
            collected.push(group);
        } else {
            missing.push(group);
        }
    }
    GroupCoverage {
        registered: true,
        collected,
        missing,
    }
}

/// Overall usage coverage status derived from token rollup, group gaps, member
/// gaps, and hidden works. Never reports `"complete"` when any group, member
/// or visible session lacks host usage or any work is hidden.
pub(crate) fn overall_coverage_status(
    tokens_status: &str,
    missing_groups: usize,
    missing_members: usize,
    hidden_works: i64,
) -> &'static str {
    if tokens_status == "not_collected" {
        "not_collected"
    } else if tokens_status == "collected"
        && missing_groups == 0
        && missing_members == 0
        && hidden_works == 0
    {
        "complete"
    } else {
        "partial"
    }
}

/// Portfolio-level project usage summary built from the batched portfolio
/// query without exposing any individual member usage or adding DB round trips.
pub(crate) fn portfolio_project_usage(
    sessions_json: &Value,
    plan_json: Option<&Value>,
    visible_streams: &BTreeSet<String>,
    grant_streams: &BTreeSet<String>,
    allowed_tasks: &BTreeSet<String>,
    hidden_works: i64,
) -> Value {
    let plan = plan_json.and_then(|value| {
        serde_json::from_value::<awr_team::ProjectDirectoryPlan>(value.clone()).ok()
    });
    let mut rollup = UsageRollup::default();
    let mut collected_people: BTreeSet<String> = BTreeSet::new();
    let mut any_unmapped_usage = false;
    if let Some(items) = sessions_json.as_array() {
        for item in items {
            let stream = item["s"].as_str().unwrap_or_default();
            let work = item["w"].as_str().unwrap_or_default();
            let visible = visible_streams.contains(stream)
                || (allowed_tasks.contains(work) && grant_streams.contains(stream));
            if !visible {
                continue;
            }
            let row = SessionUsageRow {
                work_id: work.to_owned(),
                workstream_id: stream.to_owned(),
                actor_id: item["a"].as_str().unwrap_or_default().to_owned(),
                state: item["st"].as_str().unwrap_or_default().to_owned(),
                start_ms: item["t0"].as_i64(),
                end_ms: item["t1"].as_i64(),
                has_usage: item["u"].as_bool().unwrap_or(false),
                usage_complete: item["uc"].as_bool().unwrap_or(false),
                input_tokens: item["in"].as_u64(),
                output_tokens: item["out"].as_u64(),
                cached_input_tokens: item["cache"].as_u64(),
            };
            if row.has_usage {
                if let Some(person_id) = item["p"].as_str() {
                    collected_people.insert(person_id.to_owned());
                } else {
                    any_unmapped_usage = true;
                }
            }
            rollup.add(&row);
        }
    }
    let groups = summarize_group_coverage(plan.as_ref(), &collected_people, any_unmapped_usage);
    let usage_status = match rollup.tokens_status() {
        "collected" if groups.missing.is_empty() && hidden_works == 0 => "complete",
        "not_collected" => "not_collected",
        _ => "partial",
    };
    let duration_status = match rollup.duration_status() {
        "collected" if hidden_works == 0 => "complete",
        "not_collected" => "not_collected",
        _ => "partial",
    };
    let status = overall_coverage_status(
        rollup.tokens_status(),
        groups.missing.len(),
        0,
        hidden_works,
    );
    json!({
        "status": status,
        "cost_status": "not_collected",
        "usage_status": usage_status,
        "duration_status": duration_status,
        "groups": {
            "registered": groups.registered,
            "collected": groups.collected.iter().map(|g| json!({"id":g.id,"name":g.name})).collect::<Vec<_>>(),
            "missing": groups.missing.iter().map(|g| json!({"id":g.id,"name":g.name})).collect::<Vec<_>>()
        }
    })
}

fn non_neg_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|v| u64::try_from(v).ok())
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    directory_auth: Option<&ReaderAuthority>,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let (bound_stream, selection_basis) = if let Some(work) = q.work_id.as_deref() {
        let (binding, _) = work_binding(tx, tenant, project, auth, work).await?;
        if q.workstream_id
            .is_some_and(|id| id != binding.workstream_id)
        {
            return Err(PgError::Forbidden);
        }
        (Some(binding.workstream_id.to_string()), "work")
    } else if let Some(stream) = q.workstream_id {
        auth.access
            .authorize(&auth.catalog, stream, WorkstreamAction::Read)
            .map_err(|_| PgError::Forbidden)?;
        if let Some(scope) = &auth.navigation_read_scope
            && !scope.streams.contains(&stream)
        {
            return Err(PgError::Forbidden);
        }
        (Some(stream.to_string()), "explicit")
    } else {
        (None, "project")
    };

    let (streams, task_ids, task_streams) = navigation::visible_work_scope(auth);
    let can_audit =
        authorize_domain_action(auth, awr_team::Action::AuditReadProject, None, None).is_ok();

    let as_of_ms: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM transaction_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);

    let hidden_work_count: i64 = if q.work_id.is_some() || bound_stream.is_some() {
        0
    } else {
        tx.query_one(
            "SELECT COUNT(*)::bigint
             FROM awr_team.workstream_snapshot_ownership o
             WHERE o.tenant_id=$1 AND o.project_id=$2 AND o.snapshot_id=$3 AND o.scope_id='main'
               AND NOT (o.workstream_id=ANY($4) OR EXISTS(
                   SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
                   WHERE allowed.work_id=o.work_id AND allowed.workstream_id=o.workstream_id))",
            &[
                &tenant,
                &project,
                &auth.snapshot,
                &streams,
                &task_ids,
                &task_streams,
            ],
        )
        .await?
        .get(0)
    };

    // Visible works in the requested scope so tasks with zero sessions stay
    // explicitly reported as not_collected rather than silently omitted.
    let visible_works = tx
        .query(
            "SELECT o.work_id, o.workstream_id
             FROM awr_team.workstream_snapshot_ownership o
             WHERE o.tenant_id=$1 AND o.project_id=$2 AND o.snapshot_id=$3 AND o.scope_id='main'
               AND (o.workstream_id=ANY($4) OR EXISTS(
                   SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
                   WHERE allowed.work_id=o.work_id AND allowed.workstream_id=o.workstream_id))
               AND ($7::text IS NULL OR o.work_id=$7)
               AND ($8::text IS NULL OR o.workstream_id=$8)
             ORDER BY o.work_id ASC",
            &[
                &tenant,
                &project,
                &auth.snapshot,
                &streams,
                &task_ids,
                &task_streams,
                &q.work_id,
                &bound_stream,
            ],
        )
        .await?;

    // Per-session observed timestamps and cumulative host-session counters.
    // Within a session, counters are monotonic per (source, counter_id); a new
    // counter_id indicates an explicit host reset, so per-counter maxima sum
    // across resets while null dimensions remain null (never zero).
    let session_rows = tx
        .query(
            "WITH visible_sessions AS (
                SELECT s.id, s.work_id, s.workstream_id, s.actor_id, s.state
                FROM awr_team.sessions s
                JOIN awr_team.workstream_snapshot_ownership o
                  ON o.tenant_id=s.tenant_id AND o.project_id=s.project_id
                 AND o.snapshot_id=$3 AND o.scope_id=s.scope_id
                 AND o.work_id=s.work_id AND o.workstream_id=s.workstream_id
                 AND o.ownership_version=s.ownership_version
                WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.scope_id='main'
                  AND (o.workstream_id=ANY($4) OR EXISTS(
                      SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
                      WHERE allowed.work_id=o.work_id AND allowed.workstream_id=o.workstream_id))
                  AND ($7::text IS NULL OR o.work_id=$7)
                  AND ($8::text IS NULL OR o.workstream_id=$8)
            ),
            session_times AS (
                SELECT vs.id AS session_id,
                    MIN(CASE WHEN e.event_type IN ('session.start','task.accept_assignment','task.claim_available')
                        THEN (extract(epoch FROM e.created_at)*1000)::bigint END) AS start_ms,
                    MAX(CASE WHEN e.event_type='session.end'
                        THEN (extract(epoch FROM e.created_at)*1000)::bigint END) AS ended_ms
                FROM visible_sessions vs
                LEFT JOIN awr_team.events e
                  ON e.tenant_id=$1 AND e.project_id=$2
                 AND e.work_id=vs.work_id AND e.workstream_id=vs.workstream_id
                 AND e.payload_json->>'session_id'=vs.id
                GROUP BY vs.id
            ),
            counter_peaks AS (
                SELECT c.session_id,
                    c.usage_json->>'source' AS source,
                    c.usage_json->>'counter_id' AS counter_id,
                    MAX((c.usage_json->>'input_tokens')::bigint) AS input_tokens,
                    MAX((c.usage_json->>'output_tokens')::bigint) AS output_tokens,
                    MAX((c.usage_json->>'cached_input_tokens')::bigint) AS cached_input_tokens,
                    BOOL_AND(c.usage_json->>'coverage' = 'complete') AS all_complete
                FROM visible_sessions vs
                JOIN awr_team.checkpoints c
                  ON c.tenant_id=$1 AND c.project_id=$2 AND c.session_id=vs.id
                 AND c.usage_json IS NOT NULL
                GROUP BY c.session_id, c.usage_json->>'source', c.usage_json->>'counter_id'
            ),
            session_usage AS (
                SELECT session_id,
                    TRUE AS has_usage,
                    BOOL_AND(all_complete) AS usage_complete,
                    SUM(input_tokens)::bigint AS input_tokens,
                    SUM(output_tokens)::bigint AS output_tokens,
                    SUM(cached_input_tokens)::bigint AS cached_input_tokens
                FROM counter_peaks
                GROUP BY session_id
            )
            SELECT vs.work_id, vs.workstream_id, vs.actor_id, vs.state,
                st.start_ms,
                CASE
                    WHEN vs.state='ended' THEN st.ended_ms
                    WHEN vs.state='active' AND st.start_ms IS NOT NULL THEN GREATEST(st.start_ms, $9::bigint)
                    ELSE NULL
                END AS end_ms,
                COALESCE(su.has_usage, FALSE) AS has_usage,
                COALESCE(su.usage_complete, FALSE) AS usage_complete,
                su.input_tokens,
                su.output_tokens,
                su.cached_input_tokens
            FROM visible_sessions vs
            LEFT JOIN session_times st ON st.session_id=vs.id
            LEFT JOIN session_usage su ON su.session_id=vs.id
            ORDER BY vs.work_id ASC, vs.id ASC",
            &[
                &tenant,
                &project,
                &auth.snapshot,
                &streams,
                &task_ids,
                &task_streams,
                &q.work_id,
                &bound_stream,
                &as_of_ms,
            ],
        )
        .await?;

    let sessions: Vec<SessionUsageRow> = session_rows
        .into_iter()
        .map(|row| SessionUsageRow {
            work_id: row.get(0),
            workstream_id: row.get(1),
            actor_id: row.get(2),
            state: row.get(3),
            start_ms: row.get(4),
            end_ms: row.get(5),
            has_usage: row.get(6),
            usage_complete: row.get(7),
            input_tokens: non_neg_u64(row.get(8)),
            output_tokens: non_neg_u64(row.get(9)),
            cached_input_tokens: non_neg_u64(row.get(10)),
        })
        .collect();

    // Directory plan for group coverage and registered members.
    let plan: Option<awr_team::ProjectDirectoryPlan> = tx
        .query_opt(
            "SELECT plan_json FROM awr_team.project_directories WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?
        .and_then(|row| {
            serde_json::from_value::<awr_team::ProjectDirectoryPlan>(row.get::<_, Value>(0)).ok()
        });

    // Known project actors for coverage and identity resolution.
    let membership_actors: Vec<String> = tx
        .query(
            "SELECT m.actor_id
             FROM awr_team.project_memberships m
             JOIN awr_team.actors a ON a.tenant_id=m.tenant_id AND a.id=m.actor_id
             WHERE m.tenant_id=$1 AND m.project_id=$2 AND a.status='active'
             ORDER BY m.actor_id ASC",
            &[&tenant, &project],
        )
        .await?
        .into_iter()
        .map(|row| row.get(0))
        .collect();

    let mut actor_ids: BTreeSet<String> = membership_actors.iter().cloned().collect();
    actor_ids.insert(auth.actor_id.clone());
    for session in &sessions {
        actor_ids.insert(session.actor_id.clone());
    }
    if let Some(requested) = q.member_actor_id.as_deref() {
        actor_ids.insert(requested.to_owned());
    }
    let actor_list: Vec<String> = actor_ids.into_iter().collect();
    let resolved_actors =
        crate::review::resolve_person_ids(tx, tenant, project, &actor_list).await?;

    let member_key_for_actor = |actor_id: &str| -> (String, &'static str, Option<String>) {
        if let Some(resolved) = resolved_actors.get(actor_id)
            && let Some((person_id, _)) = resolved.active_person()
        {
            return (person_id.to_owned(), "person", Some(person_id.to_owned()));
        }
        (actor_id.to_owned(), "actor", None)
    };

    let (self_member_id, _, _) = member_key_for_actor(&auth.actor_id);

    // Privacy gate for explicit member_actor_id filter: non-audit callers can
    // inspect only their own member identity; requesting any other member is
    // refused with Forbidden.
    let requested_member_id = if let Some(requested) = q.member_actor_id.as_deref() {
        let (target_key, _, _) = member_key_for_actor(requested);
        let is_self = requested == auth.actor_id || target_key == self_member_id;
        if !can_audit && !is_self {
            return Err(PgError::Forbidden);
        }
        Some(target_key)
    } else {
        None
    };

    // Name disclosure matches directory.read (same boundary as activity.rs).
    let disclose_names = match directory_auth {
        Some(authority) => {
            let mut gate = authority.clone();
            match crate::directory::authorize_read(tx, &mut gate, project).await {
                Ok(()) => true,
                Err(PgError::Forbidden) => false,
                Err(error) => return Err(error),
            }
        }
        None => false,
    };

    // Load active person display names for directory members (who may not have
    // a matching actor row yet).
    let directory_person_ids: Vec<String> = plan
        .as_ref()
        .map(|p| p.members.iter().map(|m| m.person_id.clone()).collect())
        .unwrap_or_default();
    let mut person_names: BTreeMap<String, String> = BTreeMap::new();
    if !directory_person_ids.is_empty() {
        for row in tx
            .query(
                "SELECT id, display_name FROM awr_team.persons
                 WHERE tenant_id=$1 AND project_id=$2 AND id=ANY($3) AND status='active'",
                &[&tenant, &project, &directory_person_ids],
            )
            .await?
        {
            person_names.insert(row.get(0), row.get(1));
        }
    }
    for resolved in resolved_actors.values() {
        if let Some((person_id, name)) = resolved.active_person() {
            person_names
                .entry(person_id.to_owned())
                .or_insert_with(|| name.to_owned());
        }
    }

    let person_group_ids: BTreeMap<String, Vec<String>> = plan
        .as_ref()
        .map(|p| {
            p.members
                .iter()
                .map(|m| (m.person_id.clone(), m.department_ids.clone()))
                .collect()
        })
        .unwrap_or_default();

    // Roll up project, task, and member aggregates. BTreeMap guarantees stable
    // identifier-ascending order everywhere (never personal-comparison ranking).
    let mut project_rollup = UsageRollup::default();
    let mut by_task: BTreeMap<String, (String, UsageRollup)> = BTreeMap::new();
    for row in visible_works {
        let work_id: String = row.get(0);
        let workstream_id: String = row.get(1);
        by_task.insert(work_id, (workstream_id, UsageRollup::default()));
    }

    struct MemberEntry {
        member_id: String,
        member_kind: &'static str,
        person_id: Option<String>,
        actor_ids: BTreeSet<String>,
        group_ids: Vec<String>,
        rollup: UsageRollup,
    }
    let mut members: BTreeMap<String, MemberEntry> = BTreeMap::new();

    // Seed known project members from active memberships and project directory
    // so members without any sessions appear in `coverage.members.missing`.
    for actor_id in &membership_actors {
        let (member_id, member_kind, person_id) = member_key_for_actor(actor_id);
        let entry = members
            .entry(member_id.clone())
            .or_insert_with(|| MemberEntry {
                member_id: member_id.clone(),
                member_kind,
                group_ids: person_id
                    .as_ref()
                    .and_then(|id| person_group_ids.get(id).cloned())
                    .unwrap_or_default(),
                person_id,
                actor_ids: BTreeSet::new(),
                rollup: UsageRollup::default(),
            });
        entry.actor_ids.insert(actor_id.clone());
    }
    if let Some(dir_plan) = &plan {
        for dir_member in &dir_plan.members {
            members
                .entry(dir_member.person_id.clone())
                .or_insert_with(|| MemberEntry {
                    member_id: dir_member.person_id.clone(),
                    member_kind: "person",
                    person_id: Some(dir_member.person_id.clone()),
                    actor_ids: BTreeSet::new(),
                    group_ids: dir_member.department_ids.clone(),
                    rollup: UsageRollup::default(),
                });
        }
    }

    let mut collected_person_ids: BTreeSet<String> = BTreeSet::new();
    let mut any_unmapped_usage = false;
    for session in &sessions {
        project_rollup.add(session);
        by_task
            .entry(session.work_id.clone())
            .or_insert_with(|| (session.workstream_id.clone(), UsageRollup::default()))
            .1
            .add(session);
        let (member_id, member_kind, person_id) = member_key_for_actor(&session.actor_id);
        if session.has_usage {
            if let Some(pid) = &person_id {
                collected_person_ids.insert(pid.clone());
            } else {
                any_unmapped_usage = true;
            }
        }
        let entry = members
            .entry(member_id.clone())
            .or_insert_with(|| MemberEntry {
                member_id: member_id.clone(),
                member_kind,
                group_ids: person_id
                    .as_ref()
                    .and_then(|id| person_group_ids.get(id).cloned())
                    .unwrap_or_default(),
                person_id,
                actor_ids: BTreeSet::new(),
                rollup: UsageRollup::default(),
            });
        entry.actor_ids.insert(session.actor_id.clone());
        entry.rollup.add(session);
    }

    let groups = summarize_group_coverage(plan.as_ref(), &collected_person_ids, any_unmapped_usage);
    let total_missing_members = members
        .values()
        .filter(|entry| entry.rollup.sessions_with_usage == 0)
        .count();

    let visible_name = |entry: &MemberEntry| -> Option<String> {
        if disclose_names || entry.member_id == self_member_id {
            entry
                .person_id
                .as_ref()
                .and_then(|pid| person_names.get(pid).cloned())
        } else {
            None
        }
    };

    let mut by_member_json = Vec::new();
    let mut members_collected = Vec::new();
    let mut members_missing = Vec::new();
    let mut hidden_member_count: usize = 0;

    for entry in members.values() {
        let allowed_for_caller = can_audit || entry.member_id == self_member_id;
        if !allowed_for_caller {
            hidden_member_count += 1;
            continue;
        }
        let coverage_item = json!({
            "member_id": entry.member_id,
            "member_kind": entry.member_kind,
            "person_id": entry.person_id,
            "display_name": visible_name(entry),
            "group_ids": entry.group_ids
        });
        if entry.rollup.sessions_with_usage > 0 {
            members_collected.push(coverage_item);
        } else {
            members_missing.push(coverage_item);
        }
        if requested_member_id
            .as_deref()
            .is_some_and(|target| target != entry.member_id)
        {
            continue;
        }
        by_member_json.push(json!({
            "member_id": entry.member_id,
            "member_kind": entry.member_kind,
            "person_id": entry.person_id,
            "actor_ids": entry.actor_ids.iter().cloned().collect::<Vec<_>>(),
            "display_name": visible_name(entry),
            "group_ids": entry.group_ids,
            "session_count": entry.rollup.session_count,
            "active_session_count": entry.rollup.active_session_count,
            "duration": entry.rollup.duration_json(),
            "tokens": entry.rollup.tokens_json(),
            "cost": UsageRollup::cost_json()
        }));
    }

    let by_task_json: Vec<Value> = by_task
        .into_iter()
        .map(|(work_id, (workstream_id, rollup))| {
            json!({
                "work_id": work_id,
                "workstream_id": workstream_id,
                "session_count": rollup.session_count,
                "active_session_count": rollup.active_session_count,
                "duration": rollup.duration_json(),
                "tokens": rollup.tokens_json(),
                "cost": UsageRollup::cost_json()
            })
        })
        .collect();

    let usage_status = overall_coverage_status(
        project_rollup.tokens_status(),
        groups.missing.len(),
        total_missing_members,
        hidden_work_count,
    );
    let duration_status = match project_rollup.duration_status() {
        "collected" if hidden_work_count == 0 => "complete",
        "not_collected" => "not_collected",
        _ => "partial",
    };

    let data = json!({
        "project_id": project,
        "scope": if q.work_id.is_some() {
            "work"
        } else if bound_stream.is_some() {
            "workstream"
        } else {
            "project"
        },
        "work_id": q.work_id,
        "workstream_id": bound_stream,
        "collection": {
            "personal_ledger_collected": false,
            "billing_collected": false,
            "cost_status": "not_collected",
            "usage_scope": "host_session",
            "usage_provenance": "caller_declared",
            "duration_basis": "recorded_session_events",
            "is_historical_observation": true,
            "is_not_estimated_remaining_time": true
        },
        "project": project_rollup.summary_json(),
        "by_task": by_task_json,
        "by_member": by_member_json,
        "member_scope": if can_audit { "project" } else { "self" },
        "member_ordering": "identifier_asc",
        "personal_comparison_ranking": false,
        "coverage": {
            "status": usage_status,
            "cost_status": "not_collected",
            "usage_status": usage_status,
            "duration_status": duration_status,
            "hidden_work_count": hidden_work_count,
            "groups": {
                "registered": groups.registered,
                "collected": groups.collected.iter().map(|g| json!({"id":g.id,"name":g.name})).collect::<Vec<_>>(),
                "missing": groups.missing.iter().map(|g| json!({"id":g.id,"name":g.name})).collect::<Vec<_>>()
            },
            "members": {
                "scope": if can_audit { "project" } else { "self" },
                "collected": members_collected,
                "missing": members_missing,
                "hidden_member_count": hidden_member_count
            }
        },
        "as_of": {
            "at_unix_ms": as_of_ms,
            "source_snapshot_id": auth.snapshot,
            "project_revision": auth.revision.to_string()
        },
        "execution_authorized": false
    });

    let result = json!({
        "protocol_version": 1,
        "scope_id": "main",
        "selection_basis": selection_basis,
        "coordinator_epoch": auth.epoch,
        "project_status": auth.project_status,
        "source_snapshot_id": auth.snapshot,
        "project_revision": auth.revision.to_string(),
        "data": data
    });

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

    fn session(
        work: &str,
        actor: &str,
        start: Option<i64>,
        end: Option<i64>,
        input: Option<u64>,
        output: Option<u64>,
        cached: Option<u64>,
        complete: bool,
    ) -> SessionUsageRow {
        SessionUsageRow {
            work_id: work.into(),
            workstream_id: "00000000000000000000000001".into(),
            actor_id: actor.into(),
            state: "ended".into(),
            start_ms: start,
            end_ms: end,
            has_usage: input.is_some() || output.is_some(),
            usage_complete: complete,
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: cached,
        }
    }

    #[test]
    fn missing_observations_return_not_collected_and_null_never_zero() {
        let mut rollup = UsageRollup::default();
        rollup.add(&session("a", "dev-1", None, None, None, None, None, false));
        let summary = rollup.summary_json();
        assert_eq!(summary["duration"]["status"], "not_collected");
        assert!(summary["duration"]["summed_ms"].is_null());
        assert!(summary["duration"]["wall_clock_ms"].is_null());
        assert_eq!(summary["tokens"]["status"], "not_collected");
        assert!(summary["tokens"]["input_tokens"].is_null());
        assert!(summary["tokens"]["output_tokens"].is_null());
        assert!(summary["tokens"]["cached_input_tokens"].is_null());
        assert_eq!(summary["cost"]["status"], "not_collected");
        assert!(summary["cost"]["amount_micros"].is_null());
        assert!(summary["cost"]["currency"].is_null());
    }

    #[test]
    fn parallel_sessions_keep_wall_clock_distinct_from_summed_duration_and_preserve_null_dimensions()
     {
        let mut rollup = UsageRollup::default();
        rollup.add(&session(
            "a",
            "dev-1",
            Some(1_000),
            Some(5_000),
            Some(4_000),
            None,
            None,
            true,
        ));
        rollup.add(&session(
            "b",
            "dev-2",
            Some(2_000),
            Some(6_000),
            Some(1_500),
            Some(300),
            None,
            true,
        ));
        let summary = rollup.summary_json();
        assert_eq!(summary["duration"]["status"], "collected");
        assert_eq!(summary["duration"]["summed_ms"], 8_000);
        assert_eq!(summary["duration"]["wall_clock_ms"], 5_000);
        assert_eq!(summary["tokens"]["status"], "collected");
        assert_eq!(summary["tokens"]["input_tokens"], 5_500);
        assert_eq!(summary["tokens"]["output_tokens"], 300);
        assert!(summary["tokens"]["cached_input_tokens"].is_null());
    }

    #[test]
    fn group_coverage_partitions_all_registered_groups_without_dropping_missing_ones() {
        let plan: awr_team::ProjectDirectoryPlan = serde_json::from_value(json!({
            "organization": {"id": "org", "name": "Example Org"},
            "departments": [
                {"id": "dept-eng", "name": "Engineering"},
                {"id": "grp-app", "name": "App Group", "parent_id": "dept-eng"},
                {"id": "grp-platform", "name": "Platform Group", "parent_id": "dept-eng"},
                {"id": "grp-delivery", "name": "Delivery Group", "parent_id": "dept-eng"}
            ],
            "project_department_ids": ["grp-app"],
            "members": [
                {"person_id": "p-1", "department_ids": ["grp-app"]},
                {"person_id": "p-2", "department_ids": ["grp-platform"]},
                {"person_id": "p-3", "department_ids": ["grp-delivery"]}
            ]
        }))
        .unwrap();
        let collected = BTreeSet::from(["p-1".to_owned()]);
        let coverage = summarize_group_coverage(Some(&plan), &collected, false);
        assert!(coverage.registered);
        assert_eq!(
            coverage.collected,
            vec![GroupRef {
                id: "grp-app".into(),
                name: "App Group".into()
            }]
        );
        assert_eq!(
            coverage.missing,
            vec![
                GroupRef {
                    id: "grp-delivery".into(),
                    name: "Delivery Group".into()
                },
                GroupRef {
                    id: "grp-platform".into(),
                    name: "Platform Group".into()
                }
            ]
        );
        assert_eq!(
            overall_coverage_status("collected", coverage.missing.len(), 2, 0),
            "partial"
        );
    }
}
