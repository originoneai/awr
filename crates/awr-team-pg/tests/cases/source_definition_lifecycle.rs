//! Source definition admission on isolated PostgreSQL fixtures.
use super::*;
use awr_team::{DraftDefinitionState, TASK_DEFINITIONS_FILE, TaskDefinition, TaskDefinitions};
use serde_json::json;

fn definitions(value: &WorkstreamBundle) -> TaskDefinitions {
    TaskDefinitions {
        works: value
            .contracts
            .iter()
            .map(|entry| {
                (
                    entry.contract.work_id.as_str().into(),
                    TaskDefinition {
                        title: entry.contract.external_key.clone(),
                        definition_state: DraftDefinitionState::Enabled,
                    },
                )
            })
            .collect(),
    }
}

fn with_definitions(value: &WorkstreamBundle, definitions: &TaskDefinitions) -> IngestRequest {
    let mut request = package(value);
    request.files.push(SourceFile {
        path: TASK_DEFINITIONS_FILE.into(),
        bytes: serde_json::to_vec(definitions).unwrap(),
    });
    request
}

#[tokio::test]
async fn metadata_rejects_missing_extra_invalid_and_completion_definitions_before_ingest() {
    let (_g, admin, _, store) = setup().await;
    let value = bundle();
    let valid = serde_json::to_value(definitions(&value)).unwrap();
    let mut missing = valid.clone();
    missing["works"].as_object_mut().unwrap().remove("sdk");
    let mut extra = valid.clone();
    extra["works"]["unknown"] = valid["works"]["sdk"].clone();
    let mut empty_title = valid.clone();
    empty_title["works"]["sdk"]["title"] = json!(" ");
    let mut nul_title = valid.clone();
    nul_title["works"]["sdk"]["title"] = json!("bad\0title");
    let mut completed = valid.clone();
    completed["works"]["sdk"]["definition_state"] = json!("completed");
    let mut runtime = valid.clone();
    runtime["works"]["sdk"]["execution_id"] = json!("untrusted");
    for invalid in [missing, extra, empty_title, nul_title, completed, runtime] {
        let mut request = package(&value);
        request.files.push(SourceFile {
            path: TASK_DEFINITIONS_FILE.into(),
            bytes: serde_json::to_vec(&invalid).unwrap(),
        });
        assert!(matches!(
            store.ingest(request).await,
            Err(PgError::Protocol(_))
        ));
    }
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.source_snapshots", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn direct_definition_changes_cannot_hide_a_running_transitive_consumer() {
    let (_g, admin, _, store) = setup().await;
    let value = bundle();
    let mut metadata = definitions(&value);
    let old = approved(&store, with_definitions(&value, &metadata)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &old.proposal_id, &plan(&old))
        .await
        .unwrap();
    super::source_activation_impact::fixture_execution(&admin, "integration", "running").await;
    for state in [
        DraftDefinitionState::Draft,
        DraftDefinitionState::Archived,
        DraftDefinitionState::Cancelled,
    ] {
        metadata
            .works
            .get_mut("interface")
            .unwrap()
            .definition_state = state;
        let candidate = approved(&store, with_definitions(&value, &metadata)).await;
        assert!(matches!(
            store
                .activate_workstreams(
                    TENANT,
                    PROJECT,
                    "author",
                    &candidate.proposal_id,
                    &plan(&candidate)
                )
                .await,
            Err(PgError::RecoveryBlocked)
        ));
        assert_eq!(
            current_snapshot(&admin).await,
            Some(old.snapshot_id.clone())
        );
    }
}

#[tokio::test]
async fn title_metadata_can_be_introduced_while_a_transitive_consumer_is_running() {
    let (_g, admin, _, store) = setup().await;
    let value = bundle();
    let old = approved(&store, package(&value)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &old.proposal_id, &plan(&old))
        .await
        .unwrap();
    super::source_activation_impact::fixture_execution(&admin, "integration", "running").await;
    let mut metadata = definitions(&value);
    metadata.works.get_mut("interface").unwrap().title = "Human interface title".into();
    let candidate = approved(&store, with_definitions(&value, &metadata)).await;
    store
        .activate_workstreams(
            TENANT,
            PROJECT,
            "author",
            &candidate.proposal_id,
            &plan(&candidate),
        )
        .await
        .unwrap();
    let title: String = admin.query_one(
        "SELECT title FROM awr_team.work_contracts WHERE snapshot_id=$1 AND work_id='interface'",
        &[&candidate.snapshot_id],
    ).await.unwrap().get(0);
    assert_eq!(title, "Human interface title");
    let state: String = admin
        .query_one(
            "SELECT state FROM awr_team.executions WHERE work_id='integration'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(state, "running");
}

#[tokio::test]
async fn source_definition_metadata_is_atomic_and_cannot_be_dropped() {
    let (_g, admin, _, store) = setup().await;
    let mut value = bundle();
    let mut metadata = definitions(&value);
    metadata
        .works
        .get_mut("interface")
        .unwrap()
        .definition_state = DraftDefinitionState::Draft;
    metadata.works.get_mut("sdk").unwrap().definition_state = DraftDefinitionState::Cancelled;
    metadata.works.get_mut("sdk").unwrap().title = "Cancelled SDK".into();
    let old = approved(&store, with_definitions(&value, &metadata)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &old.proposal_id, &plan(&old))
        .await
        .unwrap();
    let stripped = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams(
                TENANT,
                PROJECT,
                "author",
                &stripped.proposal_id,
                &plan(&stripped)
            )
            .await,
        Err(PgError::Protocol(_))
    ));
    assert_eq!(current_snapshot(&admin).await, Some(old.snapshot_id));
    value.contracts[2]
        .contract
        .acceptance
        .push("unrelated contract update".into());
    let candidate = approved(&store, with_definitions(&value, &metadata)).await;
    store
        .activate_workstreams(
            TENANT,
            PROJECT,
            "author",
            &candidate.proposal_id,
            &plan(&candidate),
        )
        .await
        .unwrap();
    let rows = admin.query("SELECT work_id,title,definition_state FROM awr_team.work_contracts WHERE snapshot_id=$1 ORDER BY work_id", &[&candidate.snapshot_id]).await.unwrap();
    for row in rows {
        let work: String = row.get(0);
        assert_eq!(row.get::<_, String>(1), metadata.works[&work].title);
        assert_eq!(
            row.get::<_, String>(2),
            metadata.works[&work].definition_state.as_str()
        );
    }
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn legacy_raw_packages_retain_prior_definitions_when_metadata_was_never_present() {
    let (_g, admin, _, store) = setup().await;
    let mut value = bundle();
    let old = approved(&store, package(&value)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &old.proposal_id, &plan(&old))
        .await
        .unwrap();
    admin.execute("UPDATE awr_team.work_contracts SET title='Retired SDK',definition_state='archived' WHERE work_id='sdk'", &[]).await.unwrap();
    value.contracts[2]
        .contract
        .acceptance
        .push("unrelated contract update".into());
    let candidate = approved(&store, package(&value)).await;
    store
        .activate_workstreams(
            TENANT,
            PROJECT,
            "author",
            &candidate.proposal_id,
            &plan(&candidate),
        )
        .await
        .unwrap();
    let row = admin.query_one("SELECT title,definition_state FROM awr_team.work_contracts WHERE snapshot_id=$1 AND work_id='sdk'", &[&candidate.snapshot_id]).await.unwrap();
    assert_eq!(row.get::<_, String>(0), "Retired SDK");
    assert_eq!(row.get::<_, String>(1), "archived");
}
