use crate::{
    Store, db_error,
    session::{require_active, require_branch, session_at},
    work::readiness,
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn validate_action(
    conn: &Connection,
    project: Id,
    revision: Revision,
    patch: &MutationPatch,
    target: &serde_json::Value,
    session: Option<Id>,
) -> Result<()> {
    if (patch.work_action.is_some()
        || patch
            .host_edit
            .as_ref()
            .is_some_and(|h| h.action != HostEditAction::Fields))
        && target.get("archived").and_then(serde_json::Value::as_bool) == Some(true)
    {
        return Err(Error::RuleViolation(
            "restore archived work before performing lifecycle actions".into(),
        ));
    }
    if let Some(host) = &patch.host_edit {
        host.validate()?;
        if session.is_some() {
            return Err(Error::InvalidInput(
                "host edits record provenance without creating an Agent session".into(),
            ));
        }
        if host.action == HostEditAction::ActivateDraft {
            let work: WorkItem = serde_json::from_value(target.clone())?;
            validate_draft_activation(conn, project, revision, &work)?;
        }
        if host.action == HostEditAction::ConfirmOrdinary {
            let work: WorkItem = serde_json::from_value(target.clone())?;
            let receipt: OrdinaryCompletion =
                serde_json::from_value(patch.changes["ordinary_completion"].clone())?;
            receipt.validate(&work.meta.external_key, &work.acceptance)?;
            if !matches!(
                work.status,
                WorkStatus::Planned | WorkStatus::Ready | WorkStatus::InProgress
            ) {
                return Err(Error::InvalidTransition("ordinary confirmation requires active work; draft, cancelled and completed work need their explicit lifecycle action".into()));
            }
            let claimed: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM claims WHERE project_id=?1 AND work_item_id=?2 AND status='active' AND released_at IS NULL AND (expires_at IS NULL OR expires_at>?3))", params![project.to_string(),work.meta.id.to_string(),now_millis()?], |r|r.get(0)).map_err(db_error)?;
            if claimed {
                return Err(Error::ClaimConflict(
                    "finish or release the active execution before ordinary confirmation".into(),
                ));
            }
            check_blocker(&work)?;
            check_dependencies(conn, project, revision, &work, None, false)?;
            // Reuse declared goal/acceptance readiness without inventing a draft transition.
            let mut structural = work.clone();
            structural.status = WorkStatus::Draft;
            validate_draft_activation(conn, project, revision, &structural)?;
        }
    }
    let Some(binding) = &patch.work_action else {
        return Ok(());
    };
    patch.validate()?;
    let work: WorkItem = serde_json::from_value(target.clone())?;
    if work.status != binding.from {
        return Err(Error::SourceConflict(
            "work state differs from the action's bound starting state".into(),
        ));
    }
    let session = session_at(
        conn,
        project,
        session.ok_or_else(|| {
            Error::InvalidInput("work actions require an explicit work-bound session".into())
        })?,
    )?;
    require_active(&session)?;
    require_branch(conn, project, session.branch_id)?;
    if session.work_item_id != Some(work.meta.id) {
        return Err(Error::InvalidInput(
            "work action session must be bound to the exact target work".into(),
        ));
    }
    let at = now_millis()?;
    // Source state is shared within a project, so a live claim on another branch also
    // blocks these source mutations. Source owner remains business metadata.
    let foreign:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM claims WHERE project_id=?1 AND work_item_id=?2 AND status='active' AND released_at IS NULL AND (expires_at IS NULL OR expires_at>?3) AND session_id!=?4)",params![project.to_string(),work.meta.id.to_string(),at,session.id.to_string()],|r|r.get(0)).map_err(db_error)?;
    if foreign {
        return Err(Error::ClaimConflict(
            "another session holds this work; release or hand off that runtime claim first".into(),
        ));
    }
    if binding.action.needs_claim() {
        let owned:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM claims WHERE project_id=?1 AND work_item_id=?2 AND status='active' AND released_at IS NULL AND (expires_at IS NULL OR expires_at>?3) AND session_id=?4 AND branch_id IS ?5 AND agent_id=?6)",params![project.to_string(),work.meta.id.to_string(),at,session.id.to_string(),session.branch_id.map(|id|id.to_string()),session.agent_id],|r|r.get(0)).map_err(db_error)?;
        if !owned {
            return Err(Error::ClaimConflict(
                "this work action requires an unexpired runtime claim held by its creating session"
                    .into(),
            ));
        }
    }
    if binding.action.needs_dependencies() {
        check_dependencies(
            conn,
            project,
            revision,
            &work,
            session.branch_id,
            matches!(binding.action, WorkAction::Unblock | WorkAction::Complete),
        )?;
    }
    if let Some(completion) = &binding.completion {
        validate_completion_binding(conn, project, &work, session.branch_id, completion)?;
        check_blocker(&work)?;
    }
    Ok(())
}

fn validate_draft_activation(
    conn: &Connection,
    project: Id,
    revision: Revision,
    work: &WorkItem,
) -> Result<()> {
    if work.status != WorkStatus::Draft {
        return Err(Error::InvalidTransition(
            "only a draft can be explicitly activated".into(),
        ));
    }
    validate_criteria(&work.acceptance)?;
    if work.title.trim().is_empty()
        || work.next_action.trim().is_empty()
        || work.kind.as_deref() == Some("intake")
    {
        return Err(Error::RuleViolation(
            "draft needs a title, delivery acceptance and a concrete next action".into(),
        ));
    }
    let claimed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM claims WHERE project_id=?1 AND work_item_id=?2 AND status='active' AND released_at IS NULL AND (expires_at IS NULL OR expires_at>?3))",params![project.to_string(),work.meta.id.to_string(),now_millis()?],|r|r.get(0)).map_err(db_error)?;
    if claimed {
        return Err(Error::ClaimConflict(
            "release existing runtime occupancy before activating a draft".into(),
        ));
    }
    check_dependencies(conn, project, revision, work, None, false)?;
    let configs: Vec<serde_json::Value> = conn
        .prepare("SELECT config_json FROM sources WHERE project_id=?1 AND active=1")
        .map_err(db_error)?
        .query_map([project.to_string()], |r| r.get::<_, String>(0))
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?
        .into_iter()
        .map(|s| serde_json::from_str(&s).map_err(Error::from))
        .collect::<Result<_>>()?;
    let minimal = !configs.is_empty() && configs.iter().all(|c| c["context_profile"] == "minimal");
    if !minimal {
        let rules:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sources WHERE project_id=?1 AND domain='rules' AND active=1 AND freshness='fresh')",[project.to_string()],|r|r.get(0)).map_err(db_error)?;
        if !rules
            || work
                .milestone
                .as_deref()
                .is_none_or(|m| m.trim().is_empty())
        {
            return Err(Error::RuleViolation(
                "standard projects retain their declared rule and milestone requirements".into(),
            ));
        }
    }
    if let Some(key) = &work.milestone {
        let p: Projected<Plan> = crate::query::projection(conn, project, EntityKind::Plan, key)?;
        if p.source.freshness != Freshness::Fresh {
            return Err(Error::SourceStale("draft milestone is stale".into()));
        }
    }
    let goal_keys=conn.prepare("SELECT e.to_key FROM edges e JOIN sources s ON s.id=e.source_id AND s.project_id=e.project_id WHERE e.project_id=?1 AND e.active=1 AND s.active=1 AND e.from_kind='work_item' AND e.from_key=?2 AND e.to_kind='goal' AND e.relation='supports'").map_err(db_error)?.query_map(params![project.to_string(),work.meta.external_key],|r|r.get::<_,String>(0)).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
    if goal_keys.is_empty() {
        return Err(Error::RuleViolation(
            "draft activation requires an explicit goal link".into(),
        ));
    }
    for key in goal_keys {
        let goal: Projected<Goal> =
            crate::query::projection(conn, project, EntityKind::Goal, &key)?;
        if goal.source.role != "primary"
            || goal.source.freshness != Freshness::Fresh
            || goal.item.title.trim().is_empty()
            || !["active", "confirmed", "approved", "in_progress"]
                .contains(&goal.item.status.to_ascii_lowercase().as_str())
            || (goal.item.summary.trim().is_empty()
                && validate_criteria(&goal.item.success_criteria).is_err())
            || (goal
                .source
                .locator
                .replace('\\', "/")
                .contains(".awr/intake/")
                && goal.item.title == "Establish a verified project baseline")
        {
            return Err(Error::RuleViolation(
                "draft goal is missing, unconfirmed, finished or an intake placeholder".into(),
            ));
        }
    }
    Ok(())
}

/// Terminal source actions release only the creating session's occupancy; the source owner is
/// untouched. The release IDs are recorded in the same verified work action receipt.
pub(crate) fn release_terminal_claims(
    conn: &Connection,
    project: Id,
    proposal: &MutationProposal,
) -> Result<Vec<Id>> {
    let Some(binding) = proposal.bound_patch()?.work_action else {
        return Ok(Vec::new());
    };
    if !matches!(binding.action, WorkAction::Cancel | WorkAction::Complete) {
        return Ok(Vec::new());
    }
    let ids=conn.prepare("SELECT id FROM claims WHERE project_id=?1 AND work_item_id=?2 AND session_id=?3 AND status='active'").map_err(db_error)?
        .query_map(params![project.to_string(),proposal.work_item_id.map(|id|id.to_string()),proposal.created_by_session.map(|id|id.to_string())],|r|crate::catalog::id_at(r,0)).map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
    conn.execute("UPDATE claims SET status='released',released_at=?1,revision=revision+1 WHERE project_id=?2 AND work_item_id=?3 AND session_id=?4 AND status='active'",params![now_millis()?,project.to_string(),proposal.work_item_id.map(|id|id.to_string()),proposal.created_by_session.map(|id|id.to_string())]).map_err(db_error)?;
    Ok(ids)
}

impl Store {
    /// Read current domain constraints without refreshing source files or mutating state.
    pub fn check_work_proposal(&self, project: Id, proposal: &MutationProposal) -> Result<()> {
        let patch = proposal.bound_patch()?;
        let target = crate::mutation::validate_binding(
            &self.conn,
            project,
            proposal.source_id,
            &proposal.base_fingerprint,
            &proposal.mutation_type,
            &patch,
        )?;
        validate_action(
            &self.conn,
            project,
            self.project(project)?.project_revision,
            &patch,
            &target.item,
            proposal.created_by_session,
        )
    }
}

fn check_blocker(work: &WorkItem) -> Result<()> {
    if work.blocker.as_ref().is_some_and(|b| !b.trim().is_empty()) {
        return Err(Error::DependencyBlocked(
            "work has an active blocker; resolve it before completion".into(),
        ));
    }
    Ok(())
}
pub(crate) fn check_dependencies(
    conn: &Connection,
    project: Id,
    revision: Revision,
    work: &WorkItem,
    branch: Option<Id>,
    ignore_root_blocker: bool,
) -> Result<()> {
    let ready = readiness(
        conn,
        project,
        &work.meta.external_key,
        branch,
        now_millis()?,
        revision,
    )?;
    let problems = ready
        .diagnostics
        .iter()
        .filter(|d| {
            !matches!(d.code.as_str(), "status_not_selectable" | "active_claim")
                && !(ignore_root_blocker
                    && d.code == "active_blocker"
                    && d.work_item_key == work.meta.external_key)
        })
        .collect::<Vec<_>>();
    if problems.iter().any(|d| d.code == "source_not_fresh") {
        return Err(Error::SourceStale(
            "work or a required dependency source is not fresh".into(),
        ));
    }
    if !problems.is_empty() {
        return Err(Error::DependencyBlocked(serde_json::to_string(&problems)?));
    }
    Ok(())
}
pub(crate) fn validate_completion_binding(
    conn: &Connection,
    project: Id,
    work: &WorkItem,
    branch: Option<Id>,
    binding: &CompletionBinding,
) -> Result<()> {
    binding.validate()?;
    validate_criteria(&work.acceptance)?;
    let declared = binding
        .acceptance
        .iter()
        .map(|a| a.criterion.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if declared != work.acceptance.iter().map(String::as_str).collect() {
        return Err(Error::EvidenceMissing(
            "bound acceptance no longer matches every source criterion".into(),
        ));
    }
    for bound in &binding.evidence {
        let record = crate::evidence::evidence_by_id(conn, project, bound.id)?;
        if serde_json::to_value(&record.item)? != serde_json::to_value(bound)? {
            return Err(Error::EvidenceMissing(format!(
                "evidence {} changed after binding",
                bound.id
            )));
        }
        let applies = match bound.work_item_id {
            Some(id) => id == work.meta.id,
            None => bound
                .scope
                .iter()
                .any(|key| key == &work.meta.external_key || key == "*"),
        };
        let assessment = record.assess(
            Some(&binding.source_sha),
            work.meta.source_ref.section_fingerprint.as_deref(),
            branch,
        );
        if !applies
            || assessment.currency != EvidenceCurrency::Current
            || !assessment.missing_bindings.is_empty()
        {
            return Err(Error::EvidenceMissing(format!(
                "evidence {} is not current, fully bound evidence for this work and branch",
                bound.id
            )));
        }
        // YAML appends report references into a source-owned evidence namespace. Do not
        // steal a runtime record's key or discover the collision only after source writing.
        let key = format!("{}/evidence/{}", work.meta.external_key, bound.locator);
        let collision:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM evidence WHERE project_id=?1 AND external_key=?2 AND (source_id IS NULL OR source_id!=?3))",params![project.to_string(),key,work.meta.source_ref.source_id.to_string()],|r|r.get(0)).map_err(db_error)?;
        if collision {
            return Err(Error::SourceConflict("completion report reference collides with another evidence owner; register a report at a distinct locator".into()));
        }
    }
    Ok(())
}
impl Store {
    /// The successful completion's explicit evidence selection applies only to the
    /// exact current work revision/facts and runtime branch. Source-only completion
    /// or a subsequent work edit has no such selection and requires fresh assessment.
    pub fn current_completion_binding(
        &self,
        project: Id,
        key: &str,
        branch: Option<Id>,
    ) -> Result<Option<CompletionBinding>> {
        let work = self.work_item(project, key)?.item;
        if work.status != WorkStatus::Completed {
            return Ok(None);
        }
        let payload: Option<String> = self.conn.query_row(
            "SELECT payload_json FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type='work.completed' ORDER BY project_revision DESC,id DESC LIMIT 1",
            params![project.to_string(), work.meta.id.to_string(), branch.map(|id| id.to_string())],
            |row| row.get(0),
        ).optional().map_err(db_error)?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        let value: serde_json::Value = serde_json::from_str(&payload)?;
        if value["target_revision"] != serde_json::json!(work.meta.revision)
            || value["source_id"] != serde_json::json!(work.meta.source_ref.source_id)
            || value["target_after_hash"]
                != mutation_projection_hash(&serde_json::to_value(&work)?)?
        {
            return Ok(None);
        }
        let action: WorkActionBinding = serde_json::from_value(value["work_action"].clone())?;
        if action.action != WorkAction::Complete || action.to != WorkStatus::Completed {
            return Err(Error::Storage("invalid completed work receipt".into()));
        }
        let binding = action.completion.ok_or_else(|| {
            Error::Storage("completed work receipt is missing its evidence binding".into())
        })?;
        binding.validate()?;
        Ok(Some(binding))
    }

    pub fn check_completion_dependencies(
        &self,
        project: Id,
        key: &str,
        branch: Option<Id>,
    ) -> Result<()> {
        let work = self.work_item(project, key)?.item;
        check_dependencies(
            &self.conn,
            project,
            self.project(project)?.project_revision,
            &work,
            branch,
            true,
        )
    }
    /// Validate current database proof facts. The runtime additionally reads report bytes.
    pub fn check_completion_binding(
        &self,
        project: Id,
        work: &WorkItem,
        branch: Option<Id>,
        binding: &CompletionBinding,
    ) -> Result<()> {
        validate_completion_binding(&self.conn, project, work, branch, binding)
    }
}
