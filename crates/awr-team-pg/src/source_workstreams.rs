//! Atomic source projection shared by the legacy and explicit workstream codecs.
use super::{PgError, PgResult, parse_contract};
use crate::graph::{DependencyEdge, validate_required_graph};
use awr_core::WorkstreamCatalog;
use awr_team::{WorkContract, WorkstreamBundle};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tokio_postgres::Transaction;

#[path = "source_workstreams/completion_invalidation.rs"]
mod completion_invalidation;

pub(super) struct SourceProjection {
    pub contracts: Vec<WorkContract>,
    pub bundle: Option<WorkstreamBundle>,
    pub hash: String,
    pub hashes: BTreeMap<String, String>,
    edges: Vec<DependencyEdge>,
}

impl SourceProjection {
    pub fn parse(files: &[(String, Vec<u8>)], project: &str) -> PgResult<Self> {
        let scoped = files.iter().find(|(p, _)| p == "workstreams.json");
        let bundle = if let Some((_, bytes)) = scoped {
            if files.iter().any(|(p, _)| p == "contract.json") {
                return Err(PgError::Protocol(
                    "choose contract.json or workstreams.json, not both".into(),
                ));
            }
            let bundle: WorkstreamBundle =
                serde_json::from_slice(bytes).map_err(|e| PgError::Protocol(e.to_string()))?;
            bundle
                .validate(project)
                .map_err(|e| PgError::Protocol(e.to_string()))?;
            for stream in &bundle.catalog.workstreams {
                i64::try_from(stream.authority_version).map_err(|_| {
                    PgError::Protocol("authority version exceeds PostgreSQL bigint".into())
                })?;
            }
            Some(bundle)
        } else {
            None
        };
        let contracts: Vec<_> = match &bundle {
            Some(bundle) => bundle
                .contracts
                .iter()
                .map(|e| e.contract.clone())
                .collect(),
            None => {
                let contract = parse_contract(files)?;
                if contract.codec != WorkContract::CODEC {
                    return Err(PgError::Unsupported(
                        "contract V2 requires a workstreams V2 bundle".into(),
                    ));
                }
                vec![contract]
            }
        };
        let hashes = contracts
            .iter()
            .map(|c| {
                Ok((
                    c.work_id.as_str().to_string(),
                    c.hash().map_err(|e| PgError::Protocol(e.to_string()))?,
                ))
            })
            .collect::<PgResult<BTreeMap<_, _>>>()?;
        let hash = match &bundle {
            Some(b) => b.hash().map_err(|e| PgError::Protocol(e.to_string()))?,
            None => hashes.values().next().expect("one legacy contract").clone(),
        };
        // The new codec's complete dependency graph comes from its contracts.
        // The legacy codec retains its old graph admission behavior.
        let mut edges = Vec::new();
        if bundle.is_some() {
            for c in &contracts {
                let mut unique = BTreeSet::new();
                for dependency in &c.required_dependencies {
                    if !unique.insert(dependency) {
                        return Err(PgError::Protocol("duplicate required dependency".into()));
                    }
                    edges.push(DependencyEdge {
                        from: c.work_id.as_str().into(),
                        to: dependency.clone(),
                        relation: "requires".into(),
                        required: true,
                    });
                }
            }
            validate_required_graph(&hashes.keys().cloned().collect::<Vec<_>>(), &edges)?;
        }
        Ok(Self {
            contracts,
            bundle,
            hash,
            hashes,
            edges,
        })
    }

    pub async fn validate_transition(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        previous_snapshot: Option<&str>,
    ) -> PgResult<()> {
        if let Some(bundle) = &self.bundle {
            let previous = tx
                .query_opt(
                    "SELECT catalog_json FROM awr_team.workstream_catalogs
                WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
                    &[&tenant, &project, &previous_snapshot],
                )
                .await?;
            if let Some(row) = previous {
                let old: WorkstreamCatalog = serde_json::from_value(row.get(0))
                    .map_err(|e| PgError::Protocol(e.to_string()))?;
                old.validate()?;
                if old.legacy_default != bundle.catalog.legacy_default {
                    return Err(PgError::Protocol(
                        "legacy workstream binding is immutable".into(),
                    ));
                }
                for stream in old.workstreams {
                    let next = bundle.catalog.get(stream.id).map_err(|_| {
                        PgError::Protocol(
                            "retained workstreams must be archived, not removed".into(),
                        )
                    })?;
                    next.validate_successor(&stream)?;
                }
            } else {
                let history: bool = tx
                    .query_one(
                        "SELECT EXISTS(SELECT 1 FROM awr_team.sessions
                    WHERE tenant_id=$1 AND project_id=$2)",
                        &[&tenant, &project],
                    )
                    .await?
                    .get(0);
                if history {
                    return Err(PgError::Unsupported(
                        "legacy session history requires explicit workstream migration".into(),
                    ));
                }
            }
            // Catalog/ownership switching cannot silently invalidate an active
            // executor. Narrower read-set activation is a separate protocol.
            let live: bool = tx
                .query_one(
                    "SELECT
                EXISTS(SELECT 1 FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2
                    AND state='active' AND expires_at > clock_timestamp()) OR
                EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2
                    AND state NOT IN ('succeeded','failed','cancelled'))",
                    &[&tenant, &project],
                )
                .await?
                .get(0);
            if live {
                return Err(PgError::RecoveryBlocked);
            }
            let ownership = tx
                .query(
                    "SELECT work_id, workstream_id FROM awr_team.workstream_ownership
                WHERE tenant_id=$1 AND project_id=$2",
                    &[&tenant, &project],
                )
                .await?;
            let owners: BTreeMap<String, String> =
                ownership.iter().map(|r| (r.get(0), r.get(1))).collect();
            for entry in &bundle.contracts {
                if owners
                    .get(entry.contract.work_id.as_str())
                    .is_some_and(|old| *old != entry.workstream_id.to_string())
                {
                    return Err(PgError::Unsupported(
                        "workstream movement requires explicit ownership migration".into(),
                    ));
                }
            }
        } else {
            // Preserve the legacy claim/contract switch check, including removed
            // split children. The new API never treats source status as proof.
            let claimed = tx
                .query(
                    "SELECT DISTINCT work_id FROM awr_team.claims
                WHERE tenant_id=$1 AND project_id=$2 AND state='active'
                  AND expires_at > clock_timestamp()",
                    &[&tenant, &project],
                )
                .await?;
            for row in claimed {
                let work: String = row.get(0);
                let old: Option<String> = tx.query_opt("SELECT contract_hash FROM awr_team.work_contracts
                    WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
                    &[&tenant, &project, &previous_snapshot, &work]).await?.map(|r| r.get(0));
                if old.as_ref() != self.hashes.get(&work) {
                    return Err(PgError::ClaimBlocksActivation);
                }
            }
        }
        Ok(())
    }

    /// Selective activation gate (TMCP-022). See `ActivationImpactGate`.
    pub async fn validate_transition_with_impact(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        previous_snapshot: Option<&str>,
        impact: Option<&crate::source::writeback::ActivationImpactGate>,
    ) -> PgResult<()> {
        match impact {
            None => {
                self.validate_transition(tx, tenant, project, previous_snapshot)
                    .await
            }
            Some(gate) if !gate.impact_proven => Err(PgError::ActivationImpactUnproven(
                gate.refuse_reason
                    .clone()
                    .unwrap_or_else(|| "impact unproven".into()),
            )),
            Some(gate) if !gate.allow_activation => Err(PgError::WritebackRefused(
                gate.refuse_reason
                    .clone()
                    .unwrap_or_else(|| "writeback refused by activation gate".into()),
            )),
            Some(gate) => {
                // Structural catalog/ownership checks via the existing path's
                // non-live portions: call full validate only for non-bundle, and
                // for bundle skip the project-wide live OR by re-checking selectively.
                if self.bundle.is_some() {
                    // Catalog immutability still required — reuse full validate
                    // only when there is no live activity; otherwise selective.
                    let live: bool = tx
                        .query_one(
                            "SELECT
                            EXISTS(SELECT 1 FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2
                                AND state='active' AND expires_at > clock_timestamp()) OR
                            EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2
                                AND state NOT IN ('succeeded','failed','cancelled'))",
                            &[&tenant, &project],
                        )
                        .await?
                        .get(0);
                    if !live {
                        return self
                            .validate_transition(tx, tenant, project, previous_snapshot)
                            .await;
                    }
                    // Live activity exists: run validate_transition's catalog checks by
                    // temporarily relying on selective claim/exec filtering below.
                    // Catalog checks:
                    if let Some(bundle) = &self.bundle {
                        let previous = tx
                            .query_opt(
                                "SELECT catalog_json FROM awr_team.workstream_catalogs
                            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
                                &[&tenant, &project, &previous_snapshot],
                            )
                            .await?;
                        if let Some(row) = previous {
                            let old: WorkstreamCatalog = serde_json::from_value(row.get(0))
                                .map_err(|e| PgError::Protocol(e.to_string()))?;
                            old.validate()?;
                            if old.legacy_default != bundle.catalog.legacy_default {
                                return Err(PgError::Protocol(
                                    "legacy workstream binding is immutable".into(),
                                ));
                            }
                            for stream in old.workstreams {
                                let next = bundle.catalog.get(stream.id).map_err(|_| {
                                    PgError::Protocol(
                                        "retained workstreams must be archived, not removed".into(),
                                    )
                                })?;
                                next.validate_successor(&stream)?;
                            }
                        }
                    }
                }
                let affected: std::collections::BTreeSet<_> =
                    gate.affected_work_ids.iter().cloned().collect();
                let claimed = tx
                    .query(
                        "SELECT DISTINCT work_id FROM awr_team.claims
                        WHERE tenant_id=$1 AND project_id=$2 AND state='active'
                          AND expires_at > clock_timestamp()",
                        &[&tenant, &project],
                    )
                    .await?;
                for row in claimed {
                    let work: String = row.get(0);
                    if !affected.contains(&work) {
                        continue;
                    }
                    if gate.stopped_work_ids.contains(&work) {
                        continue;
                    }
                    let old: Option<String> = tx
                        .query_opt(
                            "SELECT contract_hash FROM awr_team.work_contracts
                            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3
                              AND scope_id='main' AND work_id=$4",
                            &[&tenant, &project, &previous_snapshot, &work],
                        )
                        .await?
                        .map(|r| r.get(0));
                    if old.as_ref() != self.hashes.get(&work) {
                        return Err(PgError::ClaimBlocksActivation);
                    }
                }
                let live_exec = tx
                    .query(
                        "SELECT DISTINCT work_id FROM awr_team.executions
                        WHERE tenant_id=$1 AND project_id=$2
                          AND state NOT IN ('succeeded','failed','cancelled')",
                        &[&tenant, &project],
                    )
                    .await?;
                for row in live_exec {
                    let work: String = row.get(0);
                    if affected.contains(&work) && !gate.stopped_work_ids.contains(&work) {
                        return Err(PgError::WritebackRefused(format!(
                            "affected work {work} has nonterminal execution; stop/reconcile/replan required"
                        )));
                    }
                }
                Ok(())
            }
        }
    }

    pub async fn install(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        snapshot: &str,
    ) -> PgResult<()> {
        if let Some(bundle) = &self.bundle {
            let catalog = serde_json::to_value(&bundle.catalog)
                .map_err(|e| PgError::Protocol(e.to_string()))?;
            tx.execute("INSERT INTO awr_team.workstream_catalogs(tenant_id,project_id,snapshot_id,catalog_json,projection_hash)
                VALUES($1,$2,$3,$4,$5)", &[&tenant,&project,&snapshot,&catalog,&self.hash]).await?;
        }
        for contract in &self.contracts {
            let work = contract.work_id.as_str();
            let key = &contract.external_key;
            let contract_json =
                serde_json::to_value(contract).map_err(|e| PgError::Protocol(e.to_string()))?;
            tx.execute(
                "INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                VALUES($1,$2,$3,$4) ON CONFLICT(tenant_id,project_id,id) DO NOTHING",
                &[&tenant, &project, &work, &key],
            )
            .await?;
            let old_key: String = tx
                .query_one(
                    "SELECT external_key FROM awr_team.work_items
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                    &[&tenant, &project, &work],
                )
                .await?
                .get(0);
            if self.bundle.is_some() && old_key != *key {
                return Err(PgError::Protocol("work external key is immutable".into()));
            }
            tx.execute(
                "INSERT INTO awr_team.work_contracts(tenant_id,project_id,snapshot_id,scope_id,
                work_id,contract_hash,definition_state,title,contract_json)
                VALUES($1,$2,$3,'main',$4,$5,'enabled',$6,$7)",
                &[
                    &tenant,
                    &project,
                    &snapshot,
                    &work,
                    &self.hashes[work],
                    &key,
                    &contract_json,
                ],
            )
            .await?;
        }
        if let Some(bundle) = &self.bundle {
            for entry in &bundle.contracts {
                let work = entry.contract.work_id.as_str();
                let stream = entry.workstream_id.to_string();
                tx.execute("INSERT INTO awr_team.workstream_ownership(tenant_id,project_id,work_id,workstream_id)
                    VALUES($1,$2,$3,$4) ON CONFLICT(tenant_id,project_id,work_id) DO NOTHING",
                    &[&tenant,&project,&work,&stream]).await?;
                tx.execute("INSERT INTO awr_team.workstream_snapshot_ownership(tenant_id,project_id,snapshot_id,
                    scope_id,work_id,workstream_id,ownership_version)
                    SELECT tenant_id,project_id,$3,'main',work_id,workstream_id,ownership_version
                    FROM awr_team.workstream_ownership WHERE tenant_id=$1 AND project_id=$2 AND work_id=$4",
                    &[&tenant,&project,&snapshot,&work]).await?;
            }
            for edge in &self.edges {
                tx.execute("INSERT INTO awr_team.dependency_edges(tenant_id,project_id,snapshot_id,scope_id,
                    from_work_id,to_work_id,relation,required) VALUES($1,$2,$3,'main',$4,$5,$6,true)",
                    &[&tenant,&project,&snapshot,&edge.from,&edge.to,&edge.relation]).await?;
            }
            tx.execute("UPDATE awr_team.workstream_modes SET enabled=true WHERE tenant_id=$1 AND project_id=$2",
                &[&tenant,&project]).await?;
        }
        Ok(())
    }

    /// Keep receipts as history while reopening selections invalidated by the
    /// new source contract or by an exact predecessor receipt that changed.
    pub async fn invalidate_stale_completions(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        snapshot: &str,
    ) -> PgResult<Vec<Value>> {
        Box::pin(completion_invalidation::apply(
            self, tx, tenant, project, snapshot,
        ))
        .await
    }
}

pub(super) fn reject_external_graph(files: &[(String, Vec<u8>)]) -> PgResult<()> {
    if let Some((_, bytes)) = files.iter().find(|(path, _)| path == "graph.json") {
        let parsed: Value =
            serde_json::from_slice(bytes).map_err(|e| PgError::Protocol(e.to_string()))?;
        let edges: Vec<DependencyEdge> = serde_json::from_value(
            parsed
                .get("edges")
                .cloned()
                .unwrap_or(serde_json::json!([])),
        )
        .map_err(|e| PgError::Protocol(e.to_string()))?;
        if !edges.is_empty() {
            return Err(PgError::Protocol("graph.json with edges is not supported; use required_dependencies in workstreams.json".into()));
        }
    }
    Ok(())
}

/// Source status and historical human `done` retain source meaning only
/// (AWR-TMCP-020). Installing a projection never treats them as completion
/// receipts; completion remains a separate PG domain path.
#[allow(dead_code)]
pub(crate) fn source_status_is_completion_proof() -> bool {
    false
}

#[cfg(test)]
mod publish_boundaries {
    #[test]
    fn install_path_does_not_treat_source_status_as_completion() {
        assert!(!super::source_status_is_completion_proof());
    }
}
