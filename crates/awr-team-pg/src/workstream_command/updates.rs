//! Project-scoped periodic updates. The caller writes the judgment; the service
//! only validates, attributes and versions it. Fact snapshots are added by PPD-011.
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
        let inserted = tx
            .query_one(
                "INSERT INTO awr_team.project_period_updates(
            tenant_id,project_id,id,period_kind,period_key,starts_on,ends_on,status,summary,
            author_person_id,author_display_name,recorded_by_actor_id,recorded_by_client_id,
            request_id,version,supersedes_update_id,fact_snapshot_ref)
            VALUES($1,$2,$3,$4,$5,$6::text::date,$7::text::date,$8,$9,$10,$11,$12,$13,$14,$15,$16,NULL)
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
            "supersedes_update_id":previous_id,"fact_snapshot_ref":null,
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
