//! Project-scoped periodic updates. The caller writes the judgment; the service
//! validates, attributes and versions it, and deterministically records task facts.
use super::*;

const UPDATE_PROTOCOL: &str = "awr-team-project-update-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRecordRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub period_kind: String,
    pub period_key: String,
    pub starts_on: String,
    pub ends_on: String,
    pub status: String,
    pub summary: String,
}

pub(crate) fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return false;
    }
    let number =
        |range: std::ops::Range<usize>| value[range].parse::<u32>().expect("ASCII date component");
    let year = number(0..4);
    let month = number(5..7);
    let day = number(8..10);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    year >= 1 && (1..=days).contains(&day)
}

fn valid_period(kind: &str, key: &str) -> bool {
    if key.is_empty() || key.len() > 64 || key.chars().any(char::is_control) {
        return false;
    }
    match kind {
        "week" => {
            let bytes = key.as_bytes();
            bytes.len() == 8
                && bytes[..4].iter().all(u8::is_ascii_digit)
                && &bytes[4..6] == b"-W"
                && bytes[6..].iter().all(u8::is_ascii_digit)
                && (1..=53).contains(&key[6..].parse::<u32>().expect("ASCII week"))
        }
        "month" => {
            let bytes = key.as_bytes();
            bytes.len() == 7
                && bytes[..4].iter().all(u8::is_ascii_digit)
                && bytes[4] == b'-'
                && bytes[5..].iter().all(u8::is_ascii_digit)
                && (1..=12).contains(&key[5..].parse::<u32>().expect("ASCII month"))
        }
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WaitKind {
    Decision,
    Dependency,
    Review,
    External,
    Unknown,
}

impl WaitKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Dependency => "dependency",
            Self::Review => "review",
            Self::External => "external",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FactEvent {
    Verified(String),
    CompletedWithoutIndependentReview(String),
    WaitOpened(String, WaitKind),
    WaitCleared(String, WaitKind),
    ReviewReturned(String),
    DeliveredUnverified(String),
    DecisionMade(String),
}

#[derive(Default)]
struct Facts {
    verified: std::collections::BTreeSet<String>,
    completed_without_review: std::collections::BTreeSet<String>,
    opened: std::collections::BTreeMap<WaitKind, Vec<String>>,
    cleared: std::collections::BTreeMap<WaitKind, Vec<String>>,
    returned: std::collections::BTreeSet<String>,
    delivered: std::collections::BTreeSet<String>,
    decisions: Vec<String>,
}

fn counted(mut work_ids: Vec<String>) -> Value {
    work_ids.sort();
    json!({"count":work_ids.len(),"work_ids":work_ids})
}

fn counted_set(work_ids: std::collections::BTreeSet<String>) -> Value {
    counted(work_ids.into_iter().collect())
}

fn waits(waits: std::collections::BTreeMap<WaitKind, Vec<String>>) -> Value {
    let kinds = waits
        .into_iter()
        .filter_map(|(kind, work_ids)| {
            if work_ids.is_empty() {
                return None;
            }
            let counted = counted(work_ids);
            Some(json!({"kind":kind.as_str(),"count":counted["count"],
                "work_ids":counted["work_ids"]}))
        })
        .collect::<Vec<_>>();
    let count = kinds
        .iter()
        .map(|kind| kind["count"].as_u64().unwrap_or(0))
        .sum::<u64>();
    json!({"count":count,"kinds":kinds})
}

fn build_facts(events: impl IntoIterator<Item = FactEvent>) -> Value {
    let mut facts = Facts::default();
    for event in events {
        match event {
            FactEvent::Verified(work) => {
                facts.verified.insert(work);
            }
            FactEvent::CompletedWithoutIndependentReview(work) => {
                facts.completed_without_review.insert(work);
            }
            FactEvent::WaitOpened(work, kind) => {
                facts.opened.entry(kind).or_default().push(work);
            }
            FactEvent::WaitCleared(work, kind) => {
                facts.cleared.entry(kind).or_default().push(work);
            }
            FactEvent::ReviewReturned(work) => {
                facts.returned.insert(work);
            }
            FactEvent::DeliveredUnverified(work) => {
                facts.delivered.insert(work);
            }
            FactEvent::DecisionMade(work) => facts.decisions.push(work),
        }
    }
    json!({
        "verified":counted_set(facts.verified),
        "completed_without_independent_review":counted_set(facts.completed_without_review),
        "waits_opened":waits(facts.opened),
        "waits_cleared":waits(facts.cleared),
        "review_returned":counted_set(facts.returned),
        "delivered_unverified":counted_set(facts.delivered),
        "decisions_made":counted(facts.decisions)
    })
}

fn stored_ids(value: &Value) -> PgResult<Vec<String>> {
    value
        .get("work_ids")
        .and_then(Value::as_array)
        .ok_or(PgError::SourceDivergence)?
        .iter()
        .map(|id| {
            id.as_str()
                .map(str::to_owned)
                .ok_or(PgError::SourceDivergence)
        })
        .collect()
}

fn project_counted(value: &Value, visible: &std::collections::BTreeSet<String>) -> PgResult<Value> {
    Ok(counted(
        stored_ids(value)?
            .into_iter()
            .filter(|id| visible.contains(id))
            .collect(),
    ))
}

fn project_waits(value: &Value, visible: &std::collections::BTreeSet<String>) -> PgResult<Value> {
    let kinds = value
        .get("kinds")
        .and_then(Value::as_array)
        .ok_or(PgError::SourceDivergence)?
        .iter()
        .map(|kind| {
            let name = kind
                .get("kind")
                .and_then(Value::as_str)
                .ok_or(PgError::SourceDivergence)?;
            let projected = project_counted(kind, visible)?;
            Ok(json!({"kind":name,"count":projected["count"],
                "work_ids":projected["work_ids"]}))
        })
        .collect::<PgResult<Vec<_>>>()?
        .into_iter()
        .filter(|kind| kind["count"] != 0)
        .collect::<Vec<_>>();
    let count = kinds
        .iter()
        .map(|kind| kind["count"].as_u64().unwrap_or(0))
        .sum::<u64>();
    Ok(json!({"count":count,"kinds":kinds}))
}

/// Project an immutable project snapshot through the reader's current work visibility.
/// The hidden count contains no task identifiers and the stored JSON is never changed.
pub(crate) fn project_fact_snapshot(
    stored: &Value,
    visible: &std::collections::BTreeSet<String>,
) -> PgResult<(Value, usize)> {
    let mut all = std::collections::BTreeSet::new();
    for field in [
        "verified",
        "completed_without_independent_review",
        "review_returned",
        "delivered_unverified",
        "decisions_made",
    ] {
        all.extend(stored_ids(&stored[field])?);
    }
    for field in ["waits_opened", "waits_cleared"] {
        for kind in stored[field]["kinds"]
            .as_array()
            .ok_or(PgError::SourceDivergence)?
        {
            all.extend(stored_ids(kind)?);
        }
    }
    let hidden = all.difference(visible).count();
    Ok((
        json!({
            "verified":project_counted(&stored["verified"],visible)?,
            "completed_without_independent_review":project_counted(
                &stored["completed_without_independent_review"],visible)?,
            "waits_opened":project_waits(&stored["waits_opened"],visible)?,
            "waits_cleared":project_waits(&stored["waits_cleared"],visible)?,
            "review_returned":project_counted(&stored["review_returned"],visible)?,
            "delivered_unverified":project_counted(&stored["delivered_unverified"],visible)?,
            "decisions_made":project_counted(&stored["decisions_made"],visible)?
        }),
        hidden,
    ))
}

struct ComputedFacts {
    json: Value,
    start_ms: i64,
    end_ms: i64,
    time_zone: String,
}

fn legacy_wait_kind(kind: Option<&str>) -> PgResult<WaitKind> {
    match kind {
        None => Ok(WaitKind::Unknown),
        Some("decision") => Ok(WaitKind::Decision),
        Some("external") => Ok(WaitKind::External),
        _ => Err(PgError::SourceDivergence),
    }
}

/// A completion is verified only by independent acceptance: the current payload
/// states `team_independent_acceptance`, or the older receipt names one of the
/// two agent/simulated-member independent review policies. Everything else —
/// including a payload with no basis at all — is recorded, never verified.
fn independently_accepted(row: &tokio_postgres::Row) -> bool {
    match row.get::<_, Option<bool>>(4) {
        Some(independent) => independent,
        None => matches!(row.get::<_, Option<String>>(5).as_deref(), Some(policy)
            if policy == crate::review::AGENT_REVIEW_POLICY
                || policy == crate::review::simulated_member::POLICY),
    }
}

async fn compute_facts(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request: &UpdateRecordRequest,
) -> PgResult<ComputedFacts> {
    let time_zone = tx
        .query_one(
            "SELECT COALESCE(settings_json->>'time_zone','UTC')
             FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &project],
        )
        .await?
        .get::<_, String>(0);
    let known_zone: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name=$1)",
            &[&time_zone],
        )
        .await?
        .get(0);
    if !known_zone {
        return Err(PgError::SourceDivergence);
    }
    let window = tx
        .query_one(
            "SELECT
                (extract(epoch FROM ($1::text::date::timestamp AT TIME ZONE $3))*1000)::bigint,
                (extract(epoch FROM (($2::text::date+1)::timestamp AT TIME ZONE $3))*1000)::bigint",
            &[&request.starts_on, &request.ends_on, &time_zone],
        )
        .await?;
    let start_ms = window.get::<_, i64>(0);
    let end_ms = window.get::<_, i64>(1);
    let rows = tx
        .query(
            "SELECT event_type,work_id,payload_json->>'decision',payload_json->>'kind',
                (payload_json->>'team_independent_acceptance')::boolean,payload_json->>'policy'
             FROM awr_team.events
             WHERE tenant_id=$1 AND project_id=$2 AND work_id IS NOT NULL
               AND created_at>=to_timestamp($3::bigint::double precision/1000)
               AND created_at<to_timestamp($4::bigint::double precision/1000)
               AND (event_type IN ('work.completed','wait.opened','wait.replied',
                                   'review.opened','review.decided')
                   OR (event_type='decision.request' AND payload_json->>'deduplicated'='false')
                   OR (event_type IN ('decision.respond','decision.cancel')
                       AND payload_json->>'applied'='true'))",
            &[&tenant, &project, &start_ms, &end_ms],
        )
        .await?;
    let mut events = Vec::new();
    for row in rows {
        let work = row.get::<_, String>(1);
        match row.get::<_, String>(0).as_str() {
            "work.completed" => {
                if independently_accepted(&row) {
                    events.push(FactEvent::Verified(work));
                } else {
                    events.push(FactEvent::CompletedWithoutIndependentReview(work));
                }
            }
            "wait.opened" => {
                let kind = legacy_wait_kind(row.get::<_, Option<String>>(3).as_deref())?;
                events.push(FactEvent::WaitOpened(work, kind));
            }
            "wait.replied" => {
                let kind = legacy_wait_kind(row.get::<_, Option<String>>(3).as_deref())?;
                events.push(FactEvent::WaitCleared(work.clone(), kind));
                if kind == WaitKind::Decision {
                    events.push(FactEvent::DecisionMade(work));
                }
            }
            "decision.request" => events.push(FactEvent::WaitOpened(work, WaitKind::Decision)),
            "decision.respond" => {
                events.push(FactEvent::WaitCleared(work.clone(), WaitKind::Decision));
                events.push(FactEvent::DecisionMade(work));
            }
            "decision.cancel" => {
                events.push(FactEvent::WaitCleared(work, WaitKind::Decision));
            }
            "review.opened" => events.push(FactEvent::WaitOpened(work, WaitKind::Review)),
            "review.decided" => {
                events.push(FactEvent::WaitCleared(work.clone(), WaitKind::Review));
                if row.get::<_, Option<String>>(2).as_deref() == Some("reject") {
                    events.push(FactEvent::ReviewReturned(work));
                }
            }
            _ => return Err(PgError::SourceDivergence),
        }
    }

    // Phase only: report sentences, blockers, artifacts and test bodies never enter this read.
    let reports = tx
        .query(
            "WITH ordered AS (
                SELECT s.work_id,c.session_id,c.id,c.created_at,c.progress_json->>'phase' AS phase,
                    lag(c.progress_json->>'phase') OVER (
                        PARTITION BY c.session_id ORDER BY c.created_at,c.id) AS previous
                FROM awr_team.checkpoints c
                JOIN awr_team.sessions s ON s.tenant_id=c.tenant_id
                  AND s.project_id=c.project_id AND s.id=c.session_id
                WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.progress_json IS NOT NULL
                  AND c.created_at<to_timestamp($4::bigint::double precision/1000)
             )
             SELECT work_id,phase,previous,EXISTS(
                 SELECT 1 FROM awr_team.events e
                 WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.work_id=ordered.work_id
                   AND e.event_type='work.completed' AND e.created_at>=ordered.created_at
             ) AS verified_after FROM ordered
             WHERE created_at>=to_timestamp($3::bigint::double precision/1000)
               AND ((phase='waiting_dependency' AND previous IS DISTINCT FROM 'waiting_dependency')
                   OR (previous='waiting_dependency' AND phase IS DISTINCT FROM 'waiting_dependency')
                   OR (phase='delivered' AND previous IS DISTINCT FROM 'delivered'))",
            &[&tenant, &project, &start_ms, &end_ms],
        )
        .await?;
    for row in reports {
        let work = row.get::<_, String>(0);
        let phase = row.get::<_, Option<String>>(1);
        let previous = row.get::<_, Option<String>>(2);
        if phase.as_deref() == Some("waiting_dependency")
            && previous.as_deref() != Some("waiting_dependency")
        {
            events.push(FactEvent::WaitOpened(work.clone(), WaitKind::Dependency));
        }
        if previous.as_deref() == Some("waiting_dependency")
            && phase.as_deref() != Some("waiting_dependency")
        {
            events.push(FactEvent::WaitCleared(work.clone(), WaitKind::Dependency));
        }
        if phase.as_deref() == Some("delivered")
            && previous.as_deref() != Some("delivered")
            && !row.get::<_, bool>(3)
        {
            events.push(FactEvent::DeliveredUnverified(work));
        }
    }
    Ok(ComputedFacts {
        json: build_facts(events),
        start_ms,
        end_ms,
        time_zone,
    })
}

impl UpdateRecordRequest {
    fn validate(&self) -> PgResult<()> {
        if self.protocol_version != 1
            || !identity(&self.request_id)
            || !valid_period(&self.period_kind, &self.period_key)
            || !valid_date(&self.starts_on)
            || !valid_date(&self.ends_on)
            || self.ends_on < self.starts_on
            || !matches!(self.status.as_str(), "ok" | "risk" | "off")
            || self.summary.trim().is_empty()
            || self.summary.chars().count() > 200
            || self.summary.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl WorkstreamCommandStore {
    /// Record one new current version of a project period. A project-scoped
    /// planning delegation is required for Agents; no task is used as an auth anchor.
    pub async fn record_update(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: &UpdateRecordRequest,
    ) -> PgResult<Value> {
        request.validate()?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate_writer(&tx, tenant, project, bearer).await?;
        if !auth
            .business_roles
            .as_ref()
            .is_some_and(|roles| roles.contains(&awr_team::BusinessRole::Supervisor))
        {
            return Err(PgError::Forbidden);
        }
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project,
            awr_team::Action::PlanningEditDraft,
        )
        .await?;
        if auth.project_status != "active" {
            return Err(PgError::ProjectNotAvailable);
        }
        let request_hash = awr_team::request_hash(&json!({
            "protocol":UPDATE_PROTOCOL,"tenant":tenant,"project":project,
            "actor":auth.actor_id,"client":auth.client_id,"op":"update.record","request":request
        }))
        .map_err(|_| invalid())?;
        if let Some(row) = operation(&tx, tenant, project, &auth, &request.request_id).await? {
            if row.get::<_, String>(0) != request_hash || row.get::<_, String>(2) != "committed" {
                return Err(PgError::IdempotencyConflict);
            }
            let receipt: Value = row.get(1);
            if receipt["protocol"] != UPDATE_PROTOCOL || receipt["op"] != "update.record" {
                return Err(PgError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":receipt,"execution_authorized":false}));
        }

        let actor = task_intake::actor_instance(&tx, tenant, project, &auth).await?;
        let person_id = actor.person_id().as_str();
        let author = tx
            .query_opt(
                "SELECT COALESCE(p.display_name,a.display_name) FROM (SELECT $3::text AS id) selected
                 LEFT JOIN awr_team.persons p ON p.tenant_id=$1 AND p.project_id=$2 AND p.id=selected.id
                 LEFT JOIN awr_team.actors a ON a.tenant_id=$1 AND a.id=selected.id AND EXISTS(
                    SELECT 1 FROM awr_team.project_memberships m
                    WHERE m.tenant_id=$1 AND m.project_id=$2 AND m.actor_id=a.id)",
                &[&tenant, &project, &person_id],
            )
            .await?
            .and_then(|row| row.get::<_, Option<String>>(0))
            .ok_or(PgError::Forbidden)?;
        let previous = tx
            .query_opt(
                "SELECT update_id FROM awr_team.project_period_update_current
                 WHERE tenant_id=$1 AND project_id=$2 AND period_kind=$3 AND period_key=$4
                 FOR UPDATE",
                &[&tenant, &project, &request.period_kind, &request.period_key],
            )
            .await?;
        let previous_id = previous.as_ref().map(|row| row.get::<_, String>(0));
        let previous_version = if let Some(id) = previous_id.as_deref() {
            tx.query_one(
                "SELECT version FROM awr_team.project_period_updates
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &id],
            )
            .await?
            .get::<_, i64>(0)
        } else {
            0
        };
        let version = previous_version
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        let update_id = crate::tx::new_id();
        let fact_snapshot_ref = crate::tx::new_id();
        let facts = compute_facts(&tx, tenant, project, request).await?;
        let inserted = tx
            .query_one(
                "INSERT INTO awr_team.project_period_updates(
            tenant_id,project_id,id,period_kind,period_key,starts_on,ends_on,status,summary,
            author_person_id,author_display_name,recorded_by_actor_id,recorded_by_client_id,
            request_id,version,supersedes_update_id,fact_snapshot_ref,fact_snapshot_json,
            fact_window_start_unix_ms,fact_window_end_unix_ms,fact_time_zone)
            VALUES($1,$2,$3,$4,$5,$6::text::date,$7::text::date,$8,$9,$10,$11,$12,$13,$14,$15,$16,
                   $17,$18,$19,$20,$21)
            RETURNING (extract(epoch FROM recorded_at)*1000)::bigint",
                &[
                    &tenant,
                    &project,
                    &update_id,
                    &request.period_kind,
                    &request.period_key,
                    &request.starts_on,
                    &request.ends_on,
                    &request.status,
                    &request.summary,
                    &person_id,
                    &author,
                    &auth.actor_id,
                    &auth.client_id,
                    &request.request_id,
                    &version,
                    &previous_id,
                    &fact_snapshot_ref,
                    &facts.json,
                    &facts.start_ms,
                    &facts.end_ms,
                    &facts.time_zone,
                ],
            )
            .await?;
        let recorded_at: i64 = inserted.get(0);
        tx.execute(
            "INSERT INTO awr_team.project_period_update_current(
                tenant_id,project_id,period_kind,period_key,update_id)
             VALUES($1,$2,$3,$4,$5)
             ON CONFLICT(tenant_id,project_id,period_kind,period_key) DO UPDATE
             SET update_id=EXCLUDED.update_id",
            &[
                &tenant,
                &project,
                &request.period_kind,
                &request.period_key,
                &update_id,
            ],
        )
        .await?;
        let next = auth
            .revision
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        tx.execute(
            "UPDATE awr_team.projects SET project_revision=$3 WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &project, &next],
        )
        .await?;
        let receipt = json!({
            "protocol":UPDATE_PROTOCOL,"request_id":request.request_id,"op":"update.record",
            "request_hash":request_hash,"project_id":project,"update_id":update_id,
            "period_kind":request.period_kind,"period_key":request.period_key,
            "starts_on":request.starts_on,"ends_on":request.ends_on,"status":request.status,
            "summary":request.summary,"author":{"person_id":person_id,"display_name":author},
            "recorded_at_unix_ms":recorded_at,"version":version.to_string(),
            "supersedes_update_id":previous_id,"fact_snapshot_ref":fact_snapshot_ref,
            "committed_project_revision":next.to_string(),"state_basis":"at_commit",
            "sentence_provenance":"caller_supplied","execution_authorized":false
        });
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,
                event_type,actor_id,work_id,payload_json,workstream_id)
             VALUES($1,$2,$3,$4,0,'update.recorded',$5,NULL,$6,NULL)",
            &[
                &tenant,
                &project,
                &crate::tx::new_id(),
                &next,
                &auth.actor_id,
                &json!({"update_id":update_id,"period_kind":request.period_kind,
                "period_key":request.period_key,"version":version.to_string(),
                "status":request.status,"author_person_id":person_id}),
            ],
        )
        .await?;
        tx.execute(
            "INSERT INTO awr_team.operations(tenant_id,project_id,id,actor_id,client_id,
                request_id,op,request_hash,state,committed_project_revision,result_json)
             VALUES($1,$2,$3,$4,$5,$6,'update.record',$7,'committed',$8,$9)",
            &[
                &tenant,
                &project,
                &crate::tx::new_id(),
                &auth.actor_id,
                &auth.client_id,
                &request.request_id,
                &request_hash,
                &next,
                &receipt,
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(json!({"replayed":false,"receipt":receipt,"execution_authorized":false}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> UpdateRecordRequest {
        serde_json::from_value(json!({"protocol_version":1,"request_id":"request",
            "period_kind":"week","period_key":"2026-W47","starts_on":"2026-11-16",
            "ends_on":"2026-11-22","status":"risk","summary":"A caller-written sentence."}))
        .unwrap()
    }

    #[test]
    fn fact_snapshot_matches_every_reference_table_row() {
        let facts = build_facts([
            FactEvent::Verified("b".into()),
            FactEvent::Verified("a".into()),
            FactEvent::Verified("a".into()),
            FactEvent::CompletedWithoutIndependentReview("e".into()),
            FactEvent::CompletedWithoutIndependentReview("e".into()),
            FactEvent::WaitOpened("a".into(), WaitKind::Decision),
            FactEvent::WaitOpened("a".into(), WaitKind::Decision),
            FactEvent::WaitOpened("c".into(), WaitKind::Dependency),
            FactEvent::WaitOpened("b".into(), WaitKind::Review),
            FactEvent::WaitOpened("d".into(), WaitKind::External),
            FactEvent::WaitOpened("u".into(), WaitKind::Unknown),
            FactEvent::WaitCleared("a".into(), WaitKind::Decision),
            FactEvent::WaitCleared("c".into(), WaitKind::Dependency),
            FactEvent::WaitCleared("b".into(), WaitKind::Review),
            FactEvent::WaitCleared("d".into(), WaitKind::External),
            FactEvent::WaitCleared("u".into(), WaitKind::Unknown),
            FactEvent::ReviewReturned("b".into()),
            FactEvent::ReviewReturned("a".into()),
            FactEvent::ReviewReturned("b".into()),
            FactEvent::DeliveredUnverified("c".into()),
            FactEvent::DeliveredUnverified("c".into()),
            FactEvent::DecisionMade("b".into()),
            FactEvent::DecisionMade("a".into()),
            FactEvent::DecisionMade("a".into()),
        ]);
        assert_eq!(
            facts,
            json!({
                "verified":{"count":2,"work_ids":["a","b"]},
                "completed_without_independent_review":{"count":1,"work_ids":["e"]},
                "waits_opened":{"count":6,"kinds":[
                    {"kind":"decision","count":2,"work_ids":["a","a"]},
                    {"kind":"dependency","count":1,"work_ids":["c"]},
                    {"kind":"review","count":1,"work_ids":["b"]},
                    {"kind":"external","count":1,"work_ids":["d"]},
                    {"kind":"unknown","count":1,"work_ids":["u"]}
                ]},
                "waits_cleared":{"count":5,"kinds":[
                    {"kind":"decision","count":1,"work_ids":["a"]},
                    {"kind":"dependency","count":1,"work_ids":["c"]},
                    {"kind":"review","count":1,"work_ids":["b"]},
                    {"kind":"external","count":1,"work_ids":["d"]},
                    {"kind":"unknown","count":1,"work_ids":["u"]}
                ]},
                "review_returned":{"count":2,"work_ids":["a","b"]},
                "delivered_unverified":{"count":1,"work_ids":["c"]},
                "decisions_made":{"count":3,"work_ids":["a","a","b"]}
            })
        );
        for field in [
            "verified",
            "completed_without_independent_review",
            "review_returned",
            "delivered_unverified",
            "decisions_made",
        ] {
            assert_eq!(
                facts[field]["count"],
                facts[field]["work_ids"].as_array().unwrap().len()
            );
        }
        for field in ["waits_opened", "waits_cleared"] {
            let kinds = facts[field]["kinds"].as_array().unwrap();
            assert_eq!(
                facts[field]["count"].as_u64().unwrap(),
                kinds
                    .iter()
                    .map(|kind| kind["count"].as_u64().unwrap())
                    .sum::<u64>()
            );
            for kind in kinds {
                assert_eq!(kind["count"], kind["work_ids"].as_array().unwrap().len());
            }
        }
        assert_eq!(facts.as_object().unwrap().len(), 7);
        assert!(!facts.to_string().contains("summary"));
        assert!(!facts.to_string().contains("person"));
    }

    fn backticked(text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('`') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('`') else { break };
            tokens.push(after[..end].to_owned());
            rest = &after[end + 1..];
        }
        tokens
    }

    fn cell(line: &str, index: usize) -> String {
        line.split('|')
            .nth(index)
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    #[test]
    fn counting_table_is_machine_checked_row_by_row_against_the_implementation() {
        let doc = include_str!("../../../../docs/dev/reference/project-update-facts.md");
        let fields: Vec<String> = doc
            .split('\n')
            .filter_map(|line| line.strip_prefix("| `"))
            .map(|rest| rest.split('`').next().unwrap_or_default().to_owned())
            .collect();
        let snapshot = build_facts([]);
        let keys: std::collections::BTreeSet<&str> = snapshot
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            keys.iter()
                .map(|key| key.to_string())
                .collect::<std::collections::BTreeSet<_>>(),
            "table rows and snapshot fields must match one to one"
        );
        assert_eq!(fields.len(), keys.len());

        let kinds: Vec<&str> = [
            WaitKind::Decision,
            WaitKind::Dependency,
            WaitKind::Review,
            WaitKind::External,
            WaitKind::Unknown,
        ]
        .map(WaitKind::as_str)
        .to_vec();
        let waits_opened = doc
            .split('\n')
            .find(|line| line.starts_with("| `waits_opened`"))
            .expect("documented waits_opened row");
        let waits_cell = backticked(&cell(waits_opened, 2));
        let documented: std::collections::BTreeSet<&str> = waits_cell
            .iter()
            .map(String::as_str)
            .filter(|token| kinds.contains(token))
            .collect();
        assert_eq!(
            documented,
            kinds
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
        );
        assert!(
            waits_cell
                .iter()
                .all(|token| kinds.contains(&token.as_str()) || token == "work_ids")
        );

        for (word, expected_fields) in [
            ("核验过", vec!["verified"]),
            ("等待", vec!["waits_opened", "waits_cleared"]),
            ("压着", vec!["none"]),
        ] {
            let rows: Vec<&str> = doc
                .split('\n')
                .filter(|line| line.starts_with(&format!("| {word}")))
                .collect();
            assert_eq!(rows.len(), 1, "one shared-word row for {word}");
            let fields = backticked(&cell(rows[0], 2));
            assert_eq!(fields, expected_fields, "service fields for {word}");
            assert!(
                fields
                    .iter()
                    .all(|field| field == "none" || keys.contains(field.as_str())),
                "shared-word rows must name real snapshot fields"
            );
        }

        let shared = include_str!("../../../../docs/dev/reference/overview-fact-semantics.md");
        for word in ["核验过", "等待", "压着"] {
            assert!(
                doc.contains(word) && shared.contains(word),
                "shared word {word}"
            );
        }
        assert!(
            shared.contains("标了完成"),
            "标了完成 stays an Observer-side word"
        );
        assert!(
            doc.contains("标了完成") && doc.contains("delivered_unverified"),
            "the service doc names the Observer word it does not map and its own field"
        );
        for kind in [
            "decisionWait",
            "dependencyWait",
            "reviewWait",
            "externalWait",
        ] {
            assert!(shared.contains(kind), "Observer wait kind {kind}");
        }
        assert!(doc.contains("overview-fact-semantics.md"));
    }

    #[test]
    fn fact_snapshot_projection_recounts_only_visible_work() {
        let stored = build_facts([
            FactEvent::Verified("a".into()),
            FactEvent::Verified("b".into()),
            FactEvent::WaitOpened("a".into(), WaitKind::Decision),
            FactEvent::WaitOpened("a".into(), WaitKind::Decision),
            FactEvent::WaitOpened("c".into(), WaitKind::Dependency),
            FactEvent::WaitOpened("b".into(), WaitKind::Review),
            FactEvent::WaitOpened("d".into(), WaitKind::External),
            FactEvent::WaitCleared("a".into(), WaitKind::Decision),
            FactEvent::WaitCleared("c".into(), WaitKind::Dependency),
            FactEvent::ReviewReturned("a".into()),
            FactEvent::ReviewReturned("b".into()),
            FactEvent::CompletedWithoutIndependentReview("d".into()),
            FactEvent::DeliveredUnverified("c".into()),
            FactEvent::DecisionMade("a".into()),
            FactEvent::DecisionMade("a".into()),
            FactEvent::DecisionMade("b".into()),
        ]);
        let visible = std::collections::BTreeSet::from(["a".to_owned(), "c".to_owned()]);
        let (projected, hidden) = project_fact_snapshot(&stored, &visible).unwrap();
        assert_eq!(hidden, 2);
        assert_eq!(projected["verified"], json!({"count":1,"work_ids":["a"]}));
        assert_eq!(projected["waits_opened"]["count"], 3);
        assert_eq!(
            projected["waits_opened"]["kinds"].as_array().unwrap().len(),
            2
        );
        assert_eq!(projected["waits_cleared"]["count"], 2);
        assert_eq!(projected["review_returned"]["work_ids"], json!(["a"]));
        assert_eq!(
            projected["completed_without_independent_review"]["work_ids"],
            json!([])
        );
        assert_eq!(projected["delivered_unverified"]["work_ids"], json!(["c"]));
        assert_eq!(
            projected["decisions_made"],
            json!({"count":2,"work_ids":["a","a"]})
        );
        let text = projected.to_string();
        assert!(!text.contains("\"b\"") && !text.contains("\"d\""));
    }

    #[test]
    fn input_validation_is_unicode_aware_and_does_not_rewrite_the_sentence() {
        let mut value = request();
        value.summary = format!("  {}  ", "雪".repeat(196));
        value.validate().unwrap();
        assert_eq!(value.summary.chars().count(), 200);
        assert!(value.summary.starts_with("  ") && value.summary.ends_with("  "));
        value.summary = "雪".repeat(201);
        assert!(value.validate().err().unwrap().is_invalid_command_fields());
        for summary in ["", " \t\n", "line\nbreak", "nul\0byte", "\u{7f}"] {
            value.summary = summary.into();
            assert!(value.validate().is_err(), "accepted invalid summary");
        }
    }

    #[test]
    fn input_validation_rejects_bad_periods_dates_statuses_and_unknown_fields() {
        let mut value = request();
        for (kind, key) in [
            ("week", "2026-W00"),
            ("week", "2026-W54"),
            ("month", "2026-13"),
            ("year", "2026"),
        ] {
            value.period_kind = kind.into();
            value.period_key = key.into();
            assert!(value.validate().is_err());
        }
        value = request();
        for date in ["2026-02-29", "2026-13-01", "2026-01-32", "not-a-date"] {
            value.starts_on = date.into();
            assert!(value.validate().is_err());
        }
        value = request();
        value.ends_on = "2026-11-15".into();
        assert!(value.validate().is_err());
        let invalid_status = serde_json::from_value::<UpdateRecordRequest>(json!({
            "protocol_version":1,"request_id":"request","period_kind":"week",
            "period_key":"2026-W47","starts_on":"2026-11-16","ends_on":"2026-11-22",
            "status":"generated","summary":"text"}))
        .unwrap();
        assert!(invalid_status.validate().is_err());
        assert!(
            serde_json::from_value::<UpdateRecordRequest>(json!({
            "protocol_version":1,"request_id":"request","period_kind":"week",
            "period_key":"2026-W47","starts_on":"2026-11-16","ends_on":"2026-11-22",
            "status":"ok","summary":"text","fact_snapshot_ref":"caller-forged"}))
            .is_err()
        );
        let leap = serde_json::from_value::<UpdateRecordRequest>(json!({
            "protocol_version":1,"request_id":"request","period_kind":"month",
            "period_key":"2028-02","starts_on":"2028-02-01","ends_on":"2028-02-29",
            "status":"ok","summary":"text"}))
        .unwrap();
        leap.validate().unwrap();
    }
}
