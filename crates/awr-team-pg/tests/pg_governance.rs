#![cfg(feature = "pg-tests")]
//! Admission of governed work in a Team: a snapshot declares governance, a decision is attributed
//! to the verified decider, and claims and execution intents hold until the decision is current.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::{
    AdmissionPolicy, DefinitionSummary, Id, WorkDefinition, Workstream, WorkstreamCatalog,
    WorkstreamState,
};
use awr_team::{
    GOVERNANCE_FILE, GOVERNANCE_VERSION, GovernanceCandidate, SourceActivationPlan, WorkContract,
    WorkGovernance, WorkId, WorkstreamBundle, WorkstreamContract,
};
use awr_team_pg::{
    IngestRequest, PgError, SourceFile, SourceStore, WorkstreamCommand, WorkstreamReadStore,
};
use fixture::*;
use serde_json::{Value, json};

const PIN: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn definition() -> WorkDefinition {
    serde_json::from_value(json!({
        "version": 1, "purpose": "research", "tier": "required",
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": PIN}],
        "contributes": [{"goal": "ship", "outcome": "O", "claim": "Maps the failure modes", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One answer", "by": "agent-a"},
        "deliverable": "A list of failure modes"
    }))
    .unwrap()
}

fn complete() -> DefinitionSummary {
    DefinitionSummary {
        declared: true,
        intent_adopted: true,
        gaps: vec![],
    }
}

fn candidate(mode: &str, summary: DefinitionSummary) -> GovernanceCandidate {
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

fn files(candidate: &GovernanceCandidate) -> Vec<SourceFile> {
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

fn judgment() -> Value {
    json!({"contribution": "Advances the outcome", "necessity": "The failure modes are unknown",
           "alternatives": "Existing reports were checked", "deliverable": "A list of failure modes",
           "exit": "Stop when the list is reviewed", "responsibility": "research-owner"})
}

async fn claim(store: &WorkstreamReadStore, request: &str) -> WorkstreamCommand {
    let p = prepare(store, A, "a").await;
    command(
        &p,
        request,
        "claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1",
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":60}),
    )
}

/// A decision states the history it read; unless the test says otherwise it read the current one.
async fn decision(
    store: &WorkstreamReadStore,
    request: &str,
    mut args: Value,
) -> WorkstreamCommand {
    if args.get("expected_admission_version").is_none() {
        args["expected_admission_version"] = read(store).await["admission_version"].clone();
    }
    command(
        &prepare(store, A, "a").await,
        request,
        "admission.decide",
        args,
    )
}

async fn read(store: &WorkstreamReadStore) -> Value {
    let mut q = query("work.admission");
    q.work_id = Some("a".into());
    store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone()
}

fn refused_with(result: &Result<Value, PgError>, expected: &str) {
    match result {
        Err(PgError::AdmissionRequired { code, .. }) => assert_eq!(*code, expected),
        other => panic!("expected AdmissionRequired({expected}), got {other:?}"),
    }
}

#[tokio::test]
async fn a_governed_work_is_held_until_a_decider_decides_for_the_scope_it_read() {
    let (_guard, admin, _, store) =
        setup_with_specs(files(&candidate("governed_enforce", complete()))).await;
    enable_writes(&admin).await;
    let commands = store.commands();
    // Reading says why the work is held and which scope a decision will be checked against.
    let before = read(&store).await;
    assert_eq!(before["governed"], true);
    assert_eq!(before["enforced"], true);
    assert_eq!(before["state"], "missing");
    assert_eq!(before["diagnostics"][0]["code"], "admission_missing");
    let fingerprint = before["binding_fingerprint"].as_str().unwrap().to_string();
    let first = commands
        .execute(TENANT, PROJECT, A, claim(&store, "take-1").await)
        .await;
    refused_with(&first, "admission_missing");
    // A decision on a scope the decider did not read is refused and records nothing.
    let stale = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &store,
                "decide-stale",
                json!({"admission_decision":"approved","binding_fingerprint":"0".repeat(64),
                       "admission_judgment":judgment()}),
            )
            .await,
        )
        .await;
    assert!(
        matches!(stale, Err(PgError::PreconditionsChanged)),
        "{stale:?}"
    );
    let rows: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.work_admissions", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 0);
    // The decision of the verified caller admits the work, once, and replays as one receipt.
    let request = decision(
        &store,
        "decide-1",
        json!({"admission_decision":"approved","binding_fingerprint":fingerprint,
               "admission_judgment":judgment()}),
    )
    .await;
    let decided = commands
        .execute(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    let data = &decided["receipt"]["data"];
    assert_eq!(data["state"], "approved");
    assert_eq!(data["admitted"], true);
    assert_eq!(data["decider"]["actor_id"], "agent");
    assert_eq!(data["decider"]["identity"], "verified");
    assert_eq!(
        commands.execute(TENANT, PROJECT, A, request).await.unwrap()["receipt"],
        decided["receipt"]
    );
    let rows: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.work_admissions", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 1);
    assert_eq!(read(&store).await["admitted"], true);
    // The work can now be claimed.
    let taken = commands
        .execute(TENANT, PROJECT, A, claim(&store, "take-2").await)
        .await
        .unwrap();
    assert_eq!(taken["receipt"]["data"]["fence"], "1");
}

#[tokio::test]
async fn dispatch_waits_for_the_same_current_decision_as_a_self_claim() {
    let (_guard, admin, _, store) =
        setup_with_specs(files(&candidate("governed_enforce", complete()))).await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET assignment_grant=true WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let commands = store.commands();
    let assign = |request: &str, prepared: &Value| {
        command(
            prepared,
            request,
            "task.assign",
            json!({"assignee_person_id":"nobody",
                   "expected_responsibility_version":
                       prepared["data"]["responsibility"]["version"].as_str().unwrap_or("0")}),
        )
    };
    // Without a decision the work is not dispatched, whoever it is dispatched to.
    let prepared = prepare(&store, A, "a").await;
    let refused = commands
        .execute(TENANT, PROJECT, A, assign("assign-1", &prepared))
        .await;
    refused_with(&refused, "admission_missing");
    // With a current decision the gate lets go and the ordinary checks of dispatch answer instead.
    let fingerprint = read(&store).await["binding_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    commands
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &store,
                "decide-dispatch",
                json!({"admission_decision":"approved","binding_fingerprint":fingerprint,
                       "admission_judgment":judgment()}),
            )
            .await,
        )
        .await
        .unwrap();
    let prepared = prepare(&store, A, "a").await;
    let result = commands
        .execute(TENANT, PROJECT, A, assign("assign-2", &prepared))
        .await;
    assert!(
        !matches!(result, Err(PgError::AdmissionRequired { .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_denial_holds_the_work_and_names_its_reason() {
    let (_guard, admin, _, store) =
        setup_with_specs(files(&candidate("governed_enforce", complete()))).await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let fingerprint = read(&store).await["binding_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    // A denial needs a reason and states no judgment.
    let invalid = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &store,
                "deny-no-reason",
                json!({"admission_decision":"denied","binding_fingerprint":fingerprint}),
            )
            .await,
        )
        .await;
    assert!(invalid.is_err(), "{invalid:?}");
    let denied = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &store,
                "deny-1",
                json!({"admission_decision":"denied","binding_fingerprint":fingerprint,
                       "admission_reason":"The examples restate the acceptance"}),
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(denied["receipt"]["data"]["state"], "denied");
    let refused = commands
        .execute(TENANT, PROJECT, A, claim(&store, "take-denied").await)
        .await;
    refused_with(&refused, "admission_denied");
    match refused {
        Err(PgError::AdmissionRequired { detail, .. }) => {
            assert!(detail.contains("restate the acceptance"), "{detail}")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_approval_does_not_cover_a_definition_that_leaves_obligations_unmet() {
    let gaps = DefinitionSummary {
        declared: true,
        intent_adopted: false,
        gaps: vec!["intent_missing".into()],
    };
    let (_guard, admin, _, store) =
        setup_with_specs(files(&candidate("governed_enforce", gaps))).await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let fingerprint = read(&store).await["binding_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    commands
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &store,
                "approve-anyway",
                json!({"admission_decision":"approved","binding_fingerprint":fingerprint,
                       "admission_judgment":judgment()}),
            )
            .await,
        )
        .await
        .unwrap();
    let refused = commands
        .execute(TENANT, PROJECT, A, claim(&store, "take-gaps").await)
        .await;
    refused_with(&refused, "admission_obligations_unmet");
}

#[tokio::test]
async fn observing_reports_without_holding_and_a_snapshot_without_governance_is_untouched() {
    let (_guard, admin, _, store) =
        setup_with_specs(files(&candidate("legacy_observe", complete()))).await;
    enable_writes(&admin).await;
    let observed = read(&store).await;
    assert_eq!(observed["governed"], true);
    assert_eq!(observed["enforced"], false);
    assert_eq!(observed["diagnostics"][0]["code"], "admission_missing");
    let taken = store
        .commands()
        .execute(TENANT, PROJECT, A, claim(&store, "take-observed").await)
        .await
        .unwrap();
    assert_eq!(taken["receipt"]["data"]["fence"], "1");
    // Without governance there is nothing to decide, and a decision is refused.
    let (_guard2, admin2, _, plain) = {
        drop(_guard);
        setup().await
    };
    enable_writes(&admin2).await;
    let mut q = query("work.admission");
    q.work_id = Some("a".into());
    let none = plain.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(none["data"]["governed"], false);
    let refused = plain
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &plain,
                "no-governance",
                json!({"admission_decision":"approved","binding_fingerprint":"0".repeat(64),
                       "admission_judgment":judgment()}),
            )
            .await,
        )
        .await;
    assert!(
        matches!(refused, Err(PgError::Unsupported(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn decisions_are_append_only_and_a_snapshot_never_writes_one() {
    let (_guard, admin, _, store) =
        setup_with_specs(files(&candidate("governed_enforce", complete()))).await;
    enable_writes(&admin).await;
    // Installing the snapshot recorded what it declares and no decision.
    let declared: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.work_governance", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(declared, 1);
    let decisions: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.work_admissions", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(decisions, 0);
    let fingerprint = read(&store).await["binding_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &store,
                "decide-append",
                json!({"admission_decision":"approved","binding_fingerprint":fingerprint,
                       "admission_judgment":judgment()}),
            )
            .await,
        )
        .await
        .unwrap();
    assert!(
        admin
            .batch_execute("UPDATE awr_team.work_admissions SET reason='rewritten'")
            .await
            .is_err()
    );
    assert!(
        admin
            .batch_execute("DELETE FROM awr_team.work_admissions")
            .await
            .is_err()
    );
}

/// The same snapshot the fixture installs, published again with or without governance.
async fn publish(db: &str, governance: Option<&GovernanceCandidate>) -> Result<(), PgError> {
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

#[tokio::test]
async fn a_source_change_makes_the_decision_stale_and_a_package_cannot_drop_governance() {
    let governed = candidate("governed_enforce", complete());
    let (_guard, admin, db, store) = setup_with_specs(files(&governed)).await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let fingerprint = read(&store).await["binding_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    commands
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &store,
                "decide-before-change",
                json!({"admission_decision":"approved","binding_fingerprint":fingerprint,
                       "admission_judgment":judgment()}),
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(read(&store).await["admitted"], true);
    // A package without governance cannot replace a governed snapshot.
    let dropped = publish(&db, None).await.unwrap_err();
    assert!(
        matches!(&dropped, PgError::Protocol(message) if message.contains("governance")),
        "{dropped:?}"
    );
    assert_eq!(read(&store).await["admitted"], true, "nothing changed");
    // The definition of the work changes: the new snapshot installs and the old decision is stale.
    let mut changed = governed.clone();
    changed.works[0].definition = Some(
        serde_json::from_value(json!({
            "version": 1, "purpose": "research", "tier": "required",
            "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": PIN}],
            "contributes": [{"goal": "ship", "outcome": "O", "claim": "Maps the failure modes", "by": "agent-a"}],
            "granularity": {"judgment": "independent_result", "reason": "One answer", "by": "agent-a"},
            "deliverable": "A different list of failure modes"
        }))
        .unwrap(),
    );
    publish(&db, Some(&changed)).await.unwrap();
    let after = read(&store).await;
    assert_eq!(after["state"], "stale", "{after}");
    assert_eq!(after["admitted"], false);
    assert!(
        after["diagnostics"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("definition"),
        "{after}"
    );
    let refused = commands
        .execute(TENANT, PROJECT, A, claim(&store, "take-after-change").await)
        .await;
    refused_with(&refused, "admission_stale");
}

#[tokio::test]
async fn two_deciders_who_read_the_same_revision_get_one_decision() {
    let (_guard, admin, _, store) =
        setup_with_specs(files(&candidate("governed_enforce", complete()))).await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let seen = read(&store).await;
    let fingerprint = seen["binding_fingerprint"].as_str().unwrap().to_string();
    assert_eq!(seen["admission_version"], "0");
    let prepared = prepare(&store, A, "a").await;
    let one = command(
        &prepared,
        "decide-one",
        "admission.decide",
        json!({"admission_decision":"approved","binding_fingerprint":fingerprint,
               "expected_admission_version":"0","admission_judgment":judgment()}),
    );
    let other = command(
        &prepared,
        "decide-other",
        "admission.decide",
        json!({"admission_decision":"denied","binding_fingerprint":fingerprint,
               "expected_admission_version":"0",
               "admission_reason":"Another decider reads the scope differently"}),
    );
    commands.execute(TENANT, PROJECT, A, one).await.unwrap();
    // The second decider read a history that has grown since: nothing is recorded for it.
    let lost = commands.execute(TENANT, PROJECT, A, other).await;
    assert!(
        matches!(lost, Err(PgError::PreconditionsChanged)),
        "{lost:?}"
    );
    let rows: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.work_admissions", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 1);
    let after = read(&store).await;
    assert_eq!(after["state"], "approved");
    assert_eq!(after["admission_version"], "1");
    // Reading again, the same decider decides on what the first one decided.
    let again = decision(
        &store,
        "decide-after-reading",
        json!({"admission_decision":"denied","binding_fingerprint":fingerprint,
               "admission_reason":"Another decider reads the scope differently"}),
    )
    .await;
    commands.execute(TENANT, PROJECT, A, again).await.unwrap();
    let rows: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.work_admissions", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 2);
    let last = read(&store).await;
    assert_eq!(last["state"], "denied");
    assert_eq!(last["admission_version"], "2");
    // A decision cannot skip ahead of the history either.
    let skipped = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            decision(
                &store,
                "decide-skipping",
                json!({"admission_decision":"approved","binding_fingerprint":fingerprint,
                       "expected_admission_version":"3","admission_judgment":judgment()}),
            )
            .await,
        )
        .await;
    assert!(
        matches!(skipped, Err(PgError::PreconditionsChanged)),
        "{skipped:?}"
    );
}
