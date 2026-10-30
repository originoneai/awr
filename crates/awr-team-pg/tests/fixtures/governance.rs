#![allow(dead_code)]
//! Governed snapshots for the Team PostgreSQL tests: the declaration a package carries, and the
//! same snapshot published again with or without it.
use super::fixture::*;
use crate::common;
use awr_core::{
    AdmissionPolicy, DefinitionSummary, Id, WorkDefinition, Workstream, WorkstreamCatalog,
    WorkstreamState,
};
use awr_team::{
    GOVERNANCE_FILE, GOVERNANCE_VERSION, GovernanceCandidate, SourceActivationPlan, WorkContract,
    WorkGovernance, WorkId, WorkstreamBundle, WorkstreamContract,
};
use awr_team_pg::{IngestRequest, PgError, SourceFile, SourceStore};
use serde_json::{Value, json};

pub const PIN: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

pub fn definition() -> WorkDefinition {
    serde_json::from_value(json!({
        "version": 1, "purpose": "research", "tier": "required",
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": PIN}],
        "contributes": [{"goal": "ship", "outcome": "O", "claim": "Maps the failure modes", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One answer", "by": "agent-a"},
        "deliverable": "A list of failure modes"
    }))
    .unwrap()
}

pub fn complete() -> DefinitionSummary {
    DefinitionSummary {
        declared: true,
        intent_adopted: true,
        gaps: vec![],
    }
}

pub fn candidate(mode: &str, summary: DefinitionSummary) -> GovernanceCandidate {
    GovernanceCandidate {
        version: GOVERNANCE_VERSION,
        policy: serde_json::from_value::<AdmissionPolicy>(
            json!({"version": 1, "mode": mode, "scope": "all"}),
        )
        .unwrap(),
        works: vec![WorkGovernance {
            external_key: "a".into(),
            tags: vec![],
            definition: Some(definition()),
            summary,
        }],
    }
}

pub fn files(candidate: &GovernanceCandidate) -> Vec<SourceFile> {
    vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"Public API contract".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"Private peer contract".to_vec(),
        },
        SourceFile {
            path: GOVERNANCE_FILE.into(),
            bytes: serde_json::to_vec(candidate).unwrap(),
        },
    ]
}

pub fn judgment() -> Value {
    json!({"contribution": "Advances the outcome", "necessity": "The failure modes are unknown",
           "alternatives": "Existing reports were checked", "deliverable": "A list of failure modes",
           "exit": "Stop when the list is reviewed", "responsibility": "research-owner"})
}

/// The same snapshot the fixture installs, published again with or without governance.
pub async fn publish(db: &str, governance: Option<&GovernanceCandidate>) -> Result<(), PgError> {
    let definitions = [(1, "alpha"), (2, "private-beta")]
        .into_iter()
        .map(|(i, key)| Workstream {
            id: Id::from(i),
            project_id: PROJECT.into(),
            external_key: key.into(),
            title: key.into(),
            state: WorkstreamState::Active,
            authority_version: 1,
            goal_keys: vec![key.into()],
            acceptance_contracts: vec![format!("docs/{key}.md")],
        })
        .collect();
    let contracts = [
        ("a", 1, vec![]),
        ("b-private", 2, vec![]),
        ("c", 1, vec!["b-private"]),
    ]
    .into_iter()
    .map(|(id, stream, deps)| WorkstreamContract {
        workstream_id: Id::from(stream),
        contract: WorkContract {
            dependency_acceptance: Default::default(),
            execution_settlement: None,
            codec: WorkContract::CODEC.into(),
            work_id: WorkId::new(id).unwrap(),
            external_key: id.into(),
            goals: vec!["ship".into()],
            hard_rules: vec!["preserve compatibility".into()],
            scope_paths: vec!["src".into()],
            acceptance: vec!["verified".into()],
            required_dependencies: deps.into_iter().map(Into::into).collect(),
            completion_policy: "review".into(),
            verification_requirements: vec!["report".into()],
        },
    })
    .collect();
    let bundle = WorkstreamBundle {
        codec: WorkstreamBundle::CODEC.into(),
        catalog: WorkstreamCatalog {
            version: 1,
            project_id: PROJECT.into(),
            legacy_default: None,
            workstreams: definitions,
        },
        contracts,
    };
    let mut files = vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"Public API contract".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"Private peer contract".to_vec(),
        },
        SourceFile {
            path: "workstreams.json".into(),
            bytes: serde_json::to_vec(&bundle).unwrap(),
        },
    ];
    if let Some(governance) = governance {
        files.push(SourceFile {
            path: GOVERNANCE_FILE.into(),
            bytes: serde_json::to_vec(governance).unwrap(),
        });
    }
    let source = SourceStore::from_config(common::with_app_role(&common::test_config(), db));
    let c = source
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: "workstreams/1".into(),
            files,
        })
        .await?;
    source
        .approve(
            TENANT,
            PROJECT,
            &c.proposal_id,
            "reviewer",
            &c.manifest_digest,
        )
        .await?;
    source
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &c.proposal_id,
            &SourceActivationPlan {
                candidate_digest: c.manifest_digest.clone(),
                parser_version: c.parser_version.clone(),
                expected_authority_epoch: c.base_epoch.clone(),
                approved_candidate_digest: c.manifest_digest.clone(),
            },
        )
        .await?;
    Ok(())
}
