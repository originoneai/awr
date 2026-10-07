//! Fresh scheduled observations without manufacturing a new verification on every poll.
use super::{LocalGitAdapter, LocalGitError, LocalGitReport, digest, domain_error};
use awr_team::delivery::{DeliveryCandidate, DeliveryRecord};
use awr_team_pg::{
    DeliveryReadSet, DeliveryScheduleQuery, DeliverySyncStore, IngestDeliveryFacts, PgError,
    ReserveDeliveryInspection,
};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Slot {
    Verification,
    Integration,
    Content,
}

impl Slot {
    fn name(self) -> &'static str {
        match self {
            Self::Verification => "verification",
            Self::Integration => "integration_observation",
            Self::Content => "integration_content_proof",
        }
    }

    fn matches(self, observation: &Value) -> bool {
        observation["kind"] == self.name()
            && (!matches!(self, Self::Verification)
                || observation["check"] == super::local_git::MANIFEST_CHECK)
    }

    fn semantics(self, report: &LocalGitReport) -> Value {
        match self {
            Self::Verification => json!([
                report.source_revision,
                report.source_artifacts,
                report.manifest_outcome,
                report.unsupported_required_checks
            ]),
            Self::Integration => json!([
                report.target_revision,
                report.target_artifacts,
                report.graph_contains_source,
                report.target_stable,
                report.target_precondition_matches,
                report.integration_outcome
            ]),
            Self::Content => json!([
                report.source_revision,
                report.target_revision,
                report.content_witness
            ]),
        }
    }
}

const SLOTS: [Slot; 3] = [Slot::Verification, Slot::Integration, Slot::Content];

impl LocalGitAdapter {
    fn prior_report(&self, facts: &[Value], slot: Slot, binding: &str) -> Option<LocalGitReport> {
        let fact = facts.iter().find(|f| {
            f["current"] == true
                && f["receipt"]["connector_id"] == self.config.connector_id
                && slot.matches(&f["observation"])
        })?;
        let observation = &fact["observation"];
        let provenance = &observation["provenance"];
        if provenance["source"] != "adapter_observation"
            || fact["receipt"]["state"] != "applied"
            || fact["receipt"]["candidate_digest"] != binding
        {
            return None;
        }
        let prefix = format!("awr-local-git-report:{}:", self.config.adapter_id);
        let hash = provenance["reference"].as_str()?.strip_prefix(&prefix)?;
        // Missing or corrupt proof is never an equality result. A fresh reserved
        // inspection can publish a new proof without rewriting the old report.
        let report: LocalGitReport = serde_json::from_slice(&self.report_bytes(hash).ok()?).ok()?;
        if report.version != 1
            || report.adapter_id != self.config.adapter_id
            || report.config_digest != self.config_digest
            || report.binding_digest != binding
            || fact["receipt"]["inspection_id"] != report.inspection_id
            || provenance["observed_at_unix_ms"] != report.observed_at_unix_ms
        {
            return None;
        }
        let consistent = match slot {
            Slot::Verification => {
                observation["run_id"] == format!("local-git-{hash}")
                    && observation["outcome"] == json!(report.manifest_outcome)
            }
            Slot::Integration => {
                observation["external_reference"]
                    == format!("local-git-target:{}", self.config.adapter_id)
                    && observation["outcome"] == json!(report.integration_outcome)
                    && observation["result_revision"] == json!(report.target_revision)
            }
            Slot::Content => {
                report.content_witness.is_some()
                    && observation["observation_reference"]
                        == format!("local-git-target:{}", self.config.adapter_id)
                    && observation["request_id"].is_null()
                    && observation["result_revision"] == json!(report.target_revision)
                    && report
                        .content_witness
                        .as_ref()
                        .is_some_and(|w| observation["witness_kind"] == w.kind())
            }
        };
        consistent.then_some(report)
    }

    /// Service-owned fresh polling. No caller path, identity, approval or read set.
    /// Explicit retries of an already published poll still use `reconcile`.
    pub async fn reconcile_current(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
    ) -> Result<Value, LocalGitError> {
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
            .map_err(domain_error)?;
        if schedule["selection_state"] != "current" {
            return Err(LocalGitError::PreconditionsChanged);
        }
        if schedule["connector"]["resource"] != config.resource
            || schedule["connector"]["provider"] != "local_git"
        {
            return Err(LocalGitError::BindingMismatch);
        }
        let set: DeliveryReadSet = serde_json::from_value(schedule["read_set"].clone())
            .map_err(|_| LocalGitError::InvalidStoreResponse)?;
        let candidate: DeliveryCandidate =
            serde_json::from_value(schedule["selection"]["candidate"].clone())
                .map_err(|_| LocalGitError::InvalidStoreResponse)?;
        self.validate_candidate(&candidate)?;
        let binding = candidate
            .binding
            .digest()
            .map_err(|_| LocalGitError::BindingMismatch)?;
        let view = store
            .inspect(
                &config.tenant_id,
                &config.project_id,
                credential,
                &config.work_id,
            )
            .await
            .map_err(domain_error)?;
        if view["selected_current"] != true
            || view["candidate"] != json!(candidate)
            || view["source_snapshot_id"] != set.source_snapshot_id
            || view["coordinator_epoch"] != set.coordinator_epoch
            || view["selection_version"] != schedule["selection"]["selection_version"]
        {
            return Err(LocalGitError::PreconditionsChanged);
        }
        let facts = view["facts"]
            .as_array()
            .ok_or(LocalGitError::InvalidStoreResponse)?;
        if view["facts_truncated"] == true
            && SLOTS.iter().any(|slot| {
                !facts.iter().any(|f| {
                    f["receipt"]["connector_id"] == config.connector_id
                        && slot.matches(&f["observation"])
                })
            })
        {
            // A bounded response cannot prove an omitted slot is absent.
            return Err(LocalGitError::InvalidStoreResponse);
        }
        let previous = SLOTS.map(|slot| self.prior_report(facts, slot, &binding));
        let changed = |report: &LocalGitReport| -> Vec<Slot> {
            SLOTS
                .into_iter()
                .zip(&previous)
                .filter_map(|(slot, prior)| {
                    prior
                        .as_ref()
                        .is_none_or(|p| slot.semantics(p) != slot.semantics(report))
                        .then_some(slot)
                })
                .collect()
        };
        let probe = self.probe(&candidate).await?;
        if changed(&probe).is_empty() {
            return Ok(
                json!({"unchanged":true,"changed_slots":[],"observation":null,
                "state_basis":"at_read","read_only":true,"acceptance_ready":false,
                "execution_authorized":false,"source_synchronized":false}),
            );
        }
        let mut predecessors: Vec<_> = facts
            .iter()
            .filter(|f| f["receipt"]["connector_id"] == config.connector_id)
            .map(|f| f["fact_id"].clone())
            .collect();
        predecessors.sort_by_key(Value::to_string);
        let key = digest(
            &serde_json::to_vec(&json!([
                "local-git-current-v1",
                self.config_digest,
                set,
                schedule["connector"]["connector_version"],
                binding,
                predecessors,
                Slot::Verification.semantics(&probe),
                Slot::Integration.semantics(&probe),
                Slot::Content.semantics(&probe)
            ]))
            .map_err(|_| LocalGitError::InvalidStoreResponse)?,
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
                            .ok_or(LocalGitError::InvalidStoreResponse)?
                            .into(),
                        candidate_digest: binding.clone(),
                        lease_seconds: 120,
                    },
                )
                .await
                .map_err(domain_error)?;
            let inspection = reserved["data"]["inspection_id"]
                .as_str()
                .ok_or(LocalGitError::InvalidStoreResponse)?;
            if reserved["inspection_lease"]["live"] != true {
                if attempt == 0 {
                    prefix = format!("renew:{}", awr_core::Id::new());
                    continue;
                }
                return Err(LocalGitError::PreconditionsChanged);
            }
            // Reobserve after live domain admission; the preliminary probe is
            // neither a persisted verification nor permission to publish facts.
            let snapshot = self.inspect(&candidate, inspection).await?;
            let slots = changed(&snapshot.report);
            let records = snapshot
                .records
                .into_iter()
                .filter(|record| {
                    slots.iter().any(|slot| {
                        matches!(
                            (slot, &record.record),
                            (Slot::Verification, DeliveryRecord::Verification(_))
                                | (Slot::Integration, DeliveryRecord::IntegrationObservation(_))
                                | (Slot::Content, DeliveryRecord::IntegrationContentProof(_))
                        )
                    })
                })
                .collect::<Vec<_>>();
            if records.is_empty() {
                return Ok(
                    json!({"unchanged":true,"changed_slots":[],"observation":null,
                    "state_basis":"at_read","read_only":false,"acceptance_ready":false,
                    "execution_authorized":false,"source_synchronized":false}),
                );
            }
            let result = store
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
                .await;
            match result {
                Ok(observation) => {
                    return Ok(json!({"unchanged":false,
                    "changed_slots":slots.iter().map(|s| s.name()).collect::<Vec<_>>(),
                    "observation":observation,"state_basis":"at_read","read_only":false,
                    "acceptance_ready":false,"execution_authorized":false,"source_synchronized":false}));
                }
                Err(PgError::LeaseExpired) if attempt == 0 => {
                    // Only a confirmed expired observation lease may get a fresh
                    // reservation. This never renews or redispatches a Git effect.
                    prefix = format!("renew:{}", awr_core::Id::new());
                }
                Err(error) => return Err(domain_error(error)),
            }
        }
        Err(LocalGitError::PreconditionsChanged)
    }
}
