#![cfg(feature = "pg-tests")]
//! Admission of governed work in a Team: a snapshot declares governance, a decision is attributed
//! to the verified decider, and claims and execution intents hold until the decision is current.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::{AdmissionPolicy, DefinitionSummary, WorkDefinition};
use awr_team::{GOVERNANCE_FILE, GOVERNANCE_VERSION, GovernanceCandidate, WorkGovernance};
use awr_team_pg::{PgError, SourceFile, WorkstreamCommand, WorkstreamReadStore};
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

async fn decision(store: &WorkstreamReadStore, request: &str, args: Value) -> WorkstreamCommand {
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
