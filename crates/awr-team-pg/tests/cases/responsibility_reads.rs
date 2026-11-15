use super::*;

#[tokio::test]
async fn responsibility_names_follow_formal_roles_and_pending_assignments_without_changing_context()
{
    let (_guard, admin, _, store) = setup_with_assigned_tasks().await;
    let mut snapshot = query("work.snapshot");
    snapshot.work_id = Some("a".into());
    let empty = store
        .query(TENANT, PROJECT, A, snapshot.clone())
        .await
        .unwrap();
    for field in [
        "owner_name",
        "pending_name",
        "executor_person_name",
        "executor_agent_name",
    ] {
        assert!(empty["data"]["observation"]["responsibility"][field].is_null());
    }
    enable_writes(&admin).await;
    admin.batch_execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
        ('reader-tenant','reader-project','person-lin','Lin Chen','active'),
        ('reader-tenant','reader-project','person-shen','Shen Lan','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
        VALUES('reader-tenant','reader-project','person-lin-agent','person-lin','agent','active');
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
        VALUES('reader-tenant','person-shen','human','Shen Lan','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','reader-project','person-shen','developer');
        UPDATE awr_team.actors SET display_name='Delivery assistant' WHERE id='agent';
        UPDATE awr_team.project_memberships SET assignment_grant=true WHERE actor_id='agent';").await.unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'person-shen','shen-client',$3,1,true,true)", &[&TENANT,&PROJECT,&Id::from(1).to_string()]).await.unwrap();
    let p = prepare(&store, A, "a").await;
    Box::pin(store.commands().execute(
        TENANT,
        PROJECT,
        A,
        command(
            &p,
            "observe-responsibility-claim",
            "task.claim_available",
            json!({
        "session_id":"session-a","expected_session_version":"1","ttl_seconds":600,
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
        "expected_responsibility_version":p["data"]["responsibility"]["version"]}),
        ),
    ))
    .await
    .unwrap();
    let p = prepare(&store, A, "aa-assigned").await;
    Box::pin(store.commands().execute(TENANT,PROJECT,A,command(&p,"observe-pending-assignment","task.assign",json!({
        "assignee_person_id":"person-shen","expected_responsibility_version":p["data"]["responsibility"]["version"]})))).await.unwrap();
    let current = store
        .query(TENANT, PROJECT, A, snapshot.clone())
        .await
        .unwrap();
    let responsibility = &current["data"]["observation"]["responsibility"];
    assert_eq!(responsibility["owner_name"], "Lin Chen");
    assert_eq!(responsibility["executor_person_name"], "Lin Chen");
    assert_eq!(responsibility["executor_agent_name"], "Delivery assistant");
    assert!(responsibility["pending_name"].is_null());
    let mut assigned = snapshot.clone();
    assigned.work_id = Some("aa-assigned".into());
    let pending = store.query(TENANT, PROJECT, A, assigned).await.unwrap();
    assert_eq!(
        pending["data"]["observation"]["responsibility"]["pending_name"],
        "Shen Lan"
    );
    let context = prepare(&store, A, "a").await["data"]["context_hash"].clone();
    admin
        .batch_execute(
            "UPDATE awr_team.persons SET display_name='Lin Chen updated' WHERE id='person-lin';
        UPDATE awr_team.actors SET display_name='Delivery assistant updated' WHERE id='agent';",
        )
        .await
        .unwrap();
    let renamed = store
        .query(TENANT, PROJECT, A, snapshot.clone())
        .await
        .unwrap();
    assert_eq!(
        renamed["data"]["observation"]["responsibility"]["owner_name"],
        "Lin Chen updated"
    );
    assert_eq!(
        renamed["data"]["observation"]["responsibility"]["executor_agent_name"],
        "Delivery assistant updated"
    );
    assert_eq!(
        prepare(&store, A, "a").await["data"]["context_hash"],
        context
    );
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_read=false,can_write=false WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, snapshot).await,
        Err(PgError::Forbidden)
    ));
}
