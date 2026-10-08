use sha2::Digest;

// Mechanism fixtures, not native business acceptance. Ordinary authenticated
// commands produce the artifact, independent review, receipt and disclosure.
async fn exported_consumer(admin: &Client, store: &WorkstreamReadStore, db: &str) -> WorkContract {
    let consumer = configure_export_consumer(admin, awr_team::CrossWorkstreamReviewAssurance::SimulatedMemberIndependent).await;
    issue_member_grant(db,"simulation-consumer","person-author","agent","cli-b","bind-agent",false).await;
    assert!(store.query(TENANT, PROJECT, B, query("capabilities")).await.is_ok());
    consumer
}

async fn configure_export_consumer(admin: &Client, assurance: awr_team::CrossWorkstreamReviewAssurance) -> WorkContract {
    let mut consumer: WorkContract = serde_json::from_value(admin.query_one(
        "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='b-private'", &[]
    ).await.unwrap().get(0)).unwrap();
    consumer.codec = WorkContract::CODEC_V6.into();
    consumer.required_dependencies = vec!["a".into()];
    consumer.dependency_acceptance.insert("a".into(), awr_team::DependencyAcceptanceMode::CrossWorkstream(
        awr_team::CrossWorkstreamDependencyPolicy {
            review_assurance: assurance,
            version_policy: awr_core::DeliveryVersionPolicy::CurrentContract,
        }));
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='b-private'",
        &[&json!(consumer), &consumer.hash().unwrap()]).await.unwrap();
    consumer
}

async fn publish_request(store: &WorkstreamReadStore, consumer: &WorkContract, receipt: &Value, key: &str) -> awr_team_pg::WorkstreamCommand {
    let p = prepare(store, RUNNER, "a").await;
    command(&p,key,"delivery.export.publish",json!({"session_id":"session-runner","expected_session_version":"1",
        "consumer_work_id":"b-private","expected_consumer_contract_hash":consumer.hash().unwrap(),"expected_consumer_ownership_version":"1",
        "receipt_id":receipt["receipt_id"],"expected_artifact_sha256":crate::hex_encode(&sha2::Sha256::digest(b"simulated artifact"))}))
}

async fn publish_export(store: &WorkstreamReadStore, consumer: &WorkContract, receipt: &Value, key: &str) -> Value {
    let cmd = publish_request(store,consumer,receipt,key).await;
    store.commands().execute(TENANT,PROJECT,RUNNER,cmd).await.unwrap()
}

async fn discover_exports(store: &WorkstreamReadStore) -> Value {
    let mut q = query("delivery.exports"); q.work_id = Some("b-private".into());
    store.query(TENANT,PROJECT,B,q).await.unwrap()["data"].clone()
}

fn export_content(export: &Value) -> awr_team_pg::WorkstreamQuery {
    let mut q = query("artifact.content"); q.work_id=Some("b-private".into());
    q.export_id=Some(export["receipt"]["data"]["export_id"].as_str().unwrap().into()); q
}

#[tokio::test]
async fn approved_cross_stream_export_discloses_only_exact_artifact_and_keeps_adoption_blocked() {
    let (_g,admin,db,store)=setup().await;
    seed_simulated_members(&admin,&db).await;
    let receipt=complete_simulated_upstream(&store).await;
    let consumer=exported_consumer(&admin,&store,&db).await;
    assert!(discover_exports(&store).await["items"].as_array().unwrap().is_empty());
    let exported=publish_export(&store,&consumer,&receipt,"publish-artifact").await;
    assert_eq!(exported["receipt"]["data"]["adopted"],false);
    let discovered=discover_exports(&store).await;
    assert_eq!(discovered["items"][0]["available"],true);
    assert_eq!(discovered["items"][0]["review"]["human_approval"],false);
    assert_eq!(discovered["items"][0]["review"]["execution_basis"],"caller_asserted_workspace_settled");
    let content=store.query(TENANT,PROJECT,B,export_content(&exported)).await.unwrap();
    assert_eq!(content["data"]["text"],"simulated artifact");
    assert_eq!(content["data"]["upstream_source_access"],false);
    assert_eq!(content["data"]["execution_authorized"],false);
    let encoded=content.to_string();
    for private in ["session-a","session-runner","member_origins_json","open_loops","PRIVATE NEXT ACTION"] { assert!(!encoded.contains(private)); }
    assert!(matches!(store.query(TENANT,PROJECT,B,{let mut q=query("work.snapshot");q.work_id=Some("a".into());q}).await,Err(PgError::Forbidden)));
    let artifact: String=admin.query_one("SELECT artifact_id FROM awr_team.workstream_artifact_exports",&[]).await.unwrap().get(0);
    let mut old=export_content(&exported);old.export_id=None;old.artifact_id=Some(artifact);
    assert!(matches!(store.query(TENANT,PROJECT,B,old).await,Err(PgError::Forbidden)));
    let mut wrong=export_content(&exported);wrong.work_id=Some("c".into());
    assert!(store.query(TENANT,PROJECT,A,wrong).await.is_err());
    let mut small=export_content(&exported);small.max_context_bytes=Some(1);
    assert!(matches!(store.query(TENANT,PROJECT,B,small).await,Err(PgError::ContextIncomplete)));
    let mut wrong_sha=export_content(&exported);wrong_sha.expected_sha256=Some("f".repeat(64));
    assert!(matches!(store.query(TENANT,PROJECT,B,wrong_sha).await,Err(PgError::SnapshotDrift(_))));
    // Reading an export never satisfies the dependency gate or grants ownership.
    let mut q=query("work.prepare");q.work_id=Some("b-private".into());
    let p=store.query(TENANT,PROJECT,B,q).await.unwrap();
    assert_eq!(p["data"]["context_complete"],false);
    assert_eq!(p["data"]["dependency_export_unavailable"],true);
}

#[tokio::test]
async fn export_publication_replay_revocation_and_concurrency_preserve_original_outcome() {
    let (_g,admin,db,store)=setup().await;
    seed_simulated_members(&admin,&db).await;
    let receipt=complete_simulated_upstream(&store).await;
    let consumer=exported_consumer(&admin,&store,&db).await;
    let cmd=publish_request(&store,&consumer,&receipt,"concurrent-export").await;
    let commands=store.commands();
    let (a,b)=tokio::join!(commands.execute(TENANT,PROJECT,RUNNER,cmd.clone()),commands.execute(TENANT,PROJECT,RUNNER,cmd.clone()));
    let a=a.unwrap();let b=b.unwrap();assert_ne!(a["replayed"],b["replayed"]);assert_eq!(a["receipt"],b["receipt"]);
    let mut changed=cmd.clone();changed.args["expected_artifact_sha256"]=json!("f".repeat(64));
    assert!(matches!(store.commands().execute(TENANT,PROJECT,RUNNER,changed).await,Err(PgError::IdempotencyConflict)));
    let duplicate=publish_export(&store,&consumer,&receipt,"second-publication-intent").await;
    assert_eq!(duplicate["receipt"]["data"]["export_id"],a["receipt"]["data"]["export_id"]);
    assert_eq!(duplicate["receipt"]["data"]["already_published"],true);
    let p=prepare(&store,RUNNER,"a").await;
    let revoke=command(&p,"revoke-artifact","delivery.export.revoke",json!({"session_id":"session-runner","expected_session_version":"1",
        "export_id":a["receipt"]["data"]["export_id"],"expected_export_version":"1"}));
    let revoked=store.commands().execute(TENANT,PROJECT,RUNNER,revoke.clone()).await.unwrap();
    assert_eq!(revoked["receipt"]["data"]["status"],"revoked");
    assert_eq!(store.commands().execute(TENANT,PROJECT,RUNNER,revoke).await.unwrap()["receipt"],revoked["receipt"]);
    let replay=store.commands().execute(TENANT,PROJECT,RUNNER,cmd).await.unwrap();
    assert_eq!(replay["receipt"],a["receipt"]);assert_eq!(replay["execution_authorized"],false);
    assert!(matches!(store.query(TENANT,PROJECT,B,export_content(&a)).await,Err(PgError::Forbidden)));
    assert_eq!(discover_exports(&store).await["items"][0]["status"],"revoked");
    let inspected=store.query(TENANT,PROJECT,RUNNER,{let mut q=query("command.inspect");q.work_id=Some("a".into());q.request_id=Some("concurrent-export".into());q}).await.unwrap();
    assert_eq!(inspected["data"]["state"],"committed");
    assert_eq!(inspected["data"]["receipt"],a["receipt"]);
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-runner'").await.unwrap();
    let p=prepare(&store,RUNNER,"a").await;
    assert!(matches!(store.commands().execute(TENANT,PROJECT,RUNNER,command(&p,"after-revocation","delivery.export.publish",publish_request(&store,&consumer,&receipt,"x").await.args)).await,Err(PgError::Forbidden)));
}

#[tokio::test]
async fn publication_and_content_revalidate_policy_artifact_and_exact_original_review() {
    let (_g,admin,db,store)=setup().await;
    seed_simulated_members(&admin,&db).await;
    let receipt=complete_simulated_upstream(&store).await;
    let consumer=exported_consumer(&admin,&store,&db).await;
    let mut cmd=publish_request(&store,&consumer,&receipt,"invalid-publication").await;
    cmd.args["expected_consumer_contract_hash"]=json!("f".repeat(64));
    assert!(matches!(store.commands().execute(TENANT,PROJECT,RUNNER,cmd).await,Err(PgError::PreconditionsChanged)));
    let exported=publish_export(&store,&consumer,&receipt,"verified-publication").await;
    let row=admin.query_one("SELECT artifact_id,proof_json FROM awr_team.workstream_artifact_exports",&[]).await.unwrap();
    let artifact: String=row.get(0);let archive: Value=row.get(1);let round=archive["receipt"]["approved_by_json"]["review_round_id"].as_str().unwrap();
    admin.execute("UPDATE awr_team.artifacts SET content=NULL WHERE id=$1",&[&artifact]).await.unwrap();
    assert!(matches!(store.query(TENANT,PROJECT,B,export_content(&exported)).await,Err(PgError::EvidenceInvalid)));
    assert_eq!(discover_exports(&store).await["items"][0]["available"],false);
    admin.execute("UPDATE awr_team.artifacts SET content=$1 WHERE id=$2",&[&b"simulated artifact".to_vec(),&artifact]).await.unwrap();
    admin.execute("UPDATE awr_team.review_rounds SET state='invalidated' WHERE id=$1",&[&round]).await.unwrap();
    assert!(matches!(store.query(TENANT,PROJECT,B,export_content(&exported)).await,Err(PgError::ReviewRequired)));
    admin.execute("UPDATE awr_team.review_rounds SET state='approved' WHERE id=$1",&[&round]).await.unwrap();
    let mut changed=consumer.clone();changed.acceptance.push("Changed consumer requirement".into());
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='b-private'",&[&json!(changed),&changed.hash().unwrap()]).await.unwrap();
    assert!(matches!(store.query(TENANT,PROJECT,B,export_content(&exported)).await,Err(PgError::PreconditionsChanged)));
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='b-private'",&[&json!(consumer),&consumer.hash().unwrap()]).await.unwrap();
    assert!(admin.batch_execute("UPDATE awr_team.completion_receipts SET approved_by_json=approved_by_json-'review_round_id'").await.is_err(),
        "The immutable simulated review basis cannot be rewritten by a test");
}

#[tokio::test]
async fn independent_human_export_rechecks_successful_execution_and_refuses_unpinned_legacy_review() {
    let (_g,admin,_,store)=setup().await;
    seed_review_actors(&admin).await;
    let contract=prepare(&store,A,"a").await["data"]["contract_hash"].as_str().unwrap().to_owned();
    insert_succeeded_execution(&admin,"export-exec",&contract,1).await;
    let evidence=run(&store,RUNNER,"export-evidence","evidence.submit",submit_args("session-runner","export-exec",&hex_encode(b"ws018-artifact"))).await;
    let opened=run(&store,A,"export-review-open","review.open",json!({"session_id":"session-a","expected_session_version":"1","evidence_id":evidence["evidence_id"]})).await;
    run(&store,REVIEWER_TOKEN,"export-review-accept","review.accept",json!({"session_id":"session-reviewer","expected_session_version":"1","round_id":opened["round_id"],"reason":"Independent artifact check"})).await;
    let receipt=run(&store,A,"export-finalize","delivery.finalize",json!({"session_id":"session-a","expected_session_version":"1","evidence_id":evidence["evidence_id"],"context_complete":true})).await;
    let consumer=configure_export_consumer(&admin,awr_team::CrossWorkstreamReviewAssurance::TeamIndependent).await;
    let args=json!({"session_id":"session-a","expected_session_version":"1","consumer_work_id":"b-private",
        "expected_consumer_contract_hash":consumer.hash().unwrap(),"expected_consumer_ownership_version":"1",
        "receipt_id":receipt["receipt_id"],"expected_artifact_sha256":evidence["artifact_digest"]});
    let prepared=prepare(&store,A,"a").await;
    let exported=store.commands().execute(TENANT,PROJECT,A,command(&prepared,"human-export","delivery.export.publish",args.clone())).await.unwrap();
    assert_eq!(discover_exports(&store).await["items"][0]["review"]["human_approval"],true);
    assert_eq!(store.query(TENANT,PROJECT,B,export_content(&exported)).await.unwrap()["data"]["text"],"ws018-artifact");
    admin.batch_execute("UPDATE awr_team.executions SET result_digest=repeat('f',64) WHERE id='export-exec'").await.unwrap();
    assert!(matches!(store.query(TENANT,PROJECT,B,export_content(&exported)).await,Err(PgError::EvidenceInvalid)));
    assert_eq!(discover_exports(&store).await["items"][0]["available"],false);
    admin.execute("UPDATE awr_team.executions SET result_digest=$1 WHERE id='export-exec'",&[&RESULT]).await.unwrap();
    assert!(store.query(TENANT,PROJECT,B,export_content(&exported)).await.is_ok());
    // Model an old Team receipt lacking exact original review IDs. Do not guess
    // those IDs from the current approved round or backfill the legacy record.
    admin.batch_execute("UPDATE awr_team.completion_receipts SET approved_by_json=approved_by_json-'review_round_id'").await.unwrap();
    let prepared=prepare(&store,A,"a").await;
    assert!(matches!(store.commands().execute(TENANT,PROJECT,A,command(&prepared,"legacy-review-export","delivery.export.publish",args)).await,Err(PgError::ReviewRequired)));
}

#[tokio::test]
async fn export_rollback_keeps_unknown_outcome_and_pagination_binds_current_consumer_authority() {
    let (_g,admin,db,store)=setup().await;
    seed_simulated_members(&admin,&db).await;
    let receipt=complete_simulated_upstream(&store).await;
    let consumer=exported_consumer(&admin,&store,&db).await;
    let cmd=publish_request(&store,&consumer,&receipt,"interrupted-export").await;
    // Inject a storage failure in this isolated fixture, with all domain guards
    // still enabled. The journal, project revision and export must roll back.
    admin.batch_execute("CREATE FUNCTION awr_team.fixture_fail_export() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture write failure'; END $$;
        CREATE TRIGGER fixture_fail_export BEFORE INSERT ON awr_team.workstream_artifact_exports FOR EACH ROW EXECUTE FUNCTION awr_team.fixture_fail_export()").await.unwrap();
    assert!(store.commands().execute(TENANT,PROJECT,RUNNER,cmd.clone()).await.is_err());
    let mut inspect=query("command.inspect");inspect.work_id=Some("a".into());inspect.request_id=Some("interrupted-export".into());
    assert_eq!(store.query(TENANT,PROJECT,RUNNER,inspect).await.unwrap()["data"]["state"],"unknown");
    assert!(discover_exports(&store).await["items"].as_array().unwrap().is_empty());
    assert_eq!(prepare(&store,RUNNER,"a").await["project_revision"],cmd.expected_project_revision);
    admin.batch_execute("DROP TRIGGER fixture_fail_export ON awr_team.workstream_artifact_exports; DROP FUNCTION awr_team.fixture_fail_export()").await.unwrap();
    let first=store.commands().execute(TENANT,PROJECT,RUNNER,cmd).await.unwrap();
    let p=prepare(&store,RUNNER,"a").await;
    let revoke=command(&p,"page-revoke","delivery.export.revoke",json!({"session_id":"session-runner","expected_session_version":"1",
        "export_id":first["receipt"]["data"]["export_id"],"expected_export_version":"1"}));
    let commands=store.commands();
    let (withdrawn,read)=tokio::join!(commands.execute(TENANT,PROJECT,RUNNER,revoke),store.query(TENANT,PROJECT,B,export_content(&first)));
    assert!(withdrawn.is_ok());
    assert!(read.is_ok()||matches!(read,Err(PgError::Forbidden)),"Read/revoke concurrency observes an entire committed snapshot");
    let second=publish_export(&store,&consumer,&receipt,"page-republish").await;
    assert_ne!(first["receipt"]["data"]["export_id"],second["receipt"]["data"]["export_id"]);
    let mut q=query("delivery.exports");q.work_id=Some("b-private".into());q.limit=Some(1);
    let page=store.query(TENANT,PROJECT,B,q.clone()).await.unwrap()["data"].clone();
    assert_eq!(page["items"].as_array().unwrap().len(),1);
    q.cursor=Some(page["next_cursor"].as_str().unwrap().into());
    let next=store.query(TENANT,PROJECT,B,q.clone()).await.unwrap()["data"].clone();
    assert_eq!(next["items"].as_array().unwrap().len(),1);
    assert_ne!(next["items"][0]["export_id"],page["items"][0]["export_id"]);
    assert!(next["next_cursor"].is_null());
    admin.batch_execute("UPDATE awr_team.project_memberships SET membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
    assert!(matches!(store.query(TENANT,PROJECT,B,q).await,Err(PgError::CursorExpired)));
}
