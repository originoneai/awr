use crate::{
    Store,
    catalog::{SOURCE_COLUMNS, revision_at, source_row},
    db_error,
    query::{projection, projections},
};
use awr_core::*;
use rusqlite::{Connection, params};

fn evidence_rows(conn: &Connection, project: Id, key: Option<&str>) -> Result<Vec<EvidenceRecord>> {
    evidence_rows_by(conn, project, key, false, false)
}
fn evidence_rows_by(
    conn: &Connection,
    project: Id,
    key: Option<&str>,
    exact_id: bool,
    scoped: bool,
) -> Result<Vec<EvidenceRecord>> {
    let visibility = if scoped {
        crate::scoped_read::visible("evidence", "e.id")
    } else {
        "1".into()
    };
    let columns = SOURCE_COLUMNS
        .split(',')
        .map(|c| format!("s.{c}"))
        .collect::<Vec<_>>()
        .join(",");
    conn.prepare(&format!("SELECT {columns},e.payload_json,p.project_revision FROM evidence e
        LEFT JOIN sources s ON s.id=e.source_id AND s.project_id=e.project_id JOIN projects p ON p.id=e.project_id
        WHERE ({visibility}) AND e.project_id=?1 AND e.active=1 AND (e.source_id IS NULL OR s.active=1) AND (?2 IS NULL OR (NOT ?3 AND e.external_key=?2) OR e.id=?2)
        ORDER BY e.external_key")).map_err(db_error)?.query_map(params![project.to_string(),key,exact_id],|r| {
            let item=serde_json::from_str(&r.get::<_,String>(11)?).map_err(|e|rusqlite::Error::FromSqlConversionFailure(11,rusqlite::types::Type::Text,Box::new(e)))?;
            let source=if r.get::<_,Option<String>>(0)?.is_some() {Some(source_row(r)?)} else {None};
            Ok(EvidenceRecord {item,source,project_revision:revision_at(r,12)?})
        }).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
}

pub(crate) fn evidence_by_id(conn: &Connection, project: Id, id: Id) -> Result<EvidenceRecord> {
    evidence_rows_by(conn, project, Some(&id.to_string()), true, false)?
        .pop()
        .ok_or_else(|| {
            Error::EvidenceMissing(format!("bound evidence {id} is no longer available"))
        })
}

impl Store {
    /// Immutable proposal bindings use IDs exclusively, never ambiguous external-key lookup.
    pub fn evidence_by_id(&self, project: Id, id: Id) -> Result<EvidenceRecord> {
        evidence_by_id(&self.conn, project, id)
    }
    pub fn decision(&self, project: Id, key: &str) -> Result<Projected<Decision>> {
        let keys=self.conn.prepare("SELECT d.external_key FROM decisions d JOIN sources s ON s.id=d.source_id AND s.project_id=d.project_id WHERE d.project_id=?1 AND d.active=1 AND s.active=1 AND (d.external_key=?2 OR d.id=?2) LIMIT 2").map_err(db_error)?.query_map(params![project.to_string(),key],|r|r.get::<_,String>(0)).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        match keys.as_slice() {
            [resolved] => projection(&self.conn, project, EntityKind::Decision, resolved),
            [] => Err(Error::NotFound(format!("decision {key}"))),
            _ => Err(Error::InvalidInput(
                "decision ID conflicts with another external key".into(),
            )),
        }
    }
    pub fn decisions(&self, project: Id) -> Result<Vec<Projected<Decision>>> {
        self.project(project)?;
        projections(&self.conn, project, EntityKind::Decision, None)
    }

    /// Only accepted decisions participate. Unscoped/invalid matches remain explicit unknowns.
    pub fn decisions_for_work(&self, project: Id, key: &str) -> Result<Vec<RelatedDecision>> {
        self.decisions_for_work_with_paths(project, key, None)
    }

    /// Explicit concrete execution paths may refine broad/unknown work-source path metadata.
    pub fn decisions_for_work_with_paths(
        &self,
        project: Id,
        key: &str,
        paths: Option<&[String]>,
    ) -> Result<Vec<RelatedDecision>> {
        self.decisions_for_work_scoped(project, key, paths, false)
    }
    pub(crate) fn decisions_for_work_scoped(
        &self,
        project: Id,
        key: &str,
        paths: Option<&[String]>,
        scoped: bool,
    ) -> Result<Vec<RelatedDecision>> {
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let work: Projected<WorkItem> = projection(&tx, project, EntityKind::WorkItem, key)?;
        let known_paths = concrete_scope_paths(
            paths.or_else(|| (!work.item.paths.is_empty()).then_some(work.item.paths.as_slice())),
        );
        let decisions: Vec<Projected<Decision>> = if scoped {
            crate::query::scoped_projections(&tx, project, EntityKind::Decision)?
        } else {
            projections(&tx, project, EntityKind::Decision, None)?
        };
        let mut result = Vec::new();
        for decision in decisions
            .into_iter()
            .filter(|d| d.item.status == DecisionStatus::Accepted)
        {
            let (linked, link_stale):(bool,bool)=tx.query_row("SELECT count(*)>0,coalesce(max(s.freshness!='fresh'),0) FROM edges e JOIN sources s ON s.id=e.source_id AND s.project_id=e.project_id
                WHERE e.project_id=?1 AND e.active=1 AND s.active=1 AND e.from_kind='decision' AND e.from_key=?2
                  AND e.relation='affects' AND e.to_kind='work_item' AND e.to_key=?3",params![project.to_string(),decision.item.meta.external_key,key],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db_error)?;
            let mut reasons = Vec::new();
            if link_stale || work.source.freshness != Freshness::Fresh {
                reasons.push("decision association source is not fresh".into());
            }
            let mut relevant = linked
                || decision
                    .item
                    .affected_keys
                    .iter()
                    .any(|k| k == key || k == "*");
            let mut path_unknown = Vec::new();
            for pattern in &decision.item.paths {
                match known_paths
                    .as_deref()
                    .map(|paths| match_path_scope(pattern, paths))
                {
                    Some(Ok(m)) => relevant |= m,
                    Some(Err(error)) => path_unknown.push(error),
                    None => {
                        path_unknown.push("work paths are unknown or describe a broad area".into())
                    }
                }
            }
            if !relevant {
                reasons.extend(path_unknown);
            }
            if !relevant && decision.item.affected_keys.is_empty() && decision.item.paths.is_empty()
            {
                reasons.push("decision applicability is not declared".into());
            }
            if decision.source.freshness != Freshness::Fresh {
                reasons.push("decision source is not fresh".into());
            }
            let relevance = if !reasons.is_empty() {
                Applicability::Unknown
            } else if relevant {
                Applicability::Applicable
            } else {
                Applicability::NotApplicable
            };
            if relevance != Applicability::NotApplicable {
                result.push(RelatedDecision {
                    decision,
                    relevance,
                    reasons,
                });
            }
        }
        tx.commit().map_err(db_error)?;
        Ok(result)
    }

    pub fn evidence(&self, project: Id, key: &str) -> Result<EvidenceRecord> {
        let mut records = evidence_rows(&self.conn, project, Some(key))?;
        match records.len() {
            0 => Err(Error::NotFound(format!("evidence {key}"))),
            1 => Ok(records.pop().unwrap()),
            _ => Err(Error::InvalidInput(
                "evidence ID conflicts with another external key".into(),
            )),
        }
    }
    pub fn evidence_records(&self, project: Id) -> Result<Vec<EvidenceRecord>> {
        self.project(project)?;
        evidence_rows(&self.conn, project, None)
    }

    /// Associations are explicit work IDs, supported_by edges, or work/project scope keys.
    pub fn evidence_for_work(
        &self,
        project: Id,
        key: &str,
        source_sha: Option<&str>,
        branch: Option<Id>,
    ) -> Result<Vec<EvidenceAssessment>> {
        self.evidence_for_work_scoped(project, key, source_sha, branch, false)
    }
    pub(crate) fn evidence_for_work_scoped(
        &self,
        project: Id,
        key: &str,
        source_sha: Option<&str>,
        branch: Option<Id>,
        scoped: bool,
    ) -> Result<Vec<EvidenceAssessment>> {
        if source_sha.is_some_and(|s| !is_source_sha(s)) {
            return Err(Error::InvalidInput(
                "evidence currency requires a full source SHA".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let work: Projected<WorkItem> = projection(&tx, project, EntityKind::WorkItem, key)?;
        let mut results = Vec::new();
        for record in evidence_rows_by(&tx, project, None, false, scoped)? {
            let (linked, link_stale):(bool,bool)=tx.query_row("SELECT count(*)>0,coalesce(max(s.freshness!='fresh'),0) FROM edges e JOIN sources s ON s.id=e.source_id AND s.project_id=e.project_id
                WHERE e.project_id=?1 AND e.active=1 AND s.active=1 AND e.from_kind='work_item' AND e.from_key=?2
                  AND e.relation='supported_by' AND e.to_kind='evidence' AND e.to_key=?3",params![project.to_string(),key,record.item.external_key],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db_error)?;
            if linked
                || record.item.work_item_id == Some(work.item.meta.id)
                || (record.item.work_item_id.is_none()
                    && record
                        .item
                        .scope
                        .iter()
                        .any(|scope| scope == key || scope == "*"))
            {
                let mut assessment =
                    record.assess_for_work(source_sha, &work.item.meta.source_ref, branch);
                if link_stale || work.source.freshness != Freshness::Fresh {
                    assessment
                        .reasons
                        .push("evidence association source is not fresh".into());
                    if assessment.currency == EvidenceCurrency::Current {
                        assessment.currency = EvidenceCurrency::Unknown;
                    }
                }
                results.push(assessment);
            }
        }
        tx.commit().map_err(db_error)?;
        Ok(results)
    }

    /// Append an explicit evidence record. Events and work status never infer/promote its level.
    /// Report bindings are supplied assertions; this operation does not execute the command.
    pub fn record_evidence(
        &mut self,
        project: Id,
        expected: Revision,
        draft: EvidenceDraft,
    ) -> Result<(Evidence, Event)> {
        awr_core::ensure_public_data(&draft)?;
        if [
            &draft.external_key,
            &draft.evidence_type,
            &draft.summary,
            &draft.locator,
        ]
        .iter()
        .any(|s| s.trim().is_empty())
            || draft.scope.is_empty()
            || draft.scope.iter().any(|s| s.trim().is_empty())
        {
            return Err(Error::InvalidInput("evidence requires an external key, type, summary, report locator and nonempty scope".into()));
        }
        if draft.source_sha.as_ref().is_some_and(|s| !is_source_sha(s))
            || draft
                .sha256
                .as_ref()
                .is_some_and(|s| s.len() != 64 || !s.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(Error::InvalidInput(
                "evidence hashes must be full hexadecimal hashes".into(),
            ));
        }
        if draft.verified_at.is_some_and(|t| t < 0) {
            return Err(Error::InvalidInput("verified_at cannot be negative".into()));
        }
        let item = Evidence {
            id: Id::new(),
            project_id: project,
            work_item_id: None,
            external_key: draft.external_key,
            evidence_type: draft.evidence_type,
            level: draft.level,
            summary: draft.summary,
            locator: draft.locator,
            sha256: draft.sha256.map(|s| s.to_ascii_lowercase()),
            source_sha: draft.source_sha.map(|s| s.to_ascii_lowercase()),
            section_fingerprint: None,
            command: draft.command,
            scope: draft.scope,
            source_ref: None,
            branch_id: draft.branch_id,
            revision: 1,
            verified_at: draft.verified_at,
        };
        let missing = EvidenceRecord {
            item: item.clone(),
            source: None,
            project_revision: expected,
        }
        .assess(None, None, draft.branch_id)
        .missing_bindings;
        if matches!(
            item.level,
            EvidenceLevel::LocallyVerified
                | EvidenceLevel::RealEnvironmentValidated
                | EvidenceLevel::ReleaseCandidate
                | EvidenceLevel::Released
        ) && !missing.is_empty()
        {
            return Err(Error::EvidenceMissing(format!(
                "verified evidence is missing bindings: {}",
                missing.join(", ")
            )));
        }
        let mut event = EventDraft::new(
            "evidence.recorded",
            format!("Recorded evidence {}", item.external_key),
        );
        event.branch_id = draft.branch_id;
        event.payload = serde_json::json!({"evidence_id":item.id,"external_key":item.external_key,"level":item.level});
        if let Some(key) = &draft.work_item_key {
            event.work_item_id = Some(self.work_item(project, key)?.item.meta.id);
        }
        self.runtime_transaction(project,expected,event,|tx,_| {
            crate::session::require_branch(tx,project,item.branch_id)?;
            let mut item=item;
            if let Some(key)=&draft.work_item_key {
                let work:Projected<WorkItem>=projection(tx,project,EntityKind::WorkItem,key)?;
                item.work_item_id=Some(work.item.meta.id);
                // Attest that this evidence's SHA identifies this ledger snapshot.
                // Work-aware reads may then reuse it across ledger-only SHA drift
                // while the task contract is unchanged. Do not stamp old snapshots
                // or Git/code revisions with the current ledger's contract. A
                // known-stale projection cannot attest the registration snapshot.
                let current = work
                    .item
                    .meta
                    .source_ref
                    .source_fingerprint
                    .trim_start_matches("sha256:");
                let recorded_against_current = item
                    .source_sha
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(current));
                if item.section_fingerprint.is_none()
                    && work.source.freshness == Freshness::Fresh
                    && recorded_against_current
                {
                    item.section_fingerprint =
                        work.item.meta.source_ref.section_fingerprint.clone();
                }
            }
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM evidence WHERE project_id=?1 AND external_key=?2)",params![project.to_string(),item.external_key],|r|r.get(0)).map_err(db_error)?;
            if exists {return Err(Error::SourceConflict(format!("evidence key {} already exists; append a new record",item.external_key)));}
            let level=serde_json::to_value(item.level)?;
            tx.execute("INSERT INTO evidence(id,project_id,work_item_id,external_key,evidence_type,level,summary,locator,sha256,source_sha,command,scope_json,branch_id,revision,verified_at,payload_json)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,1,?14,?15)",params![item.id.to_string(),project.to_string(),item.work_item_id.map(|id|id.to_string()),item.external_key,item.evidence_type,level.as_str(),item.summary,item.locator,item.sha256,item.source_sha,item.command,serde_json::to_string(&item.scope)?,item.branch_id.map(|id|id.to_string()),item.verified_at,serde_json::to_string(&item)?]).map_err(db_error)?;
            Ok(item)
        })
    }
}
