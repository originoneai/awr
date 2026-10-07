//! Fresh service polling preserves unchanged fact identities and verification references.
use super::{
    GitHubAdapter, GitHubError, GitHubReport, LocalGitError, digest, github::MANIFEST_CHECK,
};
use awr_team::delivery::{DeliveryCandidate, DeliveryRecord};
use awr_team_pg::{
    DeliveryReadSet, DeliveryScheduleQuery, DeliverySyncStore, IngestDeliveryFacts, PgError,
    ReserveDeliveryInspection,
};
use serde_json::{Value, json};

fn error(e: PgError) -> GitHubError {
    match super::domain_error(e) {
        LocalGitError::AuthorizationUnavailable => GitHubError::AuthorizationUnavailable,
        LocalGitError::PreconditionsChanged => GitHubError::PreconditionsChanged,
        LocalGitError::IdempotencyConflict => GitHubError::IdempotencyConflict,
        LocalGitError::Contention => GitHubError::Contention,
        LocalGitError::DomainRejected => GitHubError::DomainRejected,
        _ => GitHubError::StoreUnavailable,
    }
}

enum Slot {
    Verification(String),
    ChangeRequest(String),
    Target(String),
    Content(String),
}

impl Slot {
    fn label(&self) -> String {
        match self {
            Self::Verification(check) => format!("verification:{check}"),
            Self::ChangeRequest(_) => "change_request".into(),
            Self::Target(_) => "integration_observation".into(),
            Self::Content(_) => "integration_content_proof".into(),
        }
    }

    fn matches(&self, observation: &Value) -> bool {
        match self {
            Self::Verification(check) => {
                observation["kind"] == "verification" && observation["check"] == *check
            }
            Self::ChangeRequest(resource) => {
                observation["kind"] == "change_request"
                    && observation["provider"] == "github"
                    && observation["resource_id"] == *resource
            }
            Self::Target(reference) => {
                observation["kind"] == "integration_observation"
                    && observation["external_reference"] == *reference
            }
            Self::Content(reference) => {
                observation["kind"] == "integration_content_proof"
                    && observation["observation_reference"] == *reference
                    && observation["request_id"].is_null()
            }
        }
    }

    fn semantics(&self, report: &GitHubReport) -> Value {
        match self {
            Self::Verification(check) if check == MANIFEST_CHECK => json!([
                report.source_revision,
                report.source_artifacts,
                report.manifest_outcome,
                report.unsupported_required_checks
            ]),
            Self::Verification(check) => json!([
                report.source_revision,
                report.checks.iter().find(|c| c.check == *check)
            ]),
            Self::ChangeRequest(_) => json!([report.pull, report.pull_stable]),
            Self::Target(_) => json!([
                report.target_revision,
                report.target_artifacts,
                report.graph_contains_source,
                report.target_stable,
                report.target_precondition_matches,
                report.integration_outcome
            ]),
            Self::Content(_) => json!([
                report.source_revision,
                report.target_revision,
                report.content_witness
            ]),
        }
    }
}

// Compare every field exposed by the neutral fact view. The immutable report/index
// reconstructs fields not included in that bounded view; only PG recording time is normalized.
fn summary(record: &DeliveryRecord, recorded_at: u64) -> Option<Value> {
    let mut record = record.clone();
    let provenance = match &mut record {
        DeliveryRecord::Verification(r) => &mut r.provenance,
        DeliveryRecord::ChangeRequest(r) => &mut r.provenance,
        DeliveryRecord::IntegrationObservation(r) => &mut r.provenance,
        DeliveryRecord::IntegrationContentProof(r) => &mut r.provenance,
        _ => return None,
    };
    provenance.recorded_at_unix_ms = recorded_at;
    Some(match record {
        DeliveryRecord::Verification(r) => json!({"kind":"verification","check":r.check,
            "run_id":r.run_id,"outcome":r.outcome,"provenance":r.provenance}),
        DeliveryRecord::ChangeRequest(r) => json!({"kind":"change_request","provider":r.provider,
            "resource_id":r.resource_id,"provenance":r.provenance}),
        DeliveryRecord::IntegrationObservation(r) => json!({"kind":"integration_observation",
            "external_reference":r.external_reference,"outcome":r.outcome,
            "result_revision":r.result_revision,"provenance":r.provenance}),
        DeliveryRecord::IntegrationContentProof(r) => json!({"kind":"integration_content_proof",
            "request_id":r.request_id,"observation_reference":r.observation_reference,
            "result_revision":r.result_revision,"witness_kind":r.witness.kind(),"provenance":r.provenance}),
        _ => return None,
    })
}

impl GitHubAdapter {
    fn current_slots(&self, candidate: &DeliveryCandidate) -> Vec<Slot> {
        let mut slots = vec![Slot::Verification(MANIFEST_CHECK.into())];
        slots.extend(
            self.config
                .checks
                .iter()
                .filter(|c| candidate.binding.required_checks.contains(&c.check))
                .map(|c| Slot::Verification(c.check.clone())),
        );
        if let Some(number) = self.config.pull_number {
            slots.push(Slot::ChangeRequest(format!(
                "{}:{number}",
                self.config.repository_id
            )));
        }
        slots.push(Slot::Target(format!(
            "github-target:{}:{}",
            self.config.repository_id, self.config.target_branch
        )));
        slots.push(Slot::Content(format!(
            "github-target:{}:{}",
            self.config.repository_id, self.config.target_branch
        )));
        slots
    }

    fn previous_current_report(
        &self,
        facts: &[Value],
        slot: &Slot,
        candidate: &DeliveryCandidate,
        schedule: &Value,
    ) -> Option<GitHubReport> {
        let binding = candidate.binding.digest().ok()?;
        let fact = facts.iter().find(|f| {
            f["current"] == true
                && f["receipt"]["connector_id"] == self.config.connector_id
                && slot.matches(&f["observation"])
        })?;
        let receipt = &fact["receipt"];
        let provenance = &fact["observation"]["provenance"];
        if receipt["state"] != "applied"
            || receipt["fact_source"] != "adapter_observation"
            || receipt["candidate_digest"] != binding
            || receipt["source_snapshot_id"] != schedule["read_set"]["source_snapshot_id"]
            || receipt["selection_version"] != schedule["selection"]["selection_version"]
            || !receipt["fact_ids"].as_array()?.contains(&fact["fact_id"])
            || provenance["source"] != "adapter_observation"
        {
            return None;
        }
        let prefix = format!("awr-github-report:{}:", self.config.adapter_id);
        let hash = provenance["reference"].as_str()?.strip_prefix(&prefix)?;
        // A missing/corrupt report is not equality. A new reserved observation
        // can recover proof without modifying the original immutable file.
        let report: GitHubReport = serde_json::from_slice(&self.report_bytes(hash).ok()?).ok()?;
        if receipt["inspection_id"] != report.inspection_id {
            return None;
        }
        let snapshot = self
            .cached(
                &self.inspection_index(&report.inspection_id),
                candidate,
                &report.inspection_id,
            )
            .ok()??;
        let recorded_at = receipt["recorded_at_unix_ms"].as_u64()?;
        let consistent = snapshot.report_artifact.sha256 == hash
            && snapshot.records.iter().any(|r| {
                summary(&r.record, recorded_at)
                    .is_some_and(|expected| expected == fact["observation"])
            });
        consistent.then_some(snapshot.report)
    }

    async fn current_schedule(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
    ) -> Result<Value, GitHubError> {
        let config = &self.config;
        let schedule = store
            .schedule(
                &config.tenant_id,
                &config.project_id,
                credential,
                DeliveryScheduleQuery {
                    work_id: config.work_id.clone(),
                    connector_id: config.connector_id.clone(),
                    cursor: None,
                    limit: 1,
                },
            )
            .await
            .map_err(error)?;
        if schedule["selection_state"] != "current" {
            return Err(GitHubError::PreconditionsChanged);
        }
        if schedule["connector"]["provider"] != "github"
            || schedule["connector"]["resource"] != config.resource
        {
            return Err(GitHubError::BindingMismatch);
        }
        Ok(schedule)
    }

    async fn recheck_unchanged_admission(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        original: &Value,
    ) -> Result<(), GitHubError> {
        let current = self.current_schedule(store, credential).await?;
        if current["read_set"] != original["read_set"]
            || current["selection"] != original["selection"]
            || current["connector"]["connector_version"]
                != original["connector"]["connector_version"]
        {
            return Err(GitHubError::PreconditionsChanged);
        }
        Ok(())
    }

    /// Fresh service-owned query, without a caller-selected inspection or authority.
    /// Unchanged slots keep their actual fact IDs, reports and verification run IDs.
    pub async fn reconcile_current(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
    ) -> Result<Value, GitHubError> {
        let config = &self.config;
        let schedule = self.current_schedule(store, credential).await?;
        let set: DeliveryReadSet = serde_json::from_value(schedule["read_set"].clone())
            .map_err(|_| GitHubError::InvalidResponse)?;
        let candidate: DeliveryCandidate =
            serde_json::from_value(schedule["selection"]["candidate"].clone())
                .map_err(|_| GitHubError::InvalidResponse)?;
        self.validate_candidate(&candidate)?;
        if set.work_id != config.work_id || set.workstream_id.to_string() != config.workstream_id {
            return Err(GitHubError::BindingMismatch);
        }
        let view = store
            .inspect(
                &config.tenant_id,
                &config.project_id,
                credential,
                &config.work_id,
            )
            .await
            .map_err(error)?;
        if view["selected_current"] != true
            || view["candidate"] != json!(candidate)
            || view["source_snapshot_id"] != set.source_snapshot_id
            || view["coordinator_epoch"] != set.coordinator_epoch
            || view["selection_version"] != schedule["selection"]["selection_version"]
        {
            return Err(GitHubError::PreconditionsChanged);
        }
        let connector = view["connectors"]
            .as_array()
            .ok_or(GitHubError::InvalidResponse)?
            .iter()
            .find(|c| c["connector_id"] == config.connector_id)
            .ok_or(GitHubError::InvalidResponse)?;
        if connector["connector_version"] != schedule["connector"]["connector_version"]
            || connector["enabled"] != true
            || connector["current_epoch"] != true
        {
            return Err(GitHubError::PreconditionsChanged);
        }
        let facts = view["facts"]
            .as_array()
            .ok_or(GitHubError::InvalidResponse)?;
        let slots = self.current_slots(&candidate);
        if view["facts_truncated"] == true
            && slots.iter().any(|slot| {
                !facts.iter().any(|f| {
                    f["receipt"]["connector_id"] == config.connector_id
                        && slot.matches(&f["observation"])
                })
            })
        {
            return Err(GitHubError::InvalidResponse);
        }
        let previous: Vec<_> = slots
            .iter()
            .map(|slot| self.previous_current_report(facts, slot, &candidate, &schedule))
            .collect();
        let changed = |report: &GitHubReport| {
            slots
                .iter()
                .zip(&previous)
                .filter_map(|(slot, prior)| {
                    prior
                        .as_ref()
                        .is_none_or(|p| slot.semantics(p) != slot.semantics(report))
                        .then_some(slot)
                })
                .collect::<Vec<_>>()
        };
        let unchanged = |read_only| {
            json!({"unchanged":true,"changed_slots":[],"observation":null,
            "state_basis":"at_read","read_only":read_only,"acceptance_ready":false,
            "execution_authorized":false,"source_synchronized":false})
        };
        let probe = self.probe(&candidate, "scheduled-probe").await?;
        if changed(&probe).is_empty() {
            self.recheck_unchanged_admission(store, credential, &schedule)
                .await?;
            return Ok(unchanged(true));
        }
        let mut predecessors: Vec<_> = facts
            .iter()
            .filter(|f| f["receipt"]["connector_id"] == config.connector_id)
            .map(|f| f["fact_id"].clone())
            .collect();
        predecessors.sort_by_key(Value::to_string);
        let semantics: Vec<_> = slots
            .iter()
            .map(|s| json!([s.label(), s.semantics(&probe)]))
            .collect();
        let binding = candidate
            .binding
            .digest()
            .map_err(|_| GitHubError::BindingMismatch)?;
        let key = digest(
            &serde_json::to_vec(&json!([
                "github-current-v1",
                self.config_digest,
                set,
                schedule["connector"]["connector_version"],
                schedule["selection"]["selection_version"],
                binding,
                predecessors,
                semantics
            ]))
            .map_err(|_| GitHubError::InvalidResponse)?,
        );
        let mut prefix = format!("stable:{key}");
        for attempt in 0..2 {
            let reserved = store
                .reserve_inspection(
                    &config.tenant_id,
                    &config.project_id,
                    credential,
                    ReserveDeliveryInspection {
                        request_id: format!("{prefix}:reserve"),
                        read_set: set.clone(),
                        connector_id: config.connector_id.clone(),
                        connector_version: schedule["connector"]["connector_version"]
                            .as_str()
                            .ok_or(GitHubError::InvalidResponse)?
                            .into(),
                        candidate_digest: binding.clone(),
                        lease_seconds: 120,
                    },
                )
                .await
                .map_err(error)?;
            if reserved["inspection_lease"]["live"] != true {
                if attempt == 0 {
                    prefix = format!("renew:{}", awr_core::Id::new());
                    continue;
                }
                return Err(GitHubError::PreconditionsChanged);
            }
            let inspection = reserved["data"]["inspection_id"]
                .as_str()
                .ok_or(GitHubError::InvalidResponse)?;
            // The preliminary probe grants no authority and is never ingested.
            let snapshot = self.inspect(&candidate, inspection).await?;
            let changed_slots = changed(&snapshot.report);
            let records = snapshot
                .records
                .into_iter()
                .filter(|r| {
                    summary(&r.record, 0)
                        .is_some_and(|v| changed_slots.iter().any(|s| s.matches(&v)))
                })
                .collect::<Vec<_>>();
            if records.is_empty() {
                self.recheck_unchanged_admission(store, credential, &schedule)
                    .await?;
                return Ok(unchanged(false));
            }
            match store
                .ingest_facts(
                    &config.tenant_id,
                    &config.project_id,
                    credential,
                    IngestDeliveryFacts {
                        request_id: format!("{prefix}:ingest"),
                        read_set: set.clone(),
                        connector_id: config.connector_id.clone(),
                        inspection_id: inspection.into(),
                        event_id: format!("{prefix}:observation"),
                        records,
                    },
                )
                .await
            {
                Ok(observation) => {
                    return Ok(json!({"unchanged":false,
                    "changed_slots":changed_slots.iter().map(|s| s.label()).collect::<Vec<_>>(),
                    "observation":observation,"state_basis":"at_read","read_only":false,
                    "acceptance_ready":false,"execution_authorized":false,"source_synchronized":false}));
                }
                Err(PgError::LeaseExpired) if attempt == 0 => {
                    prefix = format!("renew:{}", awr_core::Id::new());
                }
                Err(e) => return Err(error(e)),
            }
        }
        Err(GitHubError::PreconditionsChanged)
    }
}
