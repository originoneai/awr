use super::*;

async fn round(admin: &tokio_postgres::Client, id: &str, work: &str, index: i32, state: &str) {
    admin
        .execute(
            "INSERT INTO awr_team.review_rounds(tenant_id,project_id,id,work_id,round_index,
          state,bundle_hash,contract_hash,artifact_digest,author_actor_id)
         SELECT $1,$2,$3,$4,$5,$6,repeat('a',64),c.contract_hash,repeat('b',64),'agent'
         FROM awr_team.work_contracts c JOIN awr_team.projects p
           ON p.tenant_id=c.tenant_id AND p.id=c.project_id AND p.active_snapshot_id=c.snapshot_id
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$4 AND c.scope_id='main'",
            &[&TENANT, &PROJECT, &id, &work, &index, &state],
        )
        .await
        .unwrap();
}

async fn decision(admin: &tokio_postgres::Client, id: &str, round: &str, basis: &str) {
    admin.execute(
        "INSERT INTO awr_team.review_decisions(tenant_id,project_id,id,review_round_id,
         work_id,bundle_hash,reviewer_actor_id,decision,reason,approval_basis,independence_kind,created_at)
         SELECT tenant_id,project_id,$1,id,work_id,bundle_hash,'reviewer','approve',
         E'Contract verified\\nDetailed follow-up',$3,'unspecified','2026-10-01T10:00:00Z'
         FROM awr_team.review_rounds WHERE tenant_id=$4 AND project_id=$5 AND id=$2",
        &[&id,&round,&basis,&TENANT,&PROJECT],
    ).await.unwrap();
}

fn rounds_query() -> WorkstreamQuery {
    let mut q = query("review.rounds");
    q.work_id = Some("a".into());
    q
}

#[tokio::test]
async fn review_round_summaries_match_inspection_and_preserve_history_and_unknown_roles() {
    let (_guard, admin, _, store) = setup().await;
    for (index, state) in [
        (1, "approved"),
        (3, "rejected"),
        (7, "invalidated"),
        (9, "open"),
    ] {
        let id = format!("round-{index}");
        round(&admin, &id, "a", index, state).await;
    }
    decision(
        &admin,
        "decision-known",
        "round-1",
        "human_independent_review",
    )
    .await;
    decision(&admin, "decision-unknown", "round-3", "unspecified").await;
    admin
        .execute(
            "UPDATE awr_team.review_rounds SET contract_hash=repeat('c',64) WHERE id='round-7'",
            &[],
        )
        .await
        .unwrap();
    let result = store
        .query(TENANT, PROJECT, A, rounds_query())
        .await
        .unwrap();
    let data = &result["data"];
    assert_eq!(data["total"], 4);
    assert_eq!(data["execution_authorized"], false);
    assert_eq!(
        data["as_of"]["source_snapshot_id"],
        result["source_snapshot_id"]
    );
    assert!(data["as_of"]["at_unix_ms"].is_i64());
    for (position, item) in data["items"].as_array().unwrap().iter().enumerate() {
        assert_eq!(item["round_position"], position + 1);
        let mut q = query("review.inspect");
        q.work_id = Some("a".into());
        q.review_round_id = item["round_id"].as_str().map(Into::into);
        let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap();
        let inspected = &inspected["data"]["review"];
        for field in [
            "round_id",
            "round_index",
            "state",
            "bundle_hash",
            "contract_hash",
            "artifact_digest",
        ] {
            assert_eq!(
                item[field], inspected[field],
                "review summary diverged for {field}"
            );
        }
        if let Some(latest) = inspected["decisions"].as_array().unwrap().last() {
            for field in [
                "decision",
                "reviewer_role",
                "reviewer_actor_id",
                "decided_at_unix_ms",
            ] {
                assert_eq!(
                    item[field], latest[field],
                    "decision summary diverged for {field}"
                );
            }
            assert_eq!(
                item["reason_first_line"],
                latest["reason"].as_str().unwrap().lines().next().unwrap()
            );
        }
    }
    assert_eq!(data["items"][0]["reviewer_role"], "independent_reviewer");
    assert_eq!(data["items"][0]["role_basis"], "recorded_approval_basis");
    assert_eq!(data["items"][0]["reason_first_line"], "Contract verified");
    assert!(data["items"][1]["reviewer_role"].is_null());
    assert_eq!(data["items"][1]["missing"]["reviewer_role"], "not_recorded");
    assert_eq!(data["items"][2]["contract_matches_current"], false);
    assert!(data["items"][3]["decision"].is_null());
    assert!(!result.to_string().contains("Detailed follow-up"));
}

#[tokio::test]
async fn review_rounds_keep_unreadable_evidence_counted_without_disclosing_its_metadata() {
    let (_guard, admin, _, store) = setup().await;
    round(&admin, "round-one", "a", 1, "approved").await;
    round(&admin, "round-two", "a", 2, "open").await;
    admin
        .execute(
            "INSERT INTO awr_team.evidence(tenant_id,project_id,id,work_id,contract_hash,
        evidence_kind,trust_basis,digest,payload_json,created_by)
        VALUES($1,$2,'private-evidence','b-private','PRIVATE CONTRACT','report','caller_asserted',
          'PRIVATE DIGEST','{}','agent')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    admin.execute("UPDATE awr_team.review_rounds SET evidence_id='private-evidence',bundle_hash='PRIVATE BUNDLE',
        artifact_digest='PRIVATE ARTIFACT' WHERE id='round-one'", &[]).await.unwrap();
    let result = store
        .query(TENANT, PROJECT, A, rounds_query())
        .await
        .unwrap();
    let data = &result["data"];
    assert_eq!(data["total"], 2);
    assert_eq!(data["items"].as_array().unwrap().len(), 2);
    let hidden = &data["items"][0];
    assert_eq!(hidden["round_id"], "round-one");
    assert_eq!(hidden["round_index"], 1);
    assert_eq!(hidden["readable"], false);
    assert_eq!(hidden["reason"], "not_in_review_work_scope");
    for field in [
        "state",
        "bundle_hash",
        "artifact_digest",
        "reviewer_role",
        "decision",
        "reason_first_line",
    ] {
        assert!(hidden[field].is_null());
        assert_eq!(hidden["missing"][field], hidden["reason"]);
    }
    assert_eq!(data["items"][1]["readable"], true);
    assert!(!result.to_string().contains("PRIVATE"));
}

#[tokio::test]
async fn review_round_paging_is_bounded_and_invalidates_on_current_visible_changes() {
    let (_guard, admin, _, store) = setup().await;
    for i in 1..=3 {
        round(&admin, &format!("round-{i}"), "a", i * 2, "open").await;
    }
    let mut q = rounds_query();
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["items"][0]["round_index"], 2);
    assert_eq!(first["data"]["truncated"], true);
    q.cursor = first["data"]["next_cursor"].as_str().map(Into::into);
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(second["data"]["items"][0]["round_index"], 4);
    assert_eq!(second["data"]["items"][0]["round_position"], 2);
    decision(&admin, "late-decision", "round-1", "unspecified").await;
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    q.cursor = None;
    q.max_context_bytes = Some(1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ResponseTooLarge)
    ));
    let mut q = rounds_query();
    q.work_id = Some("c".into());
    let empty = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(empty["data"]["total"], 0);
    assert_eq!(empty["data"]["items"], json!([]));
    for work in ["b-private", "unknown"] {
        let mut q = rounds_query();
        q.work_id = Some(work.into());
        assert!(store.query(TENANT, PROJECT, A, q).await.is_err());
    }
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET can_read=false WHERE client_id='cli-a'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, rounds_query()).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn review_round_latest_decision_uses_matching_work_bundle_and_deterministic_ties() {
    let (_guard, admin, _, store) = setup().await;
    round(&admin, "round-one", "a", 1, "approved").await;
    decision(&admin, "a-first", "round-one", "unspecified").await;
    decision(&admin, "b-last", "round-one", "human_author_self_review").await;
    decision(
        &admin,
        "z-wrong-work",
        "round-one",
        "human_independent_review",
    )
    .await;
    decision(
        &admin,
        "zz-wrong-bundle",
        "round-one",
        "human_independent_review",
    )
    .await;
    admin
        .execute(
            "UPDATE awr_team.review_decisions SET work_id='b-private' WHERE id='z-wrong-work'",
            &[],
        )
        .await
        .unwrap();
    admin.execute("UPDATE awr_team.review_decisions SET bundle_hash=repeat('d',64) WHERE id='zz-wrong-bundle'", &[]).await.unwrap();
    let result = store
        .query(TENANT, PROJECT, A, rounds_query())
        .await
        .unwrap();
    assert_eq!(
        result["data"]["items"][0]["reviewer_role"],
        "author_reviewer"
    );
    let mut q = query("review.inspect");
    q.work_id = Some("a".into());
    q.review_round_id = Some("round-one".into());
    let inspect = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(
        inspect["data"]["review"]["decisions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        inspect["data"]["review"]["decisions"][1]["decision_id"],
        "b-last"
    );
}
