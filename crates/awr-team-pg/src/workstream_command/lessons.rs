//! Agent-recorded lessons. Identity and provenance are authenticated facts;
//! the words and applicability remain the person's Agent's declarations.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Content {
    title: String,
    original_words: String,
    when_to_use: String,
    not_applicable: String,
    observed_result: String,
    scope: Scope,
    dependencies: Dependencies,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    path_prefixes: Vec<String>,
    task_tags: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dependencies {
    documents: Vec<Document>,
    decision_ids: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Propose {
    session_id: String,
    expected_session_version: String,
    content: Content,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Change {
    session_id: String,
    expected_session_version: String,
    lesson_id: String,
    expected_lesson_version: String,
    content: Option<Content>,
    reason: Option<String>,
}

pub(super) enum Action {
    Propose(Propose),
    Change(Change),
}

fn text(s: &str, limit: usize) -> bool {
    !s.trim().is_empty() && s.len() <= limit && !s.contains('\0')
}

fn path(s: &str) -> bool {
    text(s, 1024)
        && !s.starts_with('/')
        && !s.contains(['\\', ':'])
        && !s.chars().any(char::is_control)
        && s.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

impl Content {
    fn validate(&self) -> PgResult<()> {
        if !text(&self.title, 200)
            || !text(&self.original_words, 8192)
            || !text(&self.when_to_use, 2048)
            || !text(&self.not_applicable, 2048)
            || !text(&self.observed_result, 2048)
            || self.scope.path_prefixes.len() + self.scope.task_tags.len() == 0
            || self.scope.path_prefixes.len() > 20
            || self.scope.task_tags.len() > 20
            || self.scope.path_prefixes.iter().any(|p| !path(p))
            || self.scope.task_tags.iter().any(|tag| !identity(tag))
            || self.dependencies.documents.len() > 20
            || self.dependencies.decision_ids.len() > 20
            || self
                .dependencies
                .documents
                .iter()
                .any(|d| !path(&d.path) || !digest(&d.sha256))
            || self
                .dependencies
                .decision_ids
                .iter()
                .any(|id| !identity(id))
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        if op == "lesson.propose" {
            let a: Propose = serde_json::from_value(args).map_err(|_| invalid())?;
            if !identity(&a.session_id) || version(&a.expected_session_version)? == 0 {
                return Err(invalid());
            }
            a.content.validate()?;
            return Ok(Self::Propose(a));
        }
        let a: Change = serde_json::from_value(args.clone()).map_err(|_| invalid())?;
        if !identity(&a.session_id)
            || version(&a.expected_session_version)? == 0
            || !identity(&a.lesson_id)
            || version(&a.expected_lesson_version)? == 0
            || (args.get("content").is_some() && op != "lesson.confirm")
            || (args.get("reason").is_some() && op != "lesson.mark_not_applicable")
        {
            return Err(invalid());
        }
        if let Some(c) = &a.content {
            c.validate()?;
        }
        match op {
            "lesson.confirm" | "lesson.revoke" | "lesson.promote" => (),
            "lesson.mark_not_applicable" if a.reason.as_deref().is_some_and(|s| text(s, 2048)) => {
                ()
            }
            _ => return Err(invalid()),
        }
        Ok(Self::Change(a))
    }
}

/// Requires a current unambiguous person binding through the selected delegation.
pub(crate) async fn person(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
) -> PgResult<String> {
    if auth.actor_kind != "agent" {
        return Err(PgError::Forbidden);
    }
    Ok(task_intake::actor_instance(tx, tenant, project, auth)
        .await?
        .person_id()
        .to_string())
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    action: Action,
) -> PgResult<Applied> {
    let (sid, sv) = match &action {
        Action::Propose(a) => (&a.session_id, &a.expected_session_version),
        Action::Change(a) => (&a.session_id, &a.expected_session_version),
    };
    session(tx, tenant, project, auth, command, sid, sv, ownership).await?;
    let person = person(tx, tenant, project, auth).await?;
    let (id, v, stage, status) = match action {
        Action::Propose(a) => {
            let id = crate::tx::new_id();
            let content = serde_json::to_value(a.content).map_err(|_| invalid())?;
            tx.execute(
                "INSERT INTO awr_team.lessons(tenant_id,project_id,id,work_id,session_id,
                author_person_id,recorded_by_actor_id,recorded_by_client_id,content_json)
                VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
                &[
                    &tenant,
                    &project,
                    &id,
                    &command.work_id,
                    &a.session_id,
                    &person,
                    &auth.actor_id,
                    &auth.client_id,
                    &content,
                ],
            )
            .await?;
            (id, 1, "candidate".to_owned(), "active".to_owned())
        }
        Action::Change(a) => {
            let r = tx
                .query_opt(
                    "SELECT author_person_id,stage,status,version FROM awr_team.lessons
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND id=$4 FOR UPDATE",
                    &[&tenant, &project, &command.work_id, &a.lesson_id],
                )
                .await?
                .ok_or(PgError::Forbidden)?;
            let author: String = r.get(0);
            let mut stage: String = r.get(1);
            let mut status: String = r.get(2);
            let v: i64 = r.get(3);
            if matches!(command.op.as_str(), "lesson.confirm" | "lesson.revoke") && author != person
            {
                return Err(PgError::Forbidden);
            }
            if command.op == "lesson.promote"
                && !auth
                    .business_roles
                    .as_ref()
                    .is_some_and(|roles| roles.contains(&awr_team::BusinessRole::Supervisor))
            {
                return Err(PgError::Forbidden);
            }
            // A revoked record stays withdrawn, including under a fresh confirmation request.
            if status != "active" || v != version(&a.expected_lesson_version)? || v == i64::MAX {
                return Err(PgError::PreconditionsChanged);
            }
            match command.op.as_str() {
                "lesson.confirm" if stage == "candidate" => stage = "project".into(),
                "lesson.revoke" => status = "revoked".into(),
                "lesson.promote" if stage == "project" => stage = "team_rule".into(),
                "lesson.mark_not_applicable" => {
                    tx.execute("INSERT INTO awr_team.lesson_feedback(tenant_id,project_id,id,lesson_id,person_id,session_id,reason)
                        VALUES($1,$2,$3,$4,$5,$6,$7)",
                        &[&tenant,&project,&crate::tx::new_id(),&a.lesson_id,&person,&a.session_id,&a.reason.unwrap()]).await?;
                }
                _ => return Err(PgError::PreconditionsChanged),
            }
            let content = a
                .content
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| invalid())?;
            tx.execute(
                "UPDATE awr_team.lessons SET stage=$5,status=$6,version=version+1,
                content_json=COALESCE($7,content_json),updated_at=clock_timestamp()
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND id=$4",
                &[
                    &tenant,
                    &project,
                    &command.work_id,
                    &a.lesson_id,
                    &stage,
                    &status,
                    &content,
                ],
            )
            .await?;
            (a.lesson_id, v + 1, stage, status)
        }
    };
    // Events and receipts carry bounded metadata only, never lesson text or feedback.
    Ok(Applied {
        data: json!({"lesson_id":id,"lesson_version":v.to_string(),"stage":stage,"status":status,
        "original_words_provenance":"recorded_by_authors_agent","execution_authorized":false}),
        preceding_events: vec![],
    })
}

pub(crate) async fn reader_person(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
) -> PgResult<String> {
    Ok(task_intake::actor_instance(tx, tenant, project, auth)
        .await?
        .person_id()
        .to_string())
}

/// Work-scoped reading shares command scope checks. A candidate benefits its author first;
/// withdrawal leaves only identity/state metadata on reads, clearing narrative and dependencies.
pub(crate) async fn records(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    after: &str,
    limit: i64,
) -> PgResult<Vec<Value>> {
    let (bound, _) = work_binding(tx, tenant, project, auth, work).await?;
    crate::workstream_auth::authorize_query(
        auth,
        Some(bound.workstream_id),
        Some(work),
        "lessons.list",
    )?;
    // Selector-free navigation has covering read grants but no selected binding.
    // Resolve one for this task; explicit reads retain their already-selected grant.
    let scoped_reader = if auth.actor_kind == "agent" && auth.delegation_id.is_none() {
        Some(crate::delegation_auth::navigation_authority(
            auth,
            awr_team::Action::WorkRead,
            bound.workstream_id,
            work,
        )?)
    } else {
        None
    };
    let reader_person =
        reader_person(tx, tenant, project, scoped_reader.as_ref().unwrap_or(auth)).await?;
    let rows = tx.query("SELECT id,author_person_id,session_id,stage,status,version,content_json,
        (extract(epoch FROM created_at)*1000)::bigint,(extract(epoch FROM updated_at)*1000)::bigint,
        (SELECT count(*) FROM awr_team.lesson_feedback f WHERE f.tenant_id=l.tenant_id AND f.project_id=l.project_id AND f.lesson_id=l.id)
        FROM awr_team.lessons l WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND id>$4
          AND (stage<>'candidate' OR author_person_id=$5) ORDER BY id LIMIT $6",
        &[&tenant,&project,&work,&after,&reader_person,&limit]).await?;
    Ok(rows.iter().map(|r| {
        let active = r.get::<_,String>(4) == "active";
        json!({"lesson_id":r.get::<_,String>(0),"work_id":work,"author_person_id":r.get::<_,String>(1),
            "source_session_id":r.get::<_,String>(2),"stage":r.get::<_,String>(3),"status":r.get::<_,String>(4),
            "lesson_version":r.get::<_,i64>(5).to_string(),"content":if active {r.get::<_,Value>(6)}else{Value::Null},
            "created_at_unix_ms":r.get::<_,i64>(7),"updated_at_unix_ms":r.get::<_,i64>(8),
            "not_applicable_count":r.get::<_,i64>(9),"use_for_personnel_evaluation":false,
            "original_words_provenance":"recorded_by_authors_agent"})
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> Value {
        json!({"title":"Bounded retries","original_words":"  Keep the first result.\n",
            "when_to_use":"Retrying a write","not_applicable":"A fresh request","observed_result":"One effect",
            "scope":{"path_prefixes":["src/service"],"task_tags":["writes"]},
            "dependencies":{"documents":[{"path":"docs/contract.md","sha256":"a".repeat(64)}],"decision_ids":["decision-1"]}})
    }

    #[test]
    fn strict_format_preserves_words_and_rejects_forged_provenance() {
        let a = json!({"session_id":"s","expected_session_version":"1","content":content()});
        assert!(Action::parse("lesson.propose", a.clone()).is_ok());
        let parsed: Content = serde_json::from_value(content()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), content());
        for field in [
            "author_person_id",
            "person_id",
            "source_work_id",
            "source_session_id",
            "work_id",
        ] {
            let mut forged = a.clone();
            forged[field] = json!("somebody-else");
            assert!(Action::parse("lesson.propose", forged).is_err(), "{field}");
        }
        for (field, value) in [
            ("title", " ".into()),
            ("title", "x".repeat(201)),
            ("original_words", "x".repeat(8193)),
            ("when_to_use", "x".repeat(2049)),
            ("not_applicable", "\0".into()),
            ("observed_result", "".into()),
        ] {
            let mut bad = a.clone();
            bad["content"][field] = json!(value);
            assert!(Action::parse("lesson.propose", bad).is_err(), "{field}");
        }
        for p in [
            "/private",
            "../src",
            "src/../private",
            "src//api",
            "C:secret",
            "src\\api",
        ] {
            let mut bad = a.clone();
            bad["content"]["scope"]["path_prefixes"] = json!([p]);
            assert!(Action::parse("lesson.propose", bad).is_err(), "{p}");
        }
        let mut bad = a.clone();
        bad["content"]["dependencies"]["documents"][0]["sha256"] = json!("bad");
        assert!(Action::parse("lesson.propose", bad).is_err());
        let mut bad = a;
        bad["content"]["scope"] = json!({"path_prefixes":[],"task_tags":[]});
        assert!(Action::parse("lesson.propose", bad).is_err());
    }
    #[test]
    fn change_inputs_require_versions_and_bounded_applicability_feedback() {
        let base = json!({"session_id":"s","expected_session_version":"1",
            "lesson_id":"l","expected_lesson_version":"1"});
        for op in ["lesson.confirm", "lesson.revoke", "lesson.promote"] {
            assert!(Action::parse(op, base.clone()).is_ok(), "{op}");
            let mut extra = base.clone();
            extra["reason"] = json!("unexpected");
            assert!(Action::parse(op, extra).is_err(), "{op}");
            for key in ["expected_session_version", "expected_lesson_version"] {
                for value in ["0", "01", "-1", "9223372036854775808"] {
                    let mut invalid = base.clone();
                    invalid[key] = json!(value);
                    assert!(Action::parse(op, invalid).is_err(), "{op}: {key}={value}");
                }
            }
        }
        for reason in [
            None,
            Some("".to_owned()),
            Some(" \n".to_owned()),
            Some("x".repeat(2049)),
            Some("\0".to_owned()),
        ] {
            let mut invalid = base.clone();
            if let Some(reason) = reason {
                invalid["reason"] = json!(reason);
            }
            assert!(Action::parse("lesson.mark_not_applicable", invalid).is_err());
        }
        let mut feedback = base.clone();
        feedback["reason"] = json!("A different request shape.");
        assert!(Action::parse("lesson.mark_not_applicable", feedback).is_ok());
        for op in [
            "lesson.revoke",
            "lesson.promote",
            "lesson.mark_not_applicable",
        ] {
            let mut invalid = base.clone();
            invalid["reason"] = json!("A different request shape.");
            invalid["content"] = content();
            assert!(Action::parse(op, invalid).is_err(), "{op}");
        }
        let mut forged = content();
        forged["author_person_id"] = json!("somebody-else");
        assert!(
            Action::parse(
                "lesson.propose",
                json!({"session_id":"s",
            "expected_session_version":"1","content":forged})
            )
            .is_err()
        );
    }
}
