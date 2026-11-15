use super::*;

async fn people(admin: &tokio_postgres::Client) {
    admin
        .execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
        ($1,$2,'author','Author','active'),($1,$2,'receiver','Receiver','active'),
        ($1,$2,'other-receiver','Other receiver','active')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
}

async fn handoff(
    admin: &tokio_postgres::Client,
    id: &str,
    work: &str,
    kind: &str,
    state: &str,
    recipient: &str,
    expires: Option<i64>,
) {
    admin.execute("INSERT INTO awr_team.team_handoffs(tenant_id,project_id,id,work_id,kind,status,version,
        from_person_id,to_person_id,package_json,body_json,created_at_ms,updated_at_ms,expires_at_ms)
        VALUES($1,$2,$3,$4,$5,$6,1,'author',$7,'{\"checkpoint\":\"PRIVATE PACKAGE\"}',
          '{\"private\":\"PRIVATE BODY\"}',100,100,$8)",
        &[&TENANT,&PROJECT,&id,&work,&kind,&state,&recipient,&expires]).await.unwrap();
}

#[tokio::test]
async fn handoff_discovery_separates_pending_history_expiry_and_existing_kinds_without_writes() {
    let (_guard, admin, _, store) = setup().await;
    people(&admin).await;
    handoff(
        &admin,
        "a-pending",
        "a",
        "execution",
        "inspected",
        "receiver",
        None,
    )
    .await;
    handoff(
        &admin,
        "c-expired",
        "c",
        "responsibility",
        "proposed",
        "receiver",
        Some(1),
    )
    .await;
    for (state, expires) in [
        ("accepted", Some(1)),
        ("rejected", None),
        ("cancelled", None),
        ("timed_out", None),
    ] {
        handoff(
            &admin,
            &format!("b-{state}"),
            "a",
            "responsibility",
            state,
            "other-receiver",
            expires,
        )
        .await;
    }
    handoff(
        &admin,
        "PRIVATE HANDOFF",
        "b-private",
        "execution",
        "proposed",
        "receiver",
        None,
    )
    .await;
    let before: Value = admin
        .query_one(
            "SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM awr_team.team_handoffs h",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let mut q = query("handoff.list");
    let pending = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(pending["data"]["handoff_state"], "pending");
    assert_eq!(pending["data"]["total"], 1);
    assert_eq!(pending["data"]["items"][0]["handoff_id"], "a-pending");
    assert_eq!(pending["data"]["items"][0]["kind"], "execution");
    assert_eq!(pending["data"]["items"][0]["from_display_name"], "Author");
    assert_eq!(pending["data"]["count_limited"], true);
    assert!(pending["data"]["hidden_count"].is_null());
    assert_eq!(pending["data"]["execution_authorized"], false);
    q.handoff_state = Some("history".into());
    let history = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(history["data"]["total"], 5);
    let rows = history["data"]["items"].as_array().unwrap();
    assert!(
        rows.iter()
            .all(|r| r["pending"] == false && r["group"] == "history")
    );
    let expired = rows
        .iter()
        .find(|r| r["handoff_id"] == "c-expired")
        .unwrap();
    assert_eq!(expired["status"], "proposed");
    assert_eq!(expired["effective_status"], "expired");
    assert_eq!(expired["expired"], true);
    let accepted = rows.iter().find(|r| r["status"] == "accepted").unwrap();
    assert_eq!(accepted["expired"], false);
    assert_eq!(accepted["effective_status"], "accepted");
    q.handoff_state = Some("all".into());
    q.recipient_person_id = Some("receiver".into());
    let recipient = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(recipient["data"]["total"], 2);
    assert!(
        recipient["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["to_person_id"] == "receiver")
    );
    for result in [&pending, &history, &recipient] {
        assert!(!result.to_string().contains("PRIVATE"));
        assert_eq!(
            result["data"]["as_of"]["source_snapshot_id"],
            result["source_snapshot_id"]
        );
        assert!(result["data"]["as_of"]["at_unix_ms"].is_i64());
    }
    q.work_id = Some("c".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["total"],
        1
    );
    q.recipient_person_id = Some("unknown-recipient".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["total"],
        0
    );
    let after: Value = admin
        .query_one(
            "SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM awr_team.team_handoffs h",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after);
}

#[tokio::test]
async fn handoff_pages_bind_the_recipient_visible_history_and_current_grant_versions() {
    let (_guard, admin, _, store) = setup().await;
    people(&admin).await;
    for index in 1..=3 {
        handoff(
            &admin,
            &format!("handoff-{index}"),
            "a",
            "execution",
            "accepted",
            "receiver",
            None,
        )
        .await;
    }
    let mut q = query("handoff.list");
    q.handoff_state = Some("all".into());
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 3);
    assert_eq!(first["data"]["items"][0]["handoff_position"], 1);
    q.cursor = first["data"]["next_cursor"].as_str().map(Into::into);
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(second["data"]["items"][0]["handoff_position"], 2);
    let mut changed = q.clone();
    changed.recipient_person_id = Some("receiver".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, changed).await,
        Err(PgError::CursorExpired)
    ));
    admin.execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'",&[]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    q.cursor = None;
    let fresh = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    q.cursor = fresh["data"]["next_cursor"].as_str().map(Into::into);
    admin.execute("UPDATE awr_team.team_handoffs SET status='cancelled',version=version+1 WHERE id='handoff-2'",&[]).await.unwrap();
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
}

#[tokio::test]
async fn handoff_expiry_observation_changes_membership_without_mutating_the_proposal() {
    let (_guard, admin, _, store) = setup().await;
    people(&admin).await;
    let future: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint+3000",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    handoff(
        &admin,
        "a-first",
        "a",
        "execution",
        "proposed",
        "receiver",
        Some(future),
    )
    .await;
    handoff(
        &admin,
        "c-second",
        "c",
        "execution",
        "proposed",
        "receiver",
        None,
    )
    .await;
    let mut q = query("handoff.list");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    q.cursor = first["data"]["next_cursor"].as_str().map(Into::into);
    let remaining = future - first["data"]["as_of"]["at_unix_ms"].as_i64().unwrap() + 50;
    tokio::time::sleep(std::time::Duration::from_millis(remaining.max(0) as u64)).await;
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    q.cursor = None;
    let pending = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(pending["data"]["total"], 1);
    assert_eq!(pending["data"]["items"][0]["handoff_id"], "c-second");
    q.handoff_state = Some("history".into());
    let history = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(history["data"]["items"][0]["effective_status"], "expired");
    assert_eq!(
        admin
            .query_one(
                "SELECT status FROM awr_team.team_handoffs WHERE id='a-first'",
                &[]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "proposed"
    );
}
