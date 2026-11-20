#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/delivery_integration.rs"]
mod integration_fixture;
use awr_core::Id;
use awr_team_pg::*;
use fixture::*;

#[path = "cases/review_round_reads.rs"]
mod review_round_reads;

#[path = "cases/handoff_reads.rs"]
mod handoff_reads;

#[path = "cases/dependency_reads.rs"]
mod dependency_reads;

#[path = "cases/responsibility_reads.rs"]
mod responsibility_reads;

#[path = "cases/document_reads.rs"]
mod document_reads;

fn work_item<'a>(list: &'a Value, work: &str) -> &'a Value {
    list["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["work_id"] == work)
        .unwrap()
}

async fn source_record(admin: &tokio_postgres::Client) -> (String, Value) {
    let row=admin.query_one("SELECT s.id,s.source_ref_json FROM awr_team.source_snapshots s
        JOIN awr_team.projects p ON p.tenant_id=s.tenant_id AND p.id=s.project_id AND p.active_snapshot_id=s.id
        WHERE p.tenant_id=$1 AND p.id=$2",&[&TENANT,&PROJECT]).await.unwrap();
    (row.get(0), row.get(1))
}

async fn replace_source_record(admin: &tokio_postgres::Client, id: &str, value: &Value) {
    admin
        .execute(
            "UPDATE awr_team.source_snapshots SET source_ref_json=$2 WHERE id=$1",
            &[&id, &value],
        )
        .await
        .unwrap();
}

async fn add_required_source(admin: &tokio_postgres::Client, stream: u128, path: &str) {
    let mut catalog: Value = admin
        .query_one("SELECT catalog_json FROM awr_team.workstream_catalogs", &[])
        .await
        .unwrap()
        .get(0);
    let entry = catalog["workstreams"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|s| s["id"] == Id::from(stream).to_string())
        .unwrap();
    entry["acceptance_contracts"]
        .as_array_mut()
        .unwrap()
        .push(json!(path));
    admin
        .execute(
            "UPDATE awr_team.workstream_catalogs SET catalog_json=$1",
            &[&catalog],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn required_catalog_access_uses_activated_scope_even_when_source_bytes_are_unusable() {
    let (_guard, admin, _, store) = setup().await;
    add_required_source(&admin, 1, "workstreams.json").await;
    let (id, original) = source_record(&admin).await;
    let mut snapshots = Vec::new();
    for (text, status) in [
        (
            json!({"catalog":{"workstreams":[{"id":Id::from(1).to_string()}]},"contracts":[]})
                .to_string()
                .into(),
            "changed",
        ),
        (Some("{invalid catalog".to_string()), "changed"),
        (None, "unverified"),
    ] {
        let mut changed = original.clone();
        let file = changed["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|file| file["path"] == "workstreams.json")
            .unwrap();
        file["text"] = json!(text);
        snapshots.push((changed, status));
    }
    let mut missing = original.clone();
    missing["files"]
        .as_array_mut()
        .unwrap()
        .retain(|file| file["path"] != "workstreams.json");
    snapshots.push((missing, "missing"));
    let mut q = query("sources.inspect");
    q.work_id = Some("a".into());
    for (snapshot, _) in &snapshots {
        replace_source_record(&admin, &id, snapshot).await;
        let result = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
        let row = &result["data"]["items"][0];
        assert_eq!(row["status"], "forbidden");
        assert!(
            row["path"].is_null()
                && row["expected_sha256"].is_null()
                && row["current_sha256"].is_null()
        );
        assert_eq!(result["data"]["collection"]["complete"], false);
        assert!(!result.to_string().contains("workstreams.json"));
        let prepared = prepare(&store, A, "a").await;
        assert_eq!(prepared["data"]["context_complete"], false);
        assert!(
            prepared["data"]["completeness_reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason == "required_spec_forbidden")
        );
        let mut content = query("source.content");
        content.source_path = Some("workstreams.json".into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, content).await,
            Err(PgError::Forbidden)
        ));
    }
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
        workstream_id,authority_version,can_read) VALUES($1,$2,'agent','cli-a',$3,1,true)",
            &[&TENANT, &PROJECT, &Id::from(2).to_string()],
        )
        .await
        .unwrap();
    for (snapshot, status) in snapshots {
        replace_source_record(&admin, &id, &snapshot).await;
        let result = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
        assert_eq!(result["data"]["items"][0]["status"], status);
        assert_eq!(result["data"]["collection"]["complete"], false);
    }
}

#[tokio::test]
async fn required_source_observation_distinguishes_integrity_missing_and_restricted_documents() {
    let (_guard, admin, _, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"Public API contract".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"PRIVATE peer contract".to_vec(),
        },
    ])
    .await;
    let (id, original) = source_record(&admin).await;
    let mut q = query("sources.inspect");
    q.work_id = Some("a".into());
    let consistent = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let data = &consistent["data"];
    let row = &data["items"][0];
    assert_eq!(data["collection"]["complete"], true);
    assert_eq!(data["collection"]["scope"], "work");
    assert_eq!(row["status"], "consistent");
    assert_eq!(row["expected_sha256"], row["current_sha256"]);
    assert!(row["checked_at_unix_ms"].is_i64());
    assert!(!consistent.to_string().contains("Public API contract"));
    assert!(!consistent.to_string().contains("PRIVATE"));
    let mut changed = original.clone();
    changed["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["path"] == "docs/alpha.md")
        .unwrap()["text"] = json!("Revised API contract");
    replace_source_record(&admin, &id, &changed).await;
    let mismatch = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(mismatch["data"]["items"][0]["status"], "changed");
    assert_ne!(
        mismatch["data"]["items"][0]["expected_sha256"],
        mismatch["data"]["items"][0]["current_sha256"]
    );
    assert_eq!(mismatch["data"]["collection"]["complete"], false);
    assert_eq!(
        mismatch["data"]["collection"]["recovery"][0]["reason"],
        "spec_digest_mismatch"
    );
    let mut missing = original.clone();
    missing["files"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["path"] != "docs/alpha.md");
    replace_source_record(&admin, &id, &missing).await;
    let absent = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(absent["data"]["items"][0]["status"], "missing");
    assert_eq!(absent["data"]["collection"]["complete"], false);
    assert_eq!(
        absent["data"]["collection"]["recovery"][0]["reason"],
        "required_spec_missing"
    );
    assert!(absent["data"]["items"][0]["current_sha256"].is_null());
    replace_source_record(&admin, &id, &original).await;
    add_required_source(&admin, 2, "docs/alpha.md").await;
    let restricted = store.query(TENANT, PROJECT, A, q).await.unwrap();
    let row = &restricted["data"]["items"][0];
    assert_eq!(row["status"], "forbidden");
    assert_eq!(row["missing"]["path"], "permission_restricted");
    assert!(
        row["path"].is_null()
            && row["expected_sha256"].is_null()
            && row["current_sha256"].is_null()
    );
    assert_eq!(restricted["data"]["collection"]["complete"], false);
    assert_eq!(
        restricted["data"]["collection"]["recovery"][0]["reason"],
        "required_spec_forbidden"
    );
    assert!(!restricted.to_string().contains("docs/alpha.md"));
    assert!(!restricted.to_string().contains("PRIVATE"));
}

#[tokio::test]
async fn required_source_completeness_covers_all_pages_and_rechecks_current_permissions() {
    let (_guard, admin, _, store) = setup_with_specs(vec![SourceFile {
        path: "docs/alpha.md".into(),
        bytes: b"Public requirements".to_vec(),
    }])
    .await;
    add_required_source(&admin, 1, "docs/z-notes.md").await;
    let mut q = query("sources.inspect");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["items"][0]["status"], "consistent");
    assert_eq!(first["data"]["collection"]["complete"], false);
    assert_eq!(
        first["data"]["collection"]["reasons"],
        json!(["required_spec_missing"])
    );
    assert_eq!(first["data"]["collection"]["recovery"], json!([]));
    assert_eq!(first["data"]["total"], 2);
    assert_eq!(first["data"]["truncated"], true);
    let cursor = first["data"]["next_cursor"].as_str().unwrap();
    assert!(!cursor.contains("docs/"));
    q.cursor = Some(cursor.into());
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(second["data"]["items"][0]["status"], "missing");
    assert_eq!(
        second["data"]["collection"]["recovery"][0]["source_position"],
        2
    );
    assert!(second["data"]["next_cursor"].is_null());
    let (id, mut current) = source_record(&admin).await;
    current["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["path"] == "docs/alpha.md")
        .unwrap()["text"] = json!("Updated requirements");
    replace_source_record(&admin, &id, &current).await;
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    for work in ["b-private", "missing"] {
        let mut selected = query("sources.inspect");
        selected.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, selected).await,
            Err(PgError::Forbidden)
        ));
    }
    let mut small = query("sources.inspect");
    small.max_context_bytes = Some(10);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, small).await,
        Err(PgError::ResponseTooLarge)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .query(TENANT, PROJECT, A, query("sources.inspect"))
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn required_source_observation_does_not_treat_missing_integrity_metadata_as_verified() {
    let (_guard, admin, _, store) = setup_with_specs(vec![SourceFile {
        path: "docs/alpha.md".into(),
        bytes: b"Public requirements".to_vec(),
    }])
    .await;
    let (id, original) = source_record(&admin).await;
    for (field, reason, missing_field) in [
        ("text", "source_text_not_recorded", "current_sha256"),
        ("sha256", "expected_digest_not_recorded", "expected_sha256"),
    ] {
        let mut changed = original.clone();
        let source = changed["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|file| file["path"] == "docs/alpha.md")
            .unwrap();
        source.as_object_mut().unwrap().remove(field);
        replace_source_record(&admin, &id, &changed).await;
        let result = store
            .query(TENANT, PROJECT, A, query("sources.inspect"))
            .await
            .unwrap();
        assert_eq!(result["data"]["collection"]["complete"], false);
        assert_eq!(result["data"]["items"][0]["status"], "unverified");
        assert!(result["data"]["items"][0][missing_field].is_null());
        assert_eq!(result["data"]["collection"]["reasons"], json!([reason]));
        assert_eq!(
            result["data"]["collection"]["recovery"][0]["recognized"],
            false
        );
        assert!(result["data"]["collection"]["recovery"][0]["next_action"].is_null());
    }
}

async fn planning_draft(admin: &tokio_postgres::Client, work: &str) -> Value {
    let row=admin.query_one("SELECT contract_json,title,definition_state FROM awr_team.work_contracts WHERE work_id=$1",&[&work]).await.unwrap();
    let mut draft: Value = row.get(0);
    draft.as_object_mut().unwrap().remove("codec");
    draft["title"] = json!(row.get::<_, String>(1));
    draft["definition_state"] = json!(row.get::<_, String>(2));
    draft
}

async fn seed_planning_candidate(admin: &tokio_postgres::Client, id: &str, changes: Value) {
    admin.execute("INSERT INTO awr_team.planning_candidates(tenant_id,project_id,id,author_person_id,
        author_actor_id,author_client_id,baseline_digest,baseline_epoch,draft_revision,candidate_digest,
        state,changes_json,self_approve_policy_json,delivery_completion_policy)
        VALUES($1,$2,$3,'agent','agent','cli-a','earlier-plan','1',1,$4,'drafting',$5,'{}','review')",
        &[&TENANT,&PROJECT,&id,&"b".repeat(64),&changes]).await.unwrap();
}

#[tokio::test]
async fn planning_observation_matches_manifest_and_epoch_instead_of_snapshot_identity() {
    let (_guard, admin, _, store) = setup().await;
    let draft = planning_draft(&admin, "a").await;
    seed_planning_candidate(
        &admin,
        "current-plan",
        json!([
            {"op":"edit_fields","before":draft,"after":draft}
        ]),
    )
    .await;
    let baseline = admin
        .query_one(
            "SELECT s.manifest_digest,p.authority_epoch::text,p.active_snapshot_id
        FROM awr_team.projects p JOIN awr_team.source_snapshots s
          ON s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id
        WHERE p.tenant_id=$1 AND p.id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    let digest: String = baseline.get(0);
    let epoch: String = baseline.get(1);
    assert_ne!(digest, baseline.get::<_, String>(2));
    admin
        .execute(
            "UPDATE awr_team.planning_candidates SET baseline_digest=$1,baseline_epoch=$2",
            &[&digest, &epoch],
        )
        .await
        .unwrap();
    let current = store
        .query(TENANT, PROJECT, A, query("planning.candidates"))
        .await
        .unwrap();
    let observed = &current["data"]["items"][0]["baseline"];
    assert_eq!(observed["source_manifest_digest"], digest);
    assert!(observed.get("source_snapshot_id").is_none());
    assert_eq!(observed["matches_current"], true);
    admin
        .batch_execute("UPDATE awr_team.planning_candidates SET baseline_digest='earlier-manifest'")
        .await
        .unwrap();
    let stale = store
        .query(TENANT, PROJECT, A, query("planning.candidates"))
        .await
        .unwrap();
    assert_eq!(
        stale["data"]["items"][0]["baseline"]["matches_current"],
        false
    );
    admin.execute("UPDATE awr_team.planning_candidates SET baseline_digest=$1,baseline_epoch='earlier-epoch'",
        &[&digest]).await.unwrap();
    let stale = store
        .query(TENANT, PROJECT, A, query("planning.candidates"))
        .await
        .unwrap();
    assert_eq!(
        stale["data"]["items"][0]["baseline"]["matches_current"],
        false
    );
}

#[tokio::test]
async fn planning_observation_includes_assurance_and_settlement_changes_without_hidden_references()
{
    let (_guard, admin, _, store) = setup().await;
    let row = admin
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='c'",
            &[],
        )
        .await
        .unwrap();
    let mut contract: awr_team::WorkContract = serde_json::from_value(row.get(0)).unwrap();
    contract.required_dependencies.push("a".into());
    let contract_hash = contract.hash().unwrap();
    let contract_json = serde_json::to_value(contract).unwrap();
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='c'",
        &[&contract_json, &contract_hash]).await.unwrap();
    let current = planning_draft(&admin, "c").await;
    let mut assurance = current.clone();
    assurance["dependency_acceptance"] = json!({"a":"agent_reviewed_caller_asserted_reconciled"});
    let mut restricted = assurance.clone();
    restricted["dependency_acceptance"]["b-private"] =
        json!("agent_reviewed_caller_asserted_reconciled");
    let mut settlement = current.clone();
    settlement["execution_settlement"] =
        json!({"mode":"independent_workspace_v1","workspace_id":"review-workspace"});
    for (id, after) in [
        ("assurance", assurance),
        ("restricted", restricted),
        ("settlement", settlement),
    ] {
        seed_planning_candidate(
            &admin,
            id,
            json!([
                {"op":"edit_fields","before":current,"after":after}
            ]),
        )
        .await;
    }
    let result = store
        .query(TENANT, PROJECT, A, query("planning.candidates"))
        .await
        .unwrap();
    let items = result["data"]["items"].as_array().unwrap();
    for item in items {
        assert_eq!(item["current_diff"]["items"][0]["kind"], "changed");
        assert_eq!(item["current_diff"]["counts"]["changed"], 1);
        let fields = item["current_diff"]["items"][0]["fields"]
            .as_array()
            .unwrap();
        assert_eq!(fields.len(), 1);
        let field = &fields[0];
        if item["candidate_id"] == "settlement" {
            assert_eq!(field["field"], "execution_settlement");
            assert_eq!(field["after"]["value"]["workspace_id"], "review-workspace");
        } else {
            assert_eq!(field["field"], "dependency_acceptance");
            if item["candidate_id"] == "restricted" {
                assert!(field["after"]["value"].is_null());
                assert_eq!(field["after"]["reason"], "permission_restricted");
            } else {
                assert_eq!(
                    field["after"]["value"]["a"],
                    "agent_reviewed_caller_asserted_reconciled"
                );
            }
        }
    }
    assert!(!result.to_string().contains("b-private"));
}

#[tokio::test]
async fn planning_observation_compares_with_current_plan_and_does_not_expose_hidden_candidates() {
    let (_guard, admin, _, store) = setup().await;
    let current = planning_draft(&admin, "a").await;
    let private = planning_draft(&admin, "b-private").await;
    let mut changed = current.clone();
    changed["title"] = json!("Clarify sign-in requirements");
    let mut old = current.clone();
    old["acceptance"] = json!(["Obsolete historical criterion"]);
    seed_planning_candidate(
        &admin,
        "plan-a",
        json!([{"op":"edit_fields","before":old,"after":changed}]),
    )
    .await;
    let mut added = current.clone();
    added["work_id"] = json!("proposed-guide");
    added["external_key"] = json!("GUIDE");
    added["title"] = json!("Write the help guide");
    added["workstream"] = json!("alpha");
    seed_planning_candidate(
        &admin,
        "plan-b",
        json!([{"op":"create_task","after":added}]),
    )
    .await;
    let mut hidden_future = added.clone();
    hidden_future["workstream"] = json!("private-beta");
    seed_planning_candidate(
        &admin,
        "PRIVATE-future",
        json!([{"op":"create_task","after":hidden_future}]),
    )
    .await;
    let mut removed = current.clone();
    removed["definition_state"] = json!("cancelled");
    seed_planning_candidate(
        &admin,
        "plan-c",
        json!([{"op":"cancel","before":current,"after":removed}]),
    )
    .await;
    seed_planning_candidate(
        &admin,
        "plan-d",
        json!([{"op":"edit_fields","before":current,"after":current}]),
    )
    .await;
    seed_planning_candidate(
        &admin,
        "PRIVATE-plan",
        json!([{"op":"edit_fields","before":private,"after":private}]),
    )
    .await;
    seed_planning_candidate(
        &admin,
        "PRIVATE-mixed",
        json!([
        {"op":"edit_fields","before":current,"after":current},
        {"op":"edit_fields","before":private,"after":private}]),
    )
    .await;
    let result = store
        .query(TENANT, PROJECT, A, query("planning.candidates"))
        .await
        .unwrap();
    let data = &result["data"];
    assert_eq!(data["total"], 4);
    assert_eq!(data["truncated"], false);
    assert!(data["next_cursor"].is_null());
    assert_eq!(data["execution_authorized"], false);
    assert_eq!(
        data["as_of"]["source_snapshot_id"],
        result["source_snapshot_id"]
    );
    let items = data["items"].as_array().unwrap();
    for (item, kind) in items
        .iter()
        .zip(["changed", "added", "removed", "unchanged"])
    {
        assert_eq!(item["current_diff"]["items"][0]["kind"], kind);
        assert_eq!(item["current_diff"]["counts"][kind], 1);
        assert_eq!(item["author"]["display_name"], "Worker");
        assert!(item["created_at_unix_ms"].is_i64());
        assert_eq!(item["approval"]["reason"], "not_approved");
        assert_eq!(item["rejection"]["reason"], "not_rejected");
    }
    let fields = items[0]["current_diff"]["items"][0]["fields"]
        .as_array()
        .unwrap();
    assert_eq!(
        fields.len(),
        1,
        "the obsolete before-image must not create a false current difference"
    );
    assert_eq!(fields[0]["field"], "title");
    assert_eq!(fields[0]["before"]["value"], current["title"]);
    assert!(!result.to_string().contains("PRIVATE"));
    let mut scoped = query("planning.candidates");
    scoped.workstream_id = Some(Id::from(1));
    let read = store
        .query(TENANT, PROJECT, A, scoped.clone())
        .await
        .unwrap();
    assert_eq!(read["data"]["total"], 4);
    assert_eq!(read["workstream_id"], Id::from(1).to_string());
    scoped.workstream_id = Some(Id::from(2));
    assert!(matches!(
        store.query(TENANT, PROJECT, A, scoped).await,
        Err(PgError::Forbidden)
    ));
    let hidden = store
        .query(TENANT, PROJECT, B, query("planning.candidates"))
        .await
        .unwrap();
    assert_eq!(hidden["data"]["total"], 2);
    assert_eq!(hidden["data"]["items"][0]["candidate_id"], "PRIVATE-future");
}

#[tokio::test]
async fn planning_observation_retains_exact_approval_publication_and_activation_records() {
    let (_guard, admin, _, store) = setup().await;
    let draft = planning_draft(&admin, "a").await;
    seed_planning_candidate(
        &admin,
        "plan-approved",
        json!([{"op":"edit_fields","before":draft,"after":draft}]),
    )
    .await;
    admin.batch_execute("INSERT INTO awr_team.planning_approvals(tenant_id,project_id,id,candidate_id,
        candidate_digest,draft_revision,approver_person_id,approver_actor_id,approver_client_id,self_approved)
        SELECT tenant_id,project_id,'approval',id,candidate_digest,draft_revision,'reviewer','reviewer','review-cli',false
        FROM awr_team.planning_candidates;
        INSERT INTO awr_team.planning_publish_receipts(tenant_id,project_id,id,candidate_id,candidate_digest,
        draft_revision,approval_id,publisher_actor_id,publisher_client_id)
        SELECT tenant_id,project_id,'publication',id,candidate_digest,draft_revision,'approval','agent','cli-a'
        FROM awr_team.planning_candidates;
        UPDATE awr_team.planning_candidates SET state='published'").await.unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='reader',business_roles='[\"observer\"]'
        WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND actor_id='agent'",
        )
        .await
        .unwrap();
    let query_plan = || query("planning.candidates");
    let pending = store.query(TENANT, PROJECT, A, query_plan()).await.unwrap();
    let item = &pending["data"]["items"][0];
    assert_eq!(item["approval"]["value"]["display_name"], "Reviewer");
    assert_eq!(item["approval"]["value"]["self_approved"], false);
    assert_eq!(
        item["publication"]["value"]["source_writeback_pending"],
        true
    );
    assert_eq!(item["activation"]["reason"], "not_applied");
    admin.batch_execute("INSERT INTO awr_team.planning_activation_receipts(tenant_id,project_id,id,request_id,
        candidate_id,candidate_digest,publish_receipt_id,approval_id,approver_actor_id,publisher_actor_id,
        source_version,activated_snapshot_id,authority_epoch,before_fingerprint,after_fingerprint,
        affected_work_ids,unrelated_work_ids,audit_json)
        SELECT c.tenant_id,c.project_id,'activation','activation-request',c.id,c.candidate_digest,
        'publication','approval','reviewer','agent','source-version',p.active_snapshot_id,p.authority_epoch::text,
        'before','after','[\"a\"]','[]','{}' FROM awr_team.planning_candidates c
        JOIN awr_team.projects p ON p.tenant_id=c.tenant_id AND p.id=c.project_id;
        UPDATE awr_team.planning_publish_receipts SET source_writeback_pending=false").await.unwrap();
    let applied = store.query(TENANT, PROJECT, A, query_plan()).await.unwrap();
    let item = &applied["data"]["items"][0];
    assert_eq!(
        item["publication"]["value"]["source_writeback_pending"],
        false
    );
    assert_eq!(
        item["activation"]["value"]["matches_current_snapshot"],
        true
    );
    // A later draft cannot borrow the old version's approval or publication.
    admin.batch_execute("UPDATE awr_team.planning_candidates SET draft_revision=2,state='drafting',updated_at=clock_timestamp()").await.unwrap();
    let edited = store.query(TENANT, PROJECT, A, query_plan()).await.unwrap();
    let item = &edited["data"]["items"][0];
    assert_eq!(item["approval"]["reason"], "not_approved");
    assert_eq!(item["publication"]["reason"], "not_published");
    assert_eq!(item["activation"]["reason"], "not_applied");
}

#[tokio::test]
async fn planning_observation_pages_expire_on_visible_changes_and_redact_dependencies() {
    let (_guard, admin, _, store) = setup().await;
    let draft = planning_draft(&admin, "c").await;
    let mut after = draft.clone();
    after["required_dependencies"] = json!(["a"]);
    let change = json!([{"op":"edit_fields","before":draft,"after":after}]);
    for id in ["plan-a", "plan-b"] {
        seed_planning_candidate(&admin, id, change.clone()).await;
    }
    let mut q = query("planning.candidates");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["truncated"], true);
    let field = &first["data"]["items"][0]["current_diff"]["items"][0]["fields"][0];
    assert_eq!(field["before"]["reason"], "permission_restricted");
    assert_eq!(field["after"]["value"], json!(["a"]));
    assert!(!first.to_string().contains("b-private"));
    q.cursor = Some(first["data"]["next_cursor"].as_str().unwrap().into());
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(second["data"]["items"][0]["candidate_id"], "plan-b");
    assert!(second["data"]["next_cursor"].is_null());
    admin.batch_execute("UPDATE awr_team.planning_candidates SET state='superseded',updated_at=clock_timestamp() WHERE id='plan-a'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    let mut small = query("planning.candidates");
    small.max_context_bytes = Some(20);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, small).await,
        Err(PgError::ResponseTooLarge)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .query(TENANT, PROJECT, A, query("planning.candidates"))
            .await,
        Err(PgError::Forbidden)
    ));
}
use serde_json::{Value, json};

#[tokio::test]
async fn list_summaries_share_exact_version_bound_integration_state_with_snapshots() {
    let f = integration_fixture::setup_content_integration().await;
    let prepared = f.prepared().await;
    let id = prepared["integration_id"].as_str().unwrap();
    for state in ["prepared", "rejected"] {
        f.admin
            .execute(
                "UPDATE awr_team.delivery_integration_intents SET state=$2 WHERE id=$1",
                &[&id, &state],
            )
            .await
            .unwrap();
        let list = f
            .reads
            .query(TENANT, PROJECT, A, query("work.list"))
            .await
            .unwrap();
        let item = &list["data"]["items"][0];
        let mut q = query("work.snapshot");
        q.work_id = Some("a".into());
        let detail = f.reads.query(TENANT, PROJECT, A, q).await.unwrap();
        let integration = &item["summary"]["outcome"]["value"]["integration"];
        assert_eq!(
            integration,
            &detail["data"]["observation"]["collaboration"]["integration"]
        );
        assert_eq!(integration["state"], state);
        assert_eq!(integration["current_binding"], true);
        if state == "rejected" {
            assert_eq!(
                item["summary"]["blocker"]["value"]["kind"],
                "integration_rejected"
            );
        }
    }
    // A rejection of a different version remains history, never a current blocker.
    f.admin.execute("UPDATE awr_team.delivery_integration_intents
        SET request_json=jsonb_set(request_json,'{prepare,selection_version}','\"earlier\"') WHERE id=$1",&[&id]).await.unwrap();
    let changed = f
        .reads
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    let summary = &changed["data"]["items"][0]["summary"];
    assert_eq!(
        summary["outcome"]["value"]["integration"]["current_binding"],
        false
    );
    assert!(summary["blocker"]["value"].is_null());
}

trait PgWire: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> PgWire for T {}

// Count actual PostgreSQL statement executions, without instrumenting product code
// or requiring a server extension. Each test still uses its own isolated database.
async fn counted_pg(
    upstream: tokio_postgres::Config,
    count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> (u16, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let upstream = upstream.clone();
            let count = count.clone();
            tokio::spawn(async move {
                socket.set_nodelay(true).unwrap();
                let port = upstream.get_ports().first().copied().unwrap_or(5432);
                let remote: Box<dyn PgWire> = match &upstream.get_hosts()[0] {
                    tokio_postgres::config::Host::Tcp(host) => {
                        let host = upstream
                            .get_hostaddrs()
                            .first()
                            .map(|ip| ip.to_string())
                            .unwrap_or_else(|| host.clone());
                        let stream = tokio::net::TcpStream::connect((host.as_str(), port))
                            .await
                            .unwrap();
                        stream.set_nodelay(true).unwrap();
                        Box::new(stream)
                    }
                    #[cfg(unix)]
                    tokio_postgres::config::Host::Unix(path) => Box::new(
                        tokio::net::UnixStream::connect(path.join(format!(".s.PGSQL.{port}")))
                            .await
                            .unwrap(),
                    ),
                };
                let (mut incoming, mut outgoing) = socket.into_split();
                let (mut responses, mut requests) = tokio::io::split(remote);
                let forward = async {
                    let length = incoming.read_u32().await?;
                    let mut startup = vec![0; length as usize - 4];
                    incoming.read_exact(&mut startup).await?;
                    requests.write_u32(length).await?;
                    requests.write_all(&startup).await?;
                    loop {
                        let kind = incoming.read_u8().await?;
                        let length = incoming.read_u32().await?;
                        assert!((4..=4 * 1024 * 1024).contains(&length));
                        let mut body = vec![0; length as usize - 4];
                        incoming.read_exact(&mut body).await?;
                        if kind == b'E' || kind == b'Q' {
                            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        requests.write_u8(kind).await?;
                        requests.write_u32(length).await?;
                        requests.write_all(&body).await?;
                        if kind == b'X' {
                            return Ok::<(), std::io::Error>(());
                        }
                    }
                };
                tokio::select! {_=forward=>{},_=tokio::io::copy(&mut responses,&mut outgoing)=>{}}
            });
        }
    });
    (port, task)
}

#[tokio::test]
async fn list_summaries_have_a_constant_number_of_database_round_trips() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_guard, _, database, _) = setup().await;
    let upstream = common::with_app_role(&common::test_config(), &database);
    let count = Arc::new(AtomicUsize::new(0));
    let (port, proxy) = counted_pg(upstream.clone(), count.clone()).await;
    let mut counts = Vec::new();
    for limit in [1, 2] {
        let mut config = tokio_postgres::Config::new();
        config
            .host("127.0.0.1")
            .port(port)
            .user("awr_app")
            .password("app-test")
            .dbname(&database);
        let store = awr_team_pg::WorkstreamReadStore::from_config(config);
        let mut q = query("work.list");
        q.limit = Some(limit);
        count.store(0, Ordering::SeqCst);
        let result = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert_eq!(
            result["data"]["items"].as_array().unwrap().len(),
            limit as usize
        );
        counts.push(count.load(Ordering::SeqCst));
        drop(store);
    }
    proxy.abort();
    println!("work.list statement counts including authentication and transaction: {counts:?}");
    assert_eq!(
        counts[0], counts[1],
        "adding items must not add database round trips"
    );
    assert!(counts[0] <= 20, "bounded authenticated read: {counts:?}");
}

#[tokio::test]
async fn list_execution_liveness_stays_batched_on_a_full_page() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_guard, admin, database, _) = setup().await;
    // Grow the readable stream to 40 works; none of them may add statements.
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
         SELECT tenant_id,project_id,'bulk-'||g,'bulk-'||g
           FROM awr_team.work_items,generate_series(1,38) g WHERE id='a';
        INSERT INTO awr_team.work_contracts(tenant_id,project_id,snapshot_id,scope_id,work_id,
          contract_hash,definition_state,title,contract_json)
         SELECT tenant_id,project_id,snapshot_id,scope_id,'bulk-'||g,contract_hash,definition_state,title,contract_json
           FROM awr_team.work_contracts,generate_series(1,38) g WHERE work_id='a';
        INSERT INTO awr_team.workstream_snapshot_ownership(tenant_id,project_id,snapshot_id,scope_id,
          work_id,workstream_id,ownership_version)
         SELECT tenant_id,project_id,snapshot_id,scope_id,'bulk-'||g,workstream_id,ownership_version
           FROM awr_team.workstream_snapshot_ownership,generate_series(1,38) g WHERE work_id='a';",
        )
        .await
        .unwrap();
    let upstream = common::with_app_role(&common::test_config(), &database);
    let count = Arc::new(AtomicUsize::new(0));
    let (port, proxy) = counted_pg(upstream.clone(), count.clone()).await;
    let mut counts = Vec::new();
    for limit in [1, 40] {
        let mut config = tokio_postgres::Config::new();
        config
            .host("127.0.0.1")
            .port(port)
            .user("awr_app")
            .password("app-test")
            .dbname(&database);
        let store = awr_team_pg::WorkstreamReadStore::from_config(config);
        let mut q = query("work.list");
        q.limit = Some(limit);
        count.store(0, Ordering::SeqCst);
        let result = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert_eq!(
            result["data"]["items"].as_array().unwrap().len(),
            limit as usize
        );
        // Every page reports each work's liveness, batched or not.
        assert!(
            result["data"]["items"][0]["summary"]["execution"]
                .get("value")
                .is_some()
        );
        counts.push(count.load(Ordering::SeqCst));
        drop(store);
    }
    proxy.abort();
    println!("work.list statement counts for pages of 1 and 40 works: {counts:?}");
    // Both reads use the same proxy and always include liveness, so a build
    // without this field cannot be compared in one test. The one-statement
    // allowance covers page-level planning variance only; per-work execution
    // lookups would exceed it immediately on this 40-work page.
    assert!(
        counts[1] <= counts[0] + 1,
        "a 40-work page must add no per-work statements: {counts:?}"
    );
}

#[tokio::test]
async fn list_summaries_match_snapshots_and_keep_latest_actual_report() {
    use serde_json::json;
    let (_guard, admin, _, store) = setup().await;
    let contract = prepare(&store, A, "a").await["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    let report = json!({"phase":"testing","summary":"Sign-in works; checking expiry",
        "completed":["Sign-in screen","Session renewal"],
        "blockers":["Waiting for the review environment"],"artifacts":[],"tests":[]});
    admin
        .execute(
            "UPDATE awr_team.checkpoints SET contract_hash=$1,progress_json=$2,
        next_action='Check interrupted renewal',created_at='2026-11-01T12:00:00Z'
        WHERE id='cp-session-a'",
            &[&contract, &report],
        )
        .await
        .unwrap();
    admin.batch_execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','reader-project','responsible','Maya','active');
        INSERT INTO awr_team.task_responsibilities(tenant_id,project_id,work_id,owner_person_id,version)
        VALUES('reader-tenant','reader-project','a','responsible',1);
        UPDATE awr_team.work_runtime SET state='in_progress' WHERE work_id='a';
        INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,
            observed_revision,next_action,open_loops_json,created_at)
        VALUES('reader-tenant','reader-project','later-journal','session-a','context','contract',2,
            'Ordinary journal entry','[]','2026-11-02T12:00:00Z');
        UPDATE awr_team.sessions SET latest_checkpoint_id='later-journal' WHERE id='session-a'").await.unwrap();
    let list = store
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    for item in list["data"]["items"].as_array().unwrap() {
        let mut q = query("work.snapshot");
        q.work_id = Some(item["work_id"].as_str().unwrap().into());
        let snapshot = store.query(TENANT, PROJECT, A, q).await.unwrap();
        let observation = &snapshot["data"]["observation"];
        let summary = &item["summary"];
        assert_eq!(
            summary["owner"]["value"]
                .get("person_id")
                .unwrap_or(&json!(null)),
            &observation["responsibility"]["owner_person_id"]
        );
        assert_eq!(
            summary["owner"]["value"]
                .get("display_name")
                .unwrap_or(&json!(null)),
            &observation["responsibility"]["owner_name"]
        );
        assert_eq!(summary["runtime"]["value"], observation["runtime"]["state"]);
        assert_eq!(summary["phase"]["value"], observation["progress"]["phase"]);
        assert_eq!(
            summary["next_action"]["value"],
            observation["progress"]["next_action"]
        );
        assert_eq!(
            summary["outcome"]["value"]["completed"],
            observation["progress"]["completed"]
        );
        assert_eq!(
            summary["outcome"]["value"]["integration"],
            observation["collaboration"]["integration"]
        );
        for field in [
            "owner",
            "phase",
            "runtime",
            "execution",
            "outcome",
            "blocker",
            "next_action",
            "report",
        ] {
            let value = &summary[field];
            assert!(value.get("value").is_some(), "missing {field}");
            if value["value"].is_null() {
                assert!(value["as_of_unix_ms"].is_null());
                assert!(value["reason"].is_string(), "missing reason for {field}");
            } else {
                assert!(value["as_of_unix_ms"].is_i64());
                assert!(value["reason"].is_null());
            }
        }
    }
    let summary = &list["data"]["items"][0]["summary"];
    assert_eq!(summary["owner"]["value"]["display_name"], "Maya");
    assert_eq!(summary["phase"]["value"], "testing");
    assert_eq!(summary["blocker"]["value"]["text"], report["blockers"][0]);
    assert_eq!(summary["next_action"]["value"], "Check interrupted renewal");
    assert_eq!(summary["phase"]["as_of_unix_ms"], 1793534400000_i64);
    // Liveness follows the latest checkpoint of the work's session, here the
    // ordinary journal entry, and never the older progress report's time.
    assert_eq!(summary["execution"]["value"]["session_state"], "active");
    assert_eq!(
        summary["execution"]["value"]["last_checkpoint_at_unix_ms"],
        1793620800000_i64
    );
    assert_eq!(
        summary["outcome"]["as_of_unix_ms"],
        summary["phase"]["as_of_unix_ms"]
    );
    assert!(!list.to_string().contains("PRIVATE"));
    // Changing the reported phase must change both reads, including after an ordinary checkpoint.
    admin.batch_execute("UPDATE awr_team.checkpoints SET progress_json=jsonb_set(progress_json,'{phase}','\"blocked\"')
        WHERE id='cp-session-a'").await.unwrap();
    let changed = store
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    assert_eq!(
        changed["data"]["items"][0]["summary"]["phase"]["value"],
        "blocked"
    );
    let mut q = query("work.snapshot");
    q.work_id = Some("a".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["observation"]["progress"]["phase"],
        "blocked"
    );
}

#[tokio::test]
async fn list_summaries_report_execution_liveness_of_the_session_of_record() {
    let (_guard, admin, _, store) = setup().await;
    admin
        .execute(
            "UPDATE awr_team.checkpoints SET created_at='2026-11-01T12:00:00Z'
        WHERE id='cp-session-a'",
            &[],
        )
        .await
        .unwrap();
    let list = store
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    // A live session with a checkpoint reports active plus that time.
    let a = &work_item(&list, "a")["summary"]["execution"];
    assert_eq!(a["value"]["session_state"], "active");
    assert_eq!(a["value"]["last_checkpoint_at_unix_ms"], 1793534400000_i64);
    assert!(a["as_of_unix_ms"].is_i64());
    assert!(a["reason"].is_null());
    assert!(
        !a.to_string().contains("session-a"),
        "session ids must not appear in the summary"
    );
    // No session under the current ownership is an explicit no_session.
    let c = &work_item(&list, "c")["summary"]["execution"];
    assert!(c["value"].is_null());
    assert!(c["as_of_unix_ms"].is_null());
    assert_eq!(c["reason"], "no_session");
    // work.search carries the same fact on the same visibility boundary.
    let mut search = query("work.search");
    search.search = Some("a".into());
    let searched = store.query(TENANT, PROJECT, A, search).await.unwrap();
    assert_eq!(searched["data"]["total"], 1);
    let found = &searched["data"]["items"][0]["summary"]["execution"];
    assert_eq!(found["value"]["session_state"], "active");
    assert_eq!(
        found["value"]["last_checkpoint_at_unix_ms"],
        a["value"]["last_checkpoint_at_unix_ms"]
    );
    // Without claims, an older active developer session still wins over a
    // newer ended reviewer session, and the checkpoint time stays with it.
    let stream = Id::from(1).to_string();
    admin
        .execute(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
             VALUES($1,$2,'zz-newer-attempt','main','a','agent','cli-a','newer-review','ended',$3,1)",
            &[&TENANT, &PROJECT, &stream],
        )
        .await
        .unwrap();
    admin
        .batch_execute(
            "INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,
            observed_revision,next_action,open_loops_json,created_at)
            VALUES('reader-tenant','reader-project','cp-newer-attempt','zz-newer-attempt','context','contract',2,
            'Newer attempt journal','[]','2026-11-03T12:00:00Z');
            UPDATE awr_team.sessions SET latest_checkpoint_id='cp-newer-attempt' WHERE id='zz-newer-attempt'",
        )
        .await
        .unwrap();
    let active = store
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    let a = &work_item(&active, "a")["summary"]["execution"];
    assert_eq!(a["value"]["session_state"], "active");
    assert_eq!(a["value"]["last_checkpoint_at_unix_ms"], 1793534400000_i64);
    assert!(a["as_of_unix_ms"].is_i64());
    // A live claim selects the developer's active session before both the
    // newer ended reviewer and another active session with a newer checkpoint.
    // The third session makes claim precedence distinguishable from active-state
    // precedence: without the claim ordering, the newer checkpoint would win.
    admin
        .batch_execute(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
             VALUES('reader-tenant','reader-project','yy-newer-active','main','a','agent','cli-a','parallel-work','active',
             (SELECT workstream_id FROM awr_team.sessions WHERE id='session-a'),1);
            INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,
             observed_revision,next_action,open_loops_json,created_at)
             VALUES('reader-tenant','reader-project','cp-newer-active','yy-newer-active','context','contract',3,
             'Parallel active journal','[]','2026-11-04T12:00:00Z');
            UPDATE awr_team.sessions SET latest_checkpoint_id='cp-newer-active' WHERE id='yy-newer-active';
            INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,
             fence,acquired_at,expires_at,state,workstream_id,ownership_version,coordinator_epoch)
             VALUES('reader-tenant','reader-project','liveness-claim','main','a','session-a','agent',1,
             clock_timestamp(),clock_timestamp()+interval '1 hour','active',
             (SELECT workstream_id FROM awr_team.sessions WHERE id='session-a'),1,'epoch-a')",
        )
        .await
        .unwrap();
    let claimed = store
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    let a = &work_item(&claimed, "a")["summary"]["execution"];
    assert_eq!(a["value"]["session_state"], "active");
    assert_eq!(a["value"]["last_checkpoint_at_unix_ms"], 1793534400000_i64);
    // With only terminal sessions, each non-active state reads as ended and
    // the newest checkpoint determines which session supplies the timestamp.
    admin
        .execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'
             WHERE tenant_id=$1 AND project_id=$2 AND id='liveness-claim'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    for state in ["ended", "interrupted", "incomplete"] {
        admin
            .execute(
                "UPDATE awr_team.sessions SET state=$3
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'",
                &[&TENANT, &PROJECT, &state],
            )
            .await
            .unwrap();
        let list = store
            .query(TENANT, PROJECT, A, query("work.list"))
            .await
            .unwrap();
        let execution = &work_item(&list, "a")["summary"]["execution"];
        assert_eq!(
            execution["value"]["session_state"], "ended",
            "{state} must read as ended"
        );
        assert_eq!(
            execution["value"]["last_checkpoint_at_unix_ms"],
            1793793600000_i64
        );
    }
    // A session superseded by a newer ownership version never counts.
    admin
        .batch_execute(
            "UPDATE awr_team.sessions SET ownership_version=ownership_version+1
        WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND work_id='a'",
        )
        .await
        .unwrap();
    let bumped = store
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    let a = &work_item(&bumped, "a")["summary"]["execution"];
    assert!(a["value"].is_null());
    assert!(a["as_of_unix_ms"].is_null());
    assert_eq!(a["reason"], "no_session");
    // The visibility boundary is unchanged: reader B still sees only its own
    // work, with its own liveness fact and none of A's sessions.
    let b = store
        .query(TENANT, PROJECT, B, query("work.list"))
        .await
        .unwrap();
    assert_eq!(b["data"]["total"], 1);
    assert_eq!(b["data"]["items"][0]["work_id"], "b-private");
    assert_eq!(
        b["data"]["items"][0]["summary"]["execution"]["value"]["session_state"],
        "active"
    );
    assert!(!b.to_string().contains("session-a"));
    assert!(!b.to_string().contains("zz-newer-attempt"));
    assert!(!b.to_string().contains("yy-newer-active"));
}

#[tokio::test]
async fn list_summaries_keep_null_reasons_permissions_search_and_pagination() {
    let (_guard, admin, _, store) = setup().await;
    let mut q = query("work.list");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 2);
    assert_eq!(
        first["data"]["items"][0]["summary"]["phase"]["reason"],
        "client_capability_unknown"
    );
    assert_eq!(
        first["data"]["items"][0]["summary"]["execution"]["value"]["session_state"],
        "active"
    );
    q.cursor = Some(first["data"]["next_cursor"].as_str().unwrap().into());
    let second = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(second["data"]["items"][0]["work_id"], "c");
    assert_eq!(
        second["data"]["items"][0]["summary"]["phase"]["reason"],
        "no_session"
    );
    assert_eq!(
        second["data"]["items"][0]["summary"]["execution"]["reason"],
        "no_session"
    );
    assert!(second["data"]["next_cursor"].is_null());
    let mut search = query("work.search");
    search.search = Some("alpha".into());
    let result = store.query(TENANT, PROJECT, A, search).await.unwrap();
    assert!(!result.to_string().contains("PRIVATE"));
    let private = store
        .query(TENANT, PROJECT, B, query("work.list"))
        .await
        .unwrap();
    assert_eq!(private["data"]["total"], 1);
    assert_eq!(private["data"]["items"][0]["work_id"], "b-private");
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='reader',business_roles='[\"observer\"]'
        WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND actor_id='agent'",
        )
        .await
        .unwrap();
    let observer = store
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    assert_eq!(observer["data"]["total"], 2);
    assert!(!observer.to_string().contains("PRIVATE"));
    for (tenant, token) in [("other-tenant", A), (TENANT, NONE)] {
        assert!(matches!(
            store
                .query(tenant, PROJECT, token, query("work.list"))
                .await,
            Err(PgError::Forbidden)
        ));
    }
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn progress_graph_pages_keep_scope_dependencies_and_revision() {
    let (_guard, admin, _, store) = setup().await;
    let row = admin
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='c'",
            &[],
        )
        .await
        .unwrap();
    let mut contract: awr_team::WorkContract = serde_json::from_value(row.get(0)).unwrap();
    contract.required_dependencies.push("a".into());
    admin.execute(
        "UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='c'",
        &[&serde_json::to_value(&contract).unwrap(), &contract.hash().unwrap()],
    ).await.unwrap();
    let mut q = query("work.graph");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 2);
    assert_eq!(first["data"]["items"][0]["work_id"], "a");
    assert_eq!(first["data"]["execution_authorized"], false);
    assert!(!first.to_string().contains("PRIVATE"));
    q.cursor = Some(first["data"]["next_cursor"].as_str().unwrap().into());
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let node = &second["data"]["items"][0];
    assert_eq!(node["work_id"], "c");
    assert_eq!(node["required_dependencies"], serde_json::json!(["a"]));
    assert_eq!(node["dependencies_complete"], false);
    assert!(second["data"]["next_cursor"].is_null());
    assert!(!second.to_string().contains("b-private"));
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    admin
        .batch_execute("UPDATE awr_team.projects SET project_revision=project_revision+1")
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    let private = store
        .query(TENANT, PROJECT, B, query("work.graph"))
        .await
        .unwrap();
    assert_eq!(private["data"]["total"], 1);
    assert_eq!(private["data"]["items"][0]["work_id"], "b-private");
    assert!(matches!(
        store
            .query("other-tenant", PROJECT, A, query("work.graph"))
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn progress_graph_only_shows_live_current_claims_with_recorded_dates() {
    let (_guard, admin, _, store) = setup().await;
    let stream = Id::from(1).to_string();
    admin
        .execute(
            "INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,last_fence)
             VALUES($1,$2,'main','a','claimed',1)
             ON CONFLICT(tenant_id,project_id,scope_id,work_id) DO UPDATE SET last_fence=1",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    admin.execute(
        "INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,
         fence,acquired_at,expires_at,state,workstream_id,ownership_version,coordinator_epoch)
         VALUES($1,$2,'graph-claim','main','a','session-a','agent',1,
         '2026-01-01T12:00:00Z',clock_timestamp()+interval '1 hour','active',$3,1,'epoch-a')",
        &[&TENANT, &PROJECT, &stream],
    ).await.unwrap();
    let graph = store
        .query(TENANT, PROJECT, A, query("work.graph"))
        .await
        .unwrap();
    let claim = &graph["data"]["items"][0]["claims"][0];
    assert_eq!(claim["actor_name"], "Worker");
    assert_eq!(claim["acquired_at_unix_ms"], 1767268800000_i64);
    assert!(claim["expires_at_unix_ms"].as_i64().unwrap() > 1767268800000);
    for mutation in [
        "UPDATE awr_team.claims SET coordinator_epoch='old'",
        "UPDATE awr_team.claims SET coordinator_epoch='epoch-a',expires_at=clock_timestamp()-interval '1 second'",
        "UPDATE awr_team.claims SET expires_at=clock_timestamp()+interval '1 hour'; UPDATE awr_team.sessions SET state='ended'",
    ] {
        admin.batch_execute(mutation).await.unwrap();
        let graph = store
            .query(TENANT, PROJECT, A, query("work.graph"))
            .await
            .unwrap();
        assert_eq!(graph["data"]["items"][0]["claims"], serde_json::json!([]));
    }
}

#[tokio::test]
async fn work_snapshot_preserves_context_hash_scope_and_total_byte_budget() {
    let (_guard, _, _, store) = setup().await;
    let prepared = prepare(&store, A, "a").await;
    let mut q = query("work.snapshot");
    q.work_id = Some("a".into());
    let snapshot = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let data = &snapshot["data"];
    assert_eq!(data["context_hash"], prepared["data"]["context_hash"]);
    assert_eq!(snapshot["project_revision"], prepared["project_revision"]);
    assert_eq!(data["snapshot"]["consistency"], "repeatable_read");
    assert_eq!(
        data["snapshot"]["project_revision"],
        snapshot["project_revision"]
    );
    assert_eq!(
        data["snapshot"]["source_snapshot_id"],
        snapshot["source_snapshot_id"]
    );
    assert_eq!(
        data["snapshot"]["coordinator_epoch"],
        snapshot["coordinator_epoch"]
    );
    assert_eq!(data["observation"]["contract_hash"], data["contract_hash"]);
    assert_eq!(data["observation"]["execution_authorized"], false);
    assert_eq!(
        data["snapshot"]["queried_at_unix_ms"],
        data["observation"]["observed_at_unix_ms"]
    );
    let mut original = data.clone();
    original.as_object_mut().unwrap().remove("snapshot");
    original.as_object_mut().unwrap().remove("observation");
    assert_eq!(original, prepared["data"]);
    assert!(!snapshot.to_string().contains("PRIVATE"));
    let caps = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    assert!(
        caps["queries"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("work.snapshot"))
    );
    assert_eq!(caps["session_feedback"]["snapshot_read"], "work.snapshot");
    // What the Team entry does not provide of the personal entries is named, per layer, before anyone asks.
    let layers = caps["semantic_layers"]["layers"].as_object().unwrap();
    assert_eq!(
        layers.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "completion_validity",
            "context_completeness",
            "delivery_observation",
            "execution_admission",
            "work_readiness"
        ]
    );
    assert_eq!(
        caps["semantic_layers"]["unsupported_operation"],
        "Unsupported"
    );
    assert!(
        layers["context_completeness"]["without_team_operation"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["entry"] == "context compile")
    );

    // The opt-in operation cannot silently drop feedback to meet a context budget.
    q.max_context_bytes = Some(serde_json::to_vec(&prepared["data"]).unwrap().len());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::ContextIncomplete)
    ));
    q.op = "work.prepare".into();
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["context_hash"],
        data["context_hash"]
    );
    q.op = "work.snapshot".into();
    q.max_context_bytes = None;
    for work in ["b-private", "missing"] {
        q.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q.clone()).await,
            Err(PgError::Forbidden)
        ));
    }
    q.work_id = Some("a".into());
    assert!(matches!(
        store.query("other-tenant", PROJECT, A, q.clone()).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q.clone()).await,
        Err(PgError::Forbidden)
    ));
    q.session_id = Some("session-b".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn incomplete_snapshot_keeps_one_restoration_action_when_duplicate_advice_is_omitted() {
    let (_guard, _, _, store) = setup_with_specs(vec![]).await;
    let mut q = query("work.snapshot");
    q.work_id = Some("a".into());
    let full = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let data = &full["data"];
    assert_eq!(data["context_complete"], false);
    assert_eq!(data["guidance"]["code"], "restore_context");
    assert_eq!(data["guidance"], data["observation"]["guidance"]);
    assert!(data["observation"]["guidance"].to_string().len() < 900);
    let mut mandatory = data.clone();
    mandatory.as_object_mut().unwrap().remove("guidance");
    let budget = serde_json::to_vec(&mandatory).unwrap().len();
    q.max_context_bytes = Some(budget);
    let compact = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert!(compact["data"].get("guidance").is_none());
    assert_eq!(compact["data"]["observation"]["guidance"], data["guidance"]);
    assert_eq!(compact["data"]["context_hash"], data["context_hash"]);
    assert_eq!(
        compact["data"]["completeness_reasons"],
        data["completeness_reasons"]
    );
    assert_eq!(
        compact["data"]["observation"]["execution_authorized"],
        false
    );
    assert!(serde_json::to_vec(&compact["data"]).unwrap().len() <= budget);
}

#[tokio::test]
async fn work_snapshot_does_not_mix_runtime_revisions_during_concurrent_writes() {
    let (_guard, admin, db, store) = setup().await;
    admin.execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
        VALUES($1,$2,'main','a','ready',3,3)", &[&TENANT,&PROJECT]).await.unwrap();
    let mut writer = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let writing = tokio::spawn(async move {
        for _ in 0..32 {
            let tx = writer.transaction().await.unwrap();
            tx.execute("UPDATE awr_team.work_runtime SET work_version=work_version+1,last_fence=last_fence+1
                WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'", &[&TENANT,&PROJECT]).await.unwrap();
            tx.execute(
                "UPDATE awr_team.projects SET project_revision=project_revision+1
                WHERE tenant_id=$1 AND id=$2",
                &[&TENANT, &PROJECT],
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });
    for _ in 0..24 {
        let mut q = query("work.snapshot");
        q.work_id = Some("a".into());
        let snapshot = store.query(TENANT, PROJECT, A, q).await.unwrap();
        let data = &snapshot["data"];
        assert_eq!(data["runtime"], data["observation"]["runtime"]);
        assert_eq!(
            data["runtime"]["work_version"],
            snapshot["project_revision"]
        );
        assert_eq!(data["runtime"]["last_fence"], snapshot["project_revision"]);
    }
    writing.await.unwrap();
    let mut q = query("work.snapshot");
    q.work_id = Some("a".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["project_revision"],
        "35"
    );
}

#[tokio::test]
async fn work_observation_is_scoped_current_and_does_not_change_context_or_authority() {
    let (_guard, admin, _, store) = setup().await;
    let prepared = prepare(&store, A, "a").await;
    let mut q = query("work.observe");
    q.work_id = Some("a".into());
    let observed = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let data = &observed["data"];
    assert_eq!(data["session"]["id"], "session-a");
    assert_eq!(data["session"]["client_id"], "cli-a");
    assert!(data["session"].get("conversation_id").is_none());
    assert_eq!(data["checkpoint"]["next_action"], "continue alpha");
    assert_eq!(data["checkpoint"]["contract_matches_current"], false);
    assert!(data["model"].is_null());
    assert!(data["usage"].is_null());
    assert_eq!(data["execution_authorized"], false);
    assert!(!observed.to_string().contains("PRIVATE"));
    assert_eq!(prepared["data"], prepare(&store, A, "a").await["data"]);
    assert_eq!(prepared["project_revision"], observed["project_revision"]);
    admin.batch_execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        SELECT tenant_id,project_id,'zz-newer-closed-supervisor',scope_id,work_id,actor_id,client_id,'closed-review','ended',workstream_id,ownership_version
        FROM awr_team.sessions WHERE id='session-a'").await.unwrap();
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["session"]["id"],
        "session-a"
    );
    for work in ["b-private", "missing"] {
        q.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q.clone()).await,
            Err(PgError::Forbidden)
        ));
    }
    q.work_id = Some("a".into());
    admin
        .batch_execute("UPDATE awr_team.sessions SET ownership_version=2 WHERE work_id='a'")
        .await
        .unwrap();
    let moved = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert!(moved["data"]["session"].is_null());
    assert!(moved["data"]["checkpoint"].is_null());
}

#[tokio::test]
async fn credentials_and_project_membership_do_not_grant_other_clients_scopes() {
    let (_guard, _, _, store) = setup().await;
    let a = store
        .query(TENANT, PROJECT, A, query("workstreams.list"))
        .await
        .unwrap();
    assert_eq!(a["total"], 1);
    assert!(!a.to_string().contains("private-beta"));
    let b = store
        .query(TENANT, PROJECT, B, query("work.list"))
        .await
        .unwrap();
    assert_eq!(b["data"]["total"], 1);
    assert_eq!(b["data"]["items"][0]["work_id"], "b-private");
    for token in [NONE, "garbage", &A.replace('a', "d")] {
        assert!(matches!(
            store
                .query(TENANT, PROJECT, token, query("capabilities"))
                .await,
            Err(PgError::Forbidden)
        ));
    }
    assert!(matches!(
        store
            .query("other-tenant", PROJECT, A, query("capabilities"))
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn protocol_discovery_requires_authentication_and_a_readable_scope() {
    let (_guard, _, _, store) = setup().await;
    for token in ["garbage", NONE] {
        assert!(matches!(
            store
                .query(TENANT, PROJECT, token, query("claim.acquire"))
                .await,
            Err(PgError::Forbidden)
        ));
    }
    assert!(matches!(
        store
            .query(TENANT, PROJECT, A, query("claim.acquire"))
            .await,
        Err(PgError::Unsupported(_))
    ));
}

#[tokio::test]
async fn hidden_and_missing_work_or_session_have_the_same_denial() {
    let (_guard, _, _, store) = setup().await;
    for work in ["b-private", "missing"] {
        let mut q = query("work.prepare");
        q.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    for session in ["session-b", "absent"] {
        let mut q = query("session.inspect");
        q.session_id = Some(session.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    let mut mismatch = query("work.prepare");
    mismatch.work_id = Some("a".into());
    mismatch.workstream_id = Some(Id::from(2));
    assert!(store.query(TENANT, PROJECT, A, mismatch).await.is_err());
}

#[tokio::test]
async fn scoped_count_search_pagination_and_cursors_never_include_hidden_works() {
    let (_guard, admin, _, store) = setup().await;
    let mut q = query("work.list");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 2);
    q.cursor = Some(first["data"]["next_cursor"].as_str().unwrap().into());
    let next = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(next["data"]["items"][0]["work_id"], "c");
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    admin.batch_execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    let mut q = query("work.search");
    q.search = Some("private".into());
    let search = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(search["data"]["total"], 0);
}

#[tokio::test]
async fn dependency_exports_are_opaque_and_required_context_is_never_cut_to_budget() {
    let (_guard, _, _, store) = setup().await;
    let mut q = query("work.prepare");
    q.work_id = Some("c".into());
    let result = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(result["data"]["context_complete"], false);
    assert_eq!(result["data"]["dependency_export_unavailable"], true);
    assert!(!result.to_string().contains("b-private"));
    assert!(result.to_string().contains("preserve compatibility"));
    q.max_context_bytes = Some(1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ContextIncomplete)
    ));
}

#[tokio::test]
async fn events_and_recovery_respect_attribution_and_ownership_generation() {
    let (_guard, admin, _, store) = setup().await;
    let events = store
        .query(TENANT, PROJECT, A, query("events.list"))
        .await
        .unwrap();
    assert_eq!(events["data"]["items"].as_array().unwrap().len(), 1);
    assert!(!events.to_string().contains("session-b"));
    let mut q = query("work.recovery");
    q.work_id = Some("a".into());
    let recovery = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(
        recovery["data"]["items"][0]["next_action"],
        "continue alpha"
    );
    assert!(!recovery.to_string().contains("PRIVATE"));
    assert_eq!(
        recovery["data"]["items"][0]["contract_matches_current"],
        false
    );
    let current = recovery["data"]["current_contract_hash"].as_str().unwrap();
    admin
        .execute(
            "UPDATE awr_team.checkpoints SET contract_hash=$1 WHERE session_id='session-a'",
            &[&current],
        )
        .await
        .unwrap();
    let matched = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(
        matched["data"]["items"][0]["contract_matches_current"],
        true
    );
    // A matching contract is only one recovery fact, never permission to resume.
    assert_eq!(matched["data"]["automatic_resume"], false);
    admin
        .batch_execute("UPDATE awr_team.sessions SET ownership_version=2 WHERE id='session-a'")
        .await
        .unwrap();
    assert!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut q = query("session.inspect");
    q.session_id = Some("session-a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn semantic_context_tracks_selected_runtime_without_global_audit_churn() {
    let (_guard, admin, _, store) = setup().await;
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    let before = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(before["data"]["context_complete"], true);
    assert_eq!(
        before["data"]["context_hash_protocol"],
        "awr-team-workstream-context-v1"
    );
    assert!(before["data"]["runtime"].is_null());
    admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version)
        VALUES('reader-tenant','reader-project','main','b-private','pending',1);
        UPDATE awr_team.projects SET project_revision=project_revision+1 WHERE tenant_id='reader-tenant';").await.unwrap();
    let unrelated = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_ne!(before["project_revision"], unrelated["project_revision"]);
    assert_eq!(before["data"], unrelated["data"]);
    admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,recovery_blocked)
        VALUES('reader-tenant','reader-project','main','a','blocked',2,true)").await.unwrap();
    let changed = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_ne!(
        changed["data"]["context_hash"],
        before["data"]["context_hash"]
    );
    assert_eq!(changed["data"]["runtime"]["work_version"], "2");
    assert_eq!(changed["data"]["runtime"]["recovery_blocked"], true);
    assert_eq!(changed["data"]["execution_admission"], "not_evaluated");
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET status='frozen' WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    let frozen = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(frozen["project_status"], "frozen");
    assert_ne!(
        frozen["data"]["context_hash"],
        changed["data"]["context_hash"]
    );
}

#[tokio::test]
async fn mismatched_event_attribution_stale_grants_and_oversized_recovery_fail_closed() {
    let (_guard, admin, _, store) = setup().await;
    admin.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json,workstream_id)
        VALUES($1,$2,'wrong-stream',4,0,'session.started','agent','b-private','{}',$3),
              ($1,$2,'unattributed',4,1,'session.started','agent','a','{}',NULL)",
        &[&TENANT,&PROJECT,&Id::from(1).to_string()]).await.unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET project_revision=4 WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    let events = store
        .query(TENANT, PROJECT, A, query("events.list"))
        .await
        .unwrap();
    assert_eq!(events["data"]["items"].as_array().unwrap().len(), 1);
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET authority_version=2 WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Forbidden)
    ));
    admin.batch_execute("UPDATE awr_team.workstream_grants SET authority_version=1 WHERE client_id='cli-a';
        UPDATE awr_team.checkpoints SET next_action=repeat('x',1048576) WHERE session_id='session-a'").await.unwrap();
    let mut q = query("work.recovery");
    q.work_id = Some("a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ResponseTooLarge)
    ));
}

async fn wait_for_lock(admin: &tokio_postgres::Client, query_fragment: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = admin
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)",
                    &[&format!("%{query_fragment}%")],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected real PostgreSQL row-lock wait");
}

#[tokio::test]
async fn revocation_and_reads_serialize_in_both_orders_without_stale_authority() {
    let (_guard, mut admin, db, store) = setup().await;
    let store = std::sync::Arc::new(store);
    let observer = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    // Revoke first. The reader began with an older repeatable-read snapshot
    // but must not return it after waiting on the credential row.
    let revoke = admin.transaction().await.unwrap();
    revoke
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    let reader = store.clone();
    let read =
        tokio::spawn(async move { reader.query(TENANT, PROJECT, A, query("work.list")).await });
    wait_for_lock(&observer, "FOR SHARE OF t,a,c,m").await;
    revoke.commit().await.unwrap();
    match read.await.unwrap() {
        Err(PgError::Forbidden) => {}
        Err(PgError::Db(e)) => assert_eq!(
            e.code(),
            Some(&tokio_postgres::error::SqlState::T_R_SERIALIZATION_FAILURE)
        ),
        other => panic!("revocation must prevent stale data: {other:?}"),
    }
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Forbidden)
    ));
    admin
        .batch_execute("UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'")
        .await
        .unwrap();
    // Read first. Hold a later grant row to observe the authenticated reader
    // retaining its credential lock until the whole query finishes.
    let hold = admin.transaction().await.unwrap();
    hold.query(
        "SELECT workstream_id FROM awr_team.workstream_grants WHERE client_id='cli-a' FOR UPDATE",
        &[],
    )
    .await
    .unwrap();
    let reader = store.clone();
    let read =
        tokio::spawn(async move { reader.query(TENANT, PROJECT, A, query("work.list")).await });
    wait_for_lock(&observer, "ORDER BY workstream_id FOR SHARE").await;
    let revoker = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let revoke = tokio::spawn(async move {
        revoker
            .batch_execute(
                "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            )
            .await
    });
    wait_for_lock(&observer, "UPDATE awr_team.credentials SET revoked_at").await;
    hold.rollback().await.unwrap();
    assert_eq!(read.await.unwrap().unwrap()["data"]["total"], 2);
    revoke.await.unwrap().unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn dynamic_revocation_expiry_actor_and_membership_changes_take_effect_on_next_read() {
    let (_guard, admin, _, store) = setup().await;
    assert!(
        store
            .query(TENANT, PROJECT, A, query("capabilities"))
            .await
            .is_ok()
    );
    for (disable, restore) in [
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'",
        ),
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='reader-a'",
        ),
        (
            "UPDATE awr_team.actors SET status='disabled' WHERE id='agent'",
            "UPDATE awr_team.actors SET status='active' WHERE id='agent'",
        ),
        (
            "UPDATE awr_team.tenants SET status='disabled' WHERE id='reader-tenant'",
            "UPDATE awr_team.tenants SET status='active' WHERE id='reader-tenant'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET active=false WHERE client_id='cli-a'",
            "UPDATE awr_team.workstream_grants SET active=true WHERE client_id='cli-a'",
        ),
    ] {
        admin.batch_execute(disable).await.unwrap();
        assert!(matches!(
            store.query(TENANT, PROJECT, A, query("capabilities")).await,
            Err(PgError::Forbidden)
        ));
        admin.batch_execute(restore).await.unwrap();
        assert!(
            store
                .query(TENANT, PROJECT, A, query("capabilities"))
                .await
                .is_ok()
        );
    }
    admin.batch_execute("DELETE FROM awr_team.workstream_grants WHERE actor_id='agent'; DELETE FROM awr_team.project_memberships WHERE actor_id='agent'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("capabilities")).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn multi_scope_choice_is_explicit_and_protocol_rejects_forged_authority_fields() {
    let (_guard, admin, _, store) = setup().await;
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-a',$3,1,true)",&[&TENANT,&PROJECT,&Id::from(2).to_string()]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Workstream(
            awr_core::WorkstreamError::ScopeRequired
        ))
    ));
    for field in ["actor_id", "tenant_id", "project_id", "client_id", "grants"] {
        let mut value = serde_json::json!({"protocol_version":1,"op":"work.list"});
        value[field] = serde_json::json!("forged");
        assert!(serde_json::from_value::<awr_team_pg::WorkstreamQuery>(value).is_err());
    }
    let mut unsupported = query("claim.acquire");
    unsupported.protocol_version = 1;
    assert!(matches!(
        store.query(TENANT, PROJECT, A, unsupported).await,
        Err(PgError::Unsupported(_))
    ));
}

#[tokio::test]
async fn future_code_paths_do_not_require_existing_source_documents() {
    let (_guard, _, _, store) = setup().await;
    let prepared = prepare(&store, A, "a").await;
    assert_eq!(
        prepared["data"]["visible_contract"]["scope_paths"],
        serde_json::json!(["src"])
    );
    assert_eq!(prepared["data"]["context_complete"], true);
    assert_eq!(prepared["data"]["required_specs"], serde_json::json!([]));
}

#[tokio::test]
async fn prepare_includes_only_the_selected_workstreams_required_specs() {
    let (_guard, _, _, store) = setup_with_specs(vec![
        awr_team_pg::SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"Public API contract".to_vec(),
        },
        awr_team_pg::SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"PRIVATE peer contract".to_vec(),
        },
    ])
    .await;
    let prepared = prepare(&store, A, "a").await;
    assert_eq!(prepared["data"]["context_complete"], true);
    assert_eq!(
        prepared["data"]["required_specs"][0]["path"],
        "docs/alpha.md"
    );
    assert_eq!(
        prepared["data"]["required_specs"][0]["text"],
        "Public API contract"
    );
    assert!(!prepared.to_string().contains("PRIVATE"));
    assert_eq!(
        prepared["data"]["visible_contract"]["scope_paths"],
        serde_json::json!(["src"])
    );
}

#[tokio::test]
async fn missing_declared_spec_still_blocks_context_even_when_code_is_not_created() {
    let (_guard, _, _, store) = setup_with_specs(vec![]).await;
    let prepared = prepare(&store, A, "a").await;
    assert_eq!(prepared["data"]["context_complete"], false);
    assert_eq!(
        prepared["data"]["completeness_reasons"],
        serde_json::json!(["required_spec_missing"])
    );
    assert_eq!(
        prepared["data"]["authorized_readable_refs"][0]["path"],
        "docs/alpha.md"
    );
}
