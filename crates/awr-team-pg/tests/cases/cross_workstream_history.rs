// Historical-delivery mechanisms on isolated PostgreSQL, not native acceptance.
async fn fixed_export_consumer(
    admin: &Client,
    store: &WorkstreamReadStore,
    db: &str,
) -> WorkContract {
    let mut consumer = exported_consumer(admin, store, db).await;
    let awr_team::DependencyAcceptanceMode::CrossWorkstream(policy) =
        consumer.dependency_acceptance.get_mut("a").unwrap()
    else {
        unreachable!()
    };
    policy.version_policy = awr_core::DeliveryVersionPolicy::FixedDelivery;
    admin
        .execute(
            "UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='b-private'",
            &[&json!(consumer), &consumer.hash().unwrap()],
        )
        .await
        .unwrap();
    consumer
}

#[tokio::test]
async fn fixed_delivery_adoption_selects_the_exact_accepted_input() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = fixed_export_consumer(&admin, &store, &db).await;
    adoption_roles(&admin).await;
    let session = adoption_session(&store, "history-consumer-session").await;
    let export = publish_export(&store, &consumer, &upstream, "history-export").await;
    let adopted = cross_execute(
        &store,
        B,
        adoption_request(&store, &session, &export, "history-adopt").await,
    )
    .await
    .expect("An explicitly fixed historical input must be adoptable");
    assert_eq!(
        adopted["receipt"]["data"]["receipt_id"],
        upstream["receipt_id"]
    );
    assert_eq!(adopted["execution_authorized"], false);
    let p = prepare(&store, B, "b-private").await;
    assert_eq!(p["data"]["adopted_dependencies"][0]["valid"], true);
    assert_eq!(
        p["data"]["adopted_dependencies"][0]["policy"]["version_policy"],
        "fixed_delivery"
    );
    assert_eq!(
        p["data"]["adopted_dependencies"][0]["receipt_id"],
        upstream["receipt_id"]
    );
    assert_eq!(discover_exports(&store).await["items"][0]["adopted"], true);
}

#[tokio::test]
async fn fixed_delivery_adoption_never_guesses_source_for_a_legacy_export() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = fixed_export_consumer(&admin, &store, &db).await;
    adoption_roles(&admin).await;
    let session = adoption_session(&store, "legacy-history-session").await;
    let export = publish_export(&store, &consumer, &upstream, "legacy-history-export").await;
    let mut request = adoption_request(&store, &session, &export, "legacy-history-adopt").await;
    let mut manifest: Value = admin
        .query_one(
            "SELECT manifest_json FROM awr_team.workstream_artifact_exports WHERE id=$1",
            &[&export["receipt"]["data"]["export_id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    manifest["codec"] = json!("awr-approved-artifact-export-v1");
    manifest
        .as_object_mut()
        .unwrap()
        .remove("provider_source_snapshot_id");
    let disclosure = awr_team::request_hash(&manifest).unwrap();
    admin
        .execute(
            "UPDATE awr_team.workstream_artifact_exports SET manifest_json=$1,disclosure_sha256=$2",
            &[&manifest, &disclosure],
        )
        .await
        .unwrap();
    request.args["expected_disclosure_sha256"] = json!(disclosure);
    assert!(matches!(
        cross_execute(&store, B, request).await,
        Err(PgError::EvidenceInvalid)
    ));
    assert_eq!(
        discover_exports(&store).await["items"][0]["available"],
        false
    );
    assert_eq!(
        admin
            .query_one(
                "SELECT count(*) FROM awr_team.workstream_artifact_adoptions",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

async fn release_settled_history_claim(
    admin: &Client,
    store: &WorkstreamReadStore,
    token: &str,
    work: &str,
    label: &str,
) {
    // Release coordination through the ordinary command. Settled execution
    // facts remain authoritative; this never asserts that a live effect stopped.
    let claims = admin
        .query(
            "SELECT c.id,c.fence,c.lease_version,c.session_id,s.session_version
        FROM awr_team.claims c JOIN awr_team.sessions s ON s.id=c.session_id
        AND s.tenant_id=c.tenant_id AND s.project_id=c.project_id
        WHERE c.work_id=$1 AND c.state='active'",
            &[&work],
        )
        .await
        .unwrap();
    for (i, claim) in claims.iter().enumerate() {
        cross_step(store, token, work, &format!("{label}-release-{i}"), "claim.release", json!({
            "session_id":claim.get::<_,String>(3),"expected_session_version":claim.get::<_,i64>(4).to_string(),
            "claim_id":claim.get::<_,String>(0),"expected_fence":claim.get::<_,i64>(1).to_string(),
            "expected_lease_version":claim.get::<_,i64>(2).to_string()
        })).await;
    }
}

async fn history_source_bundle(admin: &Client) -> awr_team::WorkstreamBundle {
    let snapshot: String = admin
        .query_one(
            "SELECT active_snapshot_id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let catalog: awr_core::WorkstreamCatalog = serde_json::from_value(
        admin
            .query_one(
                "SELECT catalog_json FROM awr_team.workstream_catalogs WHERE snapshot_id=$1",
                &[&snapshot],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    let mut contracts: Vec<awr_team::WorkstreamContract> = admin
        .query(
            "SELECT c.contract_json,o.workstream_id
        FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
          ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=c.snapshot_id
          AND o.scope_id=c.scope_id AND o.work_id=c.work_id
        WHERE c.snapshot_id=$1 AND c.scope_id='main' ORDER BY c.work_id",
            &[&snapshot],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|r| awr_team::WorkstreamContract {
            contract: serde_json::from_value(r.get(0)).unwrap(),
            workstream_id: r.get::<_, String>(1).parse().unwrap(),
        })
        .collect();
    let owners: std::collections::BTreeMap<_, _> = contracts
        .iter()
        .map(|e| (e.contract.work_id.as_str().to_owned(), e.workstream_id))
        .collect();
    for entry in &mut contracts {
        // The original fixture has an unused legacy cross-stream edge. Make
        // its explicit policy part of the new V6 package, rather than bypass
        // the graph validator to activate an incomplete bundle.
        for upstream in entry.contract.required_dependencies.clone() {
            if owners[&upstream] != entry.workstream_id
                && !entry.contract.dependency_acceptance.contains_key(&upstream)
            {
                entry.contract.codec = WorkContract::CODEC_V6.into();
                entry.contract.dependency_acceptance.insert(upstream, awr_team::DependencyAcceptanceMode::CrossWorkstream(
                    awr_team::CrossWorkstreamDependencyPolicy {
                        review_assurance: awr_team::CrossWorkstreamReviewAssurance::SimulatedMemberIndependent,
                        version_policy: awr_core::DeliveryVersionPolicy::CurrentContract,
                    }));
            }
        }
    }
    let bundle = awr_team::WorkstreamBundle {
        codec: awr_team::WorkstreamBundle::CODEC_V6.into(),
        catalog,
        contracts,
    };
    bundle
}

async fn history_source_candidate(
    db: &str,
    bundle: &awr_team::WorkstreamBundle,
) -> awr_team_pg::CandidateRecord {
    bundle.validate(PROJECT).unwrap();
    let source =
        awr_team_pg::SourceStore::from_config(common::with_app_role(&common::test_config(), db));
    let candidate = source
        .ingest(awr_team_pg::IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: "workstreams/6".into(),
            files: vec![awr_team_pg::SourceFile {
                path: "workstreams.json".into(),
                bytes: serde_json::to_vec(bundle).unwrap(),
            }],
        })
        .await
        .unwrap();
    source
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            "runner",
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    candidate
}

async fn activate_history_candidate(
    db: &str,
    candidate: &awr_team_pg::CandidateRecord,
) -> Result<awr_team_pg::CurrentWorkstreamSource, PgError> {
    let source =
        awr_team_pg::SourceStore::from_config(common::with_app_role(&common::test_config(), db));
    source
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &candidate.proposal_id,
            &awr_team::SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                approved_candidate_digest: candidate.manifest_digest.clone(),
                parser_version: candidate.parser_version.clone(),
                expected_authority_epoch: candidate.base_epoch.clone(),
            },
        )
        .await
}

async fn advance_history_source(
    admin: &Client,
    store: &WorkstreamReadStore,
    db: &str,
    label: &str,
) -> String {
    release_settled_history_claim(admin, store, A, "a", label).await;
    let mut bundle = history_source_bundle(admin).await;
    bundle
        .contracts
        .iter_mut()
        .find(|e| e.contract.work_id.as_str() == "a")
        .unwrap()
        .contract
        .acceptance
        .push(format!("Verified provider revision {label}"));
    let candidate = history_source_candidate(db, &bundle).await;
    activate_history_candidate(db, &candidate).await.unwrap();
    candidate.snapshot_id
}

#[tokio::test]
async fn fixed_delivery_survives_source_update_while_current_input_requires_republication() {
    for fixed in [true, false] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let upstream = complete_simulated_upstream(&store).await;
        let consumer = if fixed {
            fixed_export_consumer(&admin, &store, &db).await
        } else {
            exported_consumer(&admin, &store, &db).await
        };
        adoption_roles(&admin).await;
        let session = adoption_session(&store, "source-history-session").await;
        let export = publish_export(&store, &consumer, &upstream, "source-history-export").await;
        adopt_export(&store, &session, &export, "source-history-adopt").await;
        let archive: Value = admin
            .query_one(
                "SELECT to_jsonb(e) FROM awr_team.workstream_artifact_exports e",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        advance_history_source(&admin, &store, &db, "updated-provider").await;
        assert_eq!(
            admin
                .query_one(
                    "SELECT to_jsonb(e) FROM awr_team.workstream_artifact_exports e",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, Value>(0),
            archive
        );
        assert!(
            admin
                .query_one(
                    "SELECT selected_completion_id FROM awr_team.work_runtime WHERE work_id='a'",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, Option<String>>(0)
                .is_none()
        );
        let p = prepare(&store, B, "b-private").await;
        assert_eq!(p["data"]["adopted_dependencies"][0]["valid"], fixed);
        assert_eq!(
            discover_exports(&store).await["items"][0]["available"],
            fixed
        );
        if fixed {
            assert_eq!(
                store
                    .query(TENANT, PROJECT, B, export_content(&export))
                    .await
                    .unwrap()["data"]["text"],
                "simulated artifact"
            );
            assert_eq!(
                claim_cross_consumer(&store, &session, "claim-fixed-source-input")
                    .await
                    .unwrap()["receipt"]["data"]["coordination_claim_acquired"],
                true
            );
        } else {
            assert!(
                store
                    .query(TENANT, PROJECT, B, export_content(&export))
                    .await
                    .is_err()
            );
            assert!(matches!(
                claim_cross_consumer(&store, &session, "claim-stale-current-input").await,
                Err(PgError::MissingDependency)
            ));
        }
    }
}

#[tokio::test]
async fn fixed_delivery_rechecks_original_source_approval_execution_and_bytes_after_update() {
    for fault in [
        "source_missing",
        "source_changed",
        "round_invalidated",
        "bytes_missing",
        "bytes_changed",
        "execution_failed",
    ] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let upstream = complete_simulated_upstream(&store).await;
        let consumer = fixed_export_consumer(&admin, &store, &db).await;
        adoption_roles(&admin).await;
        let session = adoption_session(&store, "fault-history-session").await;
        let export = publish_export(&store, &consumer, &upstream, "fault-history-export").await;
        adopt_export(&store, &session, &export, "fault-history-adopt").await;
        let row = admin
            .query_one(
                "SELECT manifest_json,proof_json FROM awr_team.workstream_artifact_exports",
                &[],
            )
            .await
            .unwrap();
        let manifest: Value = row.get(0);
        let proof: Value = row.get(1);
        advance_history_source(&admin, &store, &db, "fault-provider-update").await;
        assert_eq!(
            prepare(&store, B, "b-private").await["data"]["adopted_dependencies"][0]["valid"],
            true
        );
        let prepared = prepare(&store, B, "b-private").await;
        let claim = command(
            &prepared,
            "claim-corrupt-fixed-input",
            "task.claim_available",
            json!({
                "session_id":session,"expected_session_version":"1",
                "expected_responsibility_version":prepared["data"]["responsibility"]["version"],
                "expected_work_version":prepared["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
                "ttl_seconds":600,
            }),
        );
        let snapshot = manifest["provider_source_snapshot_id"].as_str().unwrap();
        match fault {
            "source_missing" => {
                admin
                    .execute(
                        "DELETE FROM awr_team.workstream_snapshot_ownership WHERE snapshot_id=$1 AND work_id='a'",
                        &[&snapshot],
                    )
                    .await
                    .unwrap();
                admin
                    .execute(
                        "DELETE FROM awr_team.work_contracts WHERE snapshot_id=$1 AND work_id='a'",
                        &[&snapshot],
                    )
                    .await
                    .unwrap();
            }
            "source_changed" => {
                admin.execute("UPDATE awr_team.work_contracts SET contract_hash=repeat('f',64) WHERE snapshot_id=$1 AND work_id='a'", &[&snapshot]).await.unwrap();
            }
            "round_invalidated" => {
                admin
                    .execute(
                        "UPDATE awr_team.review_rounds SET state='invalidated' WHERE id=$1",
                        &[&proof["round"]["id"].as_str().unwrap()],
                    )
                    .await
                    .unwrap();
            }
            "bytes_missing" => {
                admin
                    .execute(
                        "UPDATE awr_team.artifacts SET content=NULL WHERE id=$1",
                        &[&manifest["artifact_id"].as_str().unwrap()],
                    )
                    .await
                    .unwrap();
            }
            "bytes_changed" => {
                admin
                    .execute(
                        "UPDATE awr_team.artifacts SET content=$1 WHERE id=$2",
                        &[
                            &b"corrupt historical bytes".to_vec(),
                            &manifest["artifact_id"].as_str().unwrap(),
                        ],
                    )
                    .await
                    .unwrap();
            }
            "execution_failed" => {
                admin
                    .execute(
                        "UPDATE awr_team.executions SET state='failed' WHERE id=$1",
                        &[&proof["evidence"]["execution_id"].as_str().unwrap()],
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        // A corrupt projection is a source divergence, not a valid input.
        assert!(
            store
                .query(TENANT, PROJECT, B, export_content(&export))
                .await
                .is_err(),
            "{fault}"
        );
        let mut q = query("work.prepare");
        q.work_id = Some("b-private".into());
        let read = store.query(TENANT, PROJECT, B, q).await;
        assert!(
            read.is_err() || read.unwrap()["data"]["adopted_dependencies"][0]["valid"] == false,
            "{fault}"
        );
        assert!(cross_execute(&store, B, claim).await.is_err(), "{fault}");
    }
}

#[tokio::test]
async fn fixed_delivery_keeps_accepted_approval_history_without_approving_new_evidence() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = fixed_export_consumer(&admin, &store, &db).await;
    let export = publish_export(&store, &consumer, &upstream, "accepted-history-export").await;
    let proof: Value = admin
        .query_one(
            "SELECT proof_json FROM awr_team.workstream_artifact_exports",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let evidence = run(&store,A,"new-history-evidence","evidence.submit",json!({
        "session_id":"session-a","expected_session_version":"1","execution_id":proof["evidence"]["execution_id"],
        "input_digest":INPUT,"dirty_tree":false,"artifact_text":"simulated artifact",
        "payload":{"passed":true,"output_digest":proof["evidence"]["output_digest"],"verification_note":"A new verification bundle needs its own approval"}
    })).await;
    let opened = open_simulated(
        &store,
        &evidence,
        RUNNER,
        "session-runner",
        "new-history-review",
    )
    .await;
    assert_eq!(opened["state"], "open");
    assert_eq!(
        admin
            .query_one(
                "SELECT state FROM awr_team.review_rounds WHERE id=$1",
                &[&proof["round"]["id"].as_str().unwrap()]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "approved"
    );
    assert_eq!(
        store
            .query(TENANT, PROJECT, B, export_content(&export))
            .await
            .unwrap()["data"]["text"],
        "simulated artifact"
    );
    assert!(matches!(
        run_err(
            &store,
            RUNNER,
            "reject-new-unapproved-history",
            "delivery.finalize",
            completion_args(&evidence)
        )
        .await,
        PgError::ReviewRequired | PgError::CompletionRejected
    ));
}

async fn complete_history_consumer(store: &WorkstreamReadStore, session: &str) -> Value {
    use sha2::Digest;
    let reviewer = cross_step(
        store,
        REVIEWER_TOKEN,
        "b-private",
        "history-review-session",
        "session.start",
        json!({"conversation_id":"history-review"}),
    )
    .await["session_id"]
        .clone();
    let supervisor = cross_step(
        store,
        RUNNER,
        "b-private",
        "history-finalize-session",
        "session.start",
        json!({"conversation_id":"history-finalize"}),
    )
    .await["session_id"]
        .clone();
    let claim = claim_cross_consumer(store, session, "history-consumer-claim")
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let p = prepare(store, B, "b-private").await;
    let intent = cross_step(
        store,
        B,
        "b-private",
        "history-consumer-intent",
        "execution.prepare",
        json!({
            "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "expected_work_version":p["data"]["runtime"]["work_version"],
            "input_digest":INPUT,"declared_scope":["src/integration"],
        }),
    )
    .await;
    let p = prepare(store, B, "b-private").await;
    let start = cross_execute(store, B, command(&p, "history-consumer-start", "execution.start", json!({
        "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],
        "execution_id":intent["execution_id"],"expected_execution_version":intent["execution_version"],
        "execution_mode":"caller_managed",
    }))).await.unwrap();
    assert_eq!(start["execution_authorized"], true);
    let output = hex_encode(&sha2::Sha256::digest(b"history consumer artifact"));
    cross_step(store, B, "b-private", "history-consumer-report", "execution.report", json!({
        "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],
        "expected_execution_version":start["receipt"]["data"]["execution_version"],"outcome":"succeeded",
        "output_digest":output,"observed_paths":["src/integration/result.json"],"note":"Observed the fixed-input fixture",
        "workspace_settlement":{"workspace_id":"history-consumer-workspace","input_digest":INPUT,
            "environment_digest":"c".repeat(64),"claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "executor_stopped":true,"no_external_effects":true},
    })).await;
    let evidence = cross_step(store, B, "b-private", "history-consumer-evidence", "evidence.submit", json!({
        "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],
        "input_digest":INPUT,"dirty_tree":false,"artifact_text":"history consumer artifact",
        "payload":{"passed":true,"output_digest":output},
    })).await;
    let round = cross_step(store, RUNNER, "b-private", "history-consumer-open", "review.open", json!({
        "session_id":supervisor,"expected_session_version":"1","evidence_id":evidence["evidence_id"],
    })).await;
    let mut decision = args(&round);
    decision["session_id"] = reviewer;
    cross_step(
        store,
        REVIEWER_TOKEN,
        "b-private",
        "history-consumer-decision",
        "review.decide",
        decision,
    )
    .await;
    let mut complete = completion_args(&evidence);
    complete["session_id"] = supervisor;
    cross_step(
        store,
        RUNNER,
        "b-private",
        "history-consumer-finalize",
        "delivery.finalize",
        complete,
    )
    .await
}

async fn history_runtime(admin: &Client) -> Value {
    json!(admin.query("SELECT work_id,state,selected_completion_id,work_version FROM awr_team.work_runtime WHERE scope_id='main' ORDER BY work_id", &[]).await.unwrap()
        .into_iter().map(|r| json!({"work_id":r.get::<_,String>(0),"state":r.get::<_,String>(1),
            "selected":r.get::<_,Option<String>>(2),"version":r.get::<_,i64>(3)})).collect::<Vec<_>>())
}

#[tokio::test]
async fn fixed_delivery_cuts_mixed_chain_and_diamond_invalidation_and_rolls_back_atomically() {
    for fault in [
        "provider_changed",
        "consumer_changed",
        "revoked",
        "original_approval_invalidated",
    ] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let upstream = complete_simulated_upstream(&store).await;
        let mut consumer = fixed_export_consumer(&admin, &store, &db).await;
        consumer.completion_policy = SIMULATED_POLICY.into();
        consumer.scope_paths = vec!["src/integration".into()];
        consumer.verification_requirements = vec!["Verify the integrated artifact".into()];
        consumer.execution_settlement = Some(awr_team::ExecutionSettlementPolicy {
            mode: awr_team::ExecutionSettlementMode::IndependentWorkspaceV1,
            workspace_id: "history-consumer-workspace".into(),
        });
        admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='b-private'",
            &[&json!(consumer), &consumer.hash().unwrap()]).await.unwrap();
        adoption_roles(&admin).await;
        let session = adoption_session(&store, "history-graph-session").await;
        let export = publish_export(&store, &consumer, &upstream, "history-graph-export").await;
        adopt_export(&store, &session, &export, "history-graph-adopt").await;
        let completed = Box::pin(complete_history_consumer(&store, &session)).await;
        release_settled_history_claim(&admin, &store, A, "a", "graph-provider").await;
        release_settled_history_claim(&admin, &store, B, "b-private", "graph-consumer").await;

        let mut bundle = history_source_bundle(&admin).await;
        let a = bundle
            .contracts
            .iter()
            .find(|e| e.contract.work_id.as_str() == "a")
            .unwrap()
            .clone();
        let b = bundle
            .contracts
            .iter()
            .find(|e| e.contract.work_id.as_str() == "b-private")
            .unwrap()
            .clone();
        // The actual a->b fixed adoption and finalized execution are the root
        // input. Additional synthetic selected receipts isolate graph projection
        // behavior; they do not claim review or native business acceptance.
        for (key, owner, dependencies) in [
            ("c-fixed-tail", b.workstream_id, vec!["b-private"]),
            ("d-current", a.workstream_id, vec!["a"]),
            (
                "e-diamond",
                b.workstream_id,
                vec!["c-fixed-tail", "d-current"],
            ),
            ("f-tail", b.workstream_id, vec!["c-fixed-tail"]),
            ("unrelated", a.workstream_id, vec![]),
        ] {
            let mut entry = a.clone();
            entry.workstream_id = owner;
            entry.contract.work_id = awr_team::WorkId::new(key).unwrap();
            entry.contract.external_key = key.into();
            entry.contract.required_dependencies =
                dependencies.into_iter().map(Into::into).collect();
            entry.contract.dependency_acceptance.clear();
            if key == "e-diamond" {
                entry.contract.codec = WorkContract::CODEC_V6.into();
                entry.contract.dependency_acceptance.insert("d-current".into(),
                    awr_team::DependencyAcceptanceMode::CrossWorkstream(awr_team::CrossWorkstreamDependencyPolicy {
                        review_assurance: awr_team::CrossWorkstreamReviewAssurance::SimulatedMemberIndependent,
                        version_policy: awr_core::DeliveryVersionPolicy::CurrentContract,
                    }));
            }
            bundle.contracts.push(entry);
        }
        let original = history_source_candidate(&db, &bundle).await;
        activate_history_candidate(&db, &original).await.unwrap();
        for entry in &bundle.contracts {
            let work = entry.contract.work_id.as_str();
            if matches!(work, "a" | "b-private" | "c") {
                continue;
            }
            let receipt = format!("graph-{work}");
            admin.execute("INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json)
                VALUES($1,$2,$3,$4,'main',$5,'fixture-result','fixture-binding','fixture-evidence','review','{}')",
                &[&TENANT,&PROJECT,&receipt,&work,&entry.contract.hash().unwrap()]).await.unwrap();
            admin.execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,selected_completion_id,work_version)
                VALUES($1,$2,'main',$3,'completed',$4,6)", &[&TENANT,&PROJECT,&work,&receipt]).await.unwrap();
            for predecessor in &entry.contract.required_dependencies {
                let pin = match predecessor.as_str() {
                    "a" => upstream["receipt_id"].as_str().unwrap().to_string(),
                    "b-private" => completed["receipt_id"].as_str().unwrap().to_string(),
                    _ => format!("graph-{predecessor}"),
                };
                admin.execute("INSERT INTO awr_team.completion_dependencies(tenant_id,project_id,completion_id,predecessor_work_id,predecessor_completion_id)
                    VALUES($1,$2,$3,$4,$5)", &[&TENANT,&PROJECT,&receipt,&predecessor,&pin]).await.unwrap();
            }
        }
        let before = history_runtime(&admin).await;
        let receipts: Value = admin
            .query_one(
                "SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.completion_receipts r",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        let adoptions: Value = admin.query_one("SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.workstream_artifact_adoptions r", &[]).await.unwrap().get(0);
        match fault {
            "provider_changed" | "consumer_changed" => {
                let work = if fault == "provider_changed" {
                    "a"
                } else {
                    "b-private"
                };
                bundle
                    .contracts
                    .iter_mut()
                    .find(|e| e.contract.work_id.as_str() == work)
                    .unwrap()
                    .contract
                    .acceptance
                    .push("Recheck the changed work contract".into());
            }
            "revoked" => revoke_adoption_export(&store, &export, "history-graph-revoke").await,
            "original_approval_invalidated" => {
                admin.execute("UPDATE awr_team.review_rounds SET state='invalidated' WHERE id=(SELECT approved_by_json->>'review_round_id' FROM awr_team.completion_receipts WHERE id=$1)",
                    &[&upstream["receipt_id"].as_str().unwrap()]).await.unwrap();
            }
            _ => unreachable!(),
        }
        bundle.catalog.workstreams[0].title = format!("Graph update {fault}");
        let candidate = history_source_candidate(&db, &bundle).await;
        admin.batch_execute("CREATE FUNCTION awr_team.fail_history_source_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
            IF NEW.event_type='source.activated' THEN RAISE EXCEPTION 'injected history source failure'; END IF; RETURN NEW; END $$;
            CREATE TRIGGER fail_history_source_event BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.fail_history_source_event()"
        ).await.unwrap();
        assert!(activate_history_candidate(&db, &candidate).await.is_err());
        assert_eq!(
            history_runtime(&admin).await,
            before,
            "{fault}: rolled back selections"
        );
        assert_eq!(
            admin
                .query_one(
                    "SELECT active_snapshot_id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
                    &[&TENANT, &PROJECT]
                )
                .await
                .unwrap()
                .get::<_, String>(0),
            original.snapshot_id
        );
        admin.batch_execute("DROP TRIGGER fail_history_source_event ON awr_team.events; DROP FUNCTION awr_team.fail_history_source_event()"
        ).await.unwrap();
        activate_history_candidate(&db, &candidate).await.unwrap();
        let expected = if fault == "provider_changed" {
            vec!["a", "d-current", "e-diamond"]
        } else {
            vec!["b-private", "c-fixed-tail", "e-diamond", "f-tail"]
        };
        for row in history_runtime(&admin).await.as_array().unwrap() {
            let work = row["work_id"].as_str().unwrap();
            let previous = before
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["work_id"] == work)
                .unwrap();
            if expected.contains(&work) {
                assert_eq!(row["state"], "unclaimed", "{fault}/{work}");
                assert_eq!(row["selected"], Value::Null);
                assert_eq!(
                    row["version"].as_i64().unwrap(),
                    previous["version"].as_i64().unwrap() + 1
                );
            } else {
                assert_eq!(row, previous, "{fault}: unrelated or fixed input retained");
            }
        }
        assert_eq!(
            admin
                .query_one(
                    "SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.completion_receipts r",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, Value>(0),
            receipts
        );
        assert_eq!(admin.query_one("SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.workstream_artifact_adoptions r", &[]).await.unwrap().get::<_,Value>(0), adoptions);
    }
}
