#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{PgError, workstream_credential_hash};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

/// A well-formed bearer for a credential the fixture does not create by default.
fn token(id: &str, fill: char) -> String {
    format!("awr1.{id}.{}", fill.to_string().repeat(64))
}

async fn add_credential(admin: &Client, id: &str, client: &str, bearer: &str) {
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
             VALUES($1,$2,'agent',$3,$4)",
            &[
                &TENANT,
                &id,
                &client,
                &workstream_credential_hash(bearer).unwrap(),
            ],
        )
        .await
        .unwrap();
}

/// A recorded request made with one exact credential of the `agent` actor.
async fn seed_request(admin: &Client, id: &str, client: &str, credential: &str) {
    admin
        .execute(
            "INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,
             credential_id,action,result)
             VALUES($1,$2,$3,'agent',$4,$5,'work.prepare','succeeded')",
            &[&TENANT, &PROJECT, &id, &client, &credential],
        )
        .await
        .unwrap();
}

/// A committed event at its own revision, attributed to `client` through the operation
/// that committed at that revision, or left unattributed like legacy events.
async fn seed_development(admin: &Client, id: &str, revision: i64, client: Option<&str>) {
    if let Some(client) = client {
        admin
            .execute(
                "INSERT INTO awr_team.operations(tenant_id,project_id,id,actor_id,client_id,
                 request_id,op,request_hash,state,committed_project_revision,result_json)
                 VALUES($1,$2,$3,'agent',$4,$3,'session.checkpoint','hash','committed',$5,'{}')",
                &[&TENANT, &PROJECT, &format!("op-{id}"), &client, &revision],
            )
            .await
            .unwrap();
    }
    admin
        .execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,
             event_type,actor_id,work_id,payload_json)
             VALUES($1,$2,$3,$4,0,'session.start','agent',NULL,'{}')",
            &[&TENANT, &PROJECT, &id, &revision],
        )
        .await
        .unwrap();
}

fn named(name: &str) -> Value {
    json!({"display_name":name})
}

fn unnamed() -> Value {
    json!({"display_name":null})
}

fn null_actor() -> Value {
    json!({"person_id":null,"display_name":null,"kind":null})
}

fn activity_item<'a>(reply: &'a Value, id: &str) -> &'a Value {
    reply["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap_or_else(|| panic!("missing activity item {id}: {reply}"))
}

async fn stored_name(admin: &Client, id: &str) -> Option<String> {
    admin
        .query_one(
            "SELECT client_name FROM awr_team.credentials WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &id],
        )
        .await
        .unwrap()
        .get(0)
}

async fn full_directory_access(admin: &tokio_postgres::Client) {
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
             workstream_id,authority_version,can_read)
             SELECT tenant_id,project_id,actor_id,'cli-a',workstream_id,authority_version,can_read
             FROM awr_team.workstream_grants WHERE client_id='cli-b'",
            &[],
        )
        .await
        .unwrap();
}

// AWR-PPD-062 acceptance 1 and 2: the name an approved connection registered travels
// with its credential, and both audit reads disclose it to a reader who passes the
// directory-read gate. A credential without an approved connection stays unnamed.
#[tokio::test]
async fn both_audit_reads_name_the_credential_an_approved_connection_named() {
    let (_guard, admin, _, store) = setup().await;
    full_directory_access(&admin).await;
    let c = token("reader-c", 'd');
    add_credential(&admin, "reader-c", "cli-c", &c).await;
    store
        .connections()
        .record_client_name(TENANT, PROJECT, B, "Synthetic Named Client")
        .await
        .unwrap();
    store
        .connections()
        .record_client_name(TENANT, PROJECT, &c, "Synthetic Second Client")
        .await
        .unwrap();
    seed_request(&admin, "request-a", "cli-a", "reader-a").await;
    seed_request(&admin, "request-b", "cli-b", "reader-b").await;
    seed_request(&admin, "request-c", "cli-c", "reader-c").await;
    // A request made through a credential that is gone: nothing left to read.
    seed_request(&admin, "request-retired", "cli-b", "retired-credential").await;
    seed_development(&admin, "development-a", 80, Some("cli-a")).await;
    seed_development(&admin, "development-b", 81, Some("cli-b")).await;
    seed_development(&admin, "development-c", 82, Some("cli-c")).await;
    seed_development(&admin, "development-unattributed", 83, None).await;

    let requests = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&requests, "request-b")["client"],
        named("Synthetic Named Client")
    );
    assert_eq!(
        activity_item(&requests, "request-c")["client"],
        named("Synthetic Second Client")
    );
    for id in ["request-a", "request-retired"] {
        assert_eq!(activity_item(&requests, id)["client"], unnamed(), "{id}");
    }
    // The name is added beside the row's own attribution; it replaces nothing.
    let named_row = activity_item(&requests, "request-b");
    assert_eq!(named_row["client_id"], "cli-b");
    assert_eq!(named_row["credential_id"], "reader-b");
    assert_eq!(named_row["actor"]["kind"], "human");

    let development = store
        .query(TENANT, PROJECT, A, query("audit.development"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&development, "development-b")["client"],
        named("Synthetic Named Client")
    );
    assert_eq!(
        activity_item(&development, "development-c")["client"],
        named("Synthetic Second Client")
    );
    for id in ["development-a", "development-unattributed"] {
        assert_eq!(activity_item(&development, id)["client"], unnamed(), "{id}");
    }
    assert_eq!(
        activity_item(&development, "development-b")["client_id"],
        "cli-b"
    );
}

// Acceptance 2: the same directory-read gate governs client names. Without it the
// rows return as before and every identity field, the client name included, is null.
#[tokio::test]
async fn without_the_directory_gate_the_name_stays_withheld_and_the_audit_still_returns() {
    let (_guard, admin, _, store) = setup().await;
    store
        .connections()
        .record_client_name(TENANT, PROJECT, B, "Withheld Client")
        .await
        .unwrap();
    assert_eq!(
        stored_name(&admin, "reader-b").await.as_deref(),
        Some("Withheld Client")
    );
    seed_request(&admin, "request-b", "cli-b", "reader-b").await;
    seed_development(&admin, "development-b", 80, Some("cli-b")).await;
    assert!(matches!(
        store.directory().read(TENANT, PROJECT, A).await,
        Err(PgError::Forbidden)
    ));

    let requests = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(requests["data"]["scope"], "project");
    let withheld = activity_item(&requests, "request-b");
    assert_eq!(withheld["client"], unnamed());
    assert_eq!(withheld["actor"], null_actor());
    assert_eq!(withheld["client_id"], "cli-b");
    let development = store
        .query(TENANT, PROJECT, A, query("audit.development"))
        .await
        .unwrap();
    let withheld = activity_item(&development, "development-b");
    assert_eq!(withheld["client"], unnamed());
    assert_eq!(withheld["actor"], null_actor());

    full_directory_access(&admin).await;
    let requests = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&requests, "request-b")["client"],
        named("Withheld Client")
    );
    let development = store
        .query(TENANT, PROJECT, A, query("audit.development"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&development, "development-b")["client"],
        named("Withheld Client")
    );
}

// Acceptance 3: the name is untrusted text. Before it is stored, control characters
// are dropped and the length is cut to the registered bound on a character boundary.
// The database keeps the same bound, and the first approved connection keeps its name.
#[tokio::test]
async fn control_characters_and_excess_length_are_removed_before_a_name_is_stored() {
    let (_guard, admin, _, store) = setup().await;
    let names = store.connections();
    names
        .record_client_name(
            TENANT,
            PROJECT,
            B,
            "  Synthetic\u{7f}\nClient\t\u{85}\u{9f}  ",
        )
        .await
        .unwrap();
    assert_eq!(
        stored_name(&admin, "reader-b").await.as_deref(),
        Some("SyntheticClient")
    );

    names
        .record_client_name(TENANT, PROJECT, A, &"客".repeat(300))
        .await
        .unwrap();
    let cut = stored_name(&admin, "reader-a").await.unwrap();
    assert_eq!(cut.chars().count(), 120);
    assert!(cut.chars().all(|character| character == '客'));

    // Blank after reduction: no name to record, and no error.
    names
        .record_client_name(TENANT, PROJECT, NONE, " \t\n\u{0}\u{3000}")
        .await
        .unwrap();
    assert_eq!(stored_name(&admin, "no-grants").await, None);

    // A later connection through the same credential keeps the recorded name.
    names
        .record_client_name(TENANT, PROJECT, B, "Later Client")
        .await
        .unwrap();
    assert_eq!(
        stored_name(&admin, "reader-b").await.as_deref(),
        Some("SyntheticClient")
    );

    // The column itself refuses what the reduction removes.
    for value in ["E'bad\\nname'", "repeat('x',121)", "''"] {
        let statement = format!(
            "UPDATE awr_team.credentials SET client_name={value}
             WHERE tenant_id='reader-tenant' AND id='no-grants'"
        );
        assert!(
            admin.execute(&statement, &[]).await.is_err(),
            "database invariant accepted {statement}"
        );
    }
    assert_eq!(stored_name(&admin, "no-grants").await, None);
    admin
        .execute(
            "UPDATE awr_team.credentials SET client_name=repeat('客',120)
             WHERE tenant_id='reader-tenant' AND id='no-grants'",
            &[],
        )
        .await
        .unwrap();

    // Only the credential's own secret names it: an unknown, forged, revoked or
    // malformed bearer records nothing.
    let forged = token("reader-b", 'f');
    names
        .record_client_name(TENANT, PROJECT, &forged, "Forged Client")
        .await
        .unwrap();
    assert_eq!(
        stored_name(&admin, "reader-b").await.as_deref(),
        Some("SyntheticClient")
    );
    let c = token("reader-c", 'd');
    add_credential(&admin, "reader-c", "cli-c", &c).await;
    admin
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp()
             WHERE tenant_id='reader-tenant' AND id='reader-c'",
            &[],
        )
        .await
        .unwrap();
    names
        .record_client_name(TENANT, PROJECT, &c, "Revoked Client")
        .await
        .unwrap();
    assert_eq!(stored_name(&admin, "reader-c").await, None);
    assert!(matches!(
        names
            .record_client_name(TENANT, PROJECT, "not-a-credential", "Nowhere")
            .await,
        Err(PgError::Forbidden)
    ));
}

// Acceptance 3: two clients that report the same name stay two credentials with
// their own ids; each audit row keeps its own attribution.
#[tokio::test]
async fn two_clients_reporting_the_same_name_stay_separate_credentials() {
    let (_guard, admin, _, store) = setup().await;
    full_directory_access(&admin).await;
    let c = token("reader-c", 'd');
    add_credential(&admin, "reader-c", "cli-c", &c).await;
    for bearer in [B, c.as_str()] {
        store
            .connections()
            .record_client_name(TENANT, PROJECT, bearer, "Synthetic Twin Client")
            .await
            .unwrap();
    }
    seed_request(&admin, "request-a", "cli-a", "reader-a").await;
    seed_request(&admin, "request-b", "cli-b", "reader-b").await;
    seed_request(&admin, "request-c", "cli-c", "reader-c").await;
    seed_development(&admin, "development-a", 80, Some("cli-a")).await;
    seed_development(&admin, "development-b", 81, Some("cli-b")).await;
    seed_development(&admin, "development-c", 82, Some("cli-c")).await;

    let credentials = admin
        .query(
            "SELECT id,client_id FROM awr_team.credentials
             WHERE tenant_id=$1 AND client_name='Synthetic Twin Client' ORDER BY id",
            &[&TENANT],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
        .collect::<Vec<_>>();
    assert_eq!(
        credentials,
        vec![
            ("reader-b".to_owned(), "cli-b".to_owned()),
            ("reader-c".to_owned(), "cli-c".to_owned())
        ]
    );

    let requests = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    for (id, credential, client) in [
        ("request-b", "reader-b", "cli-b"),
        ("request-c", "reader-c", "cli-c"),
    ] {
        let item = activity_item(&requests, id);
        assert_eq!(item["client"], named("Synthetic Twin Client"), "{id}");
        assert_eq!(item["credential_id"], credential, "{id}");
        assert_eq!(item["client_id"], client, "{id}");
    }
    assert_eq!(activity_item(&requests, "request-a")["client"], unnamed());
    let development = store
        .query(TENANT, PROJECT, A, query("audit.development"))
        .await
        .unwrap();
    for id in ["development-b", "development-c"] {
        assert_eq!(
            activity_item(&development, id)["client"],
            named("Synthetic Twin Client"),
            "{id}"
        );
    }
    assert_eq!(
        activity_item(&development, "development-a")["client"],
        unnamed()
    );
}

// A request row names its exact credential. A development row only knows an actor and
// a client id: it is named when those credentials agree, and stays unnamed when two of
// them recorded different names.
#[tokio::test]
async fn a_development_row_is_named_only_when_the_credentials_behind_it_agree() {
    let (_guard, admin, _, store) = setup().await;
    full_directory_access(&admin).await;
    let b2 = token("reader-b2", 'e');
    let c = token("reader-c", 'd');
    let c2 = token("reader-c2", 'f');
    add_credential(&admin, "reader-b2", "cli-b", &b2).await;
    add_credential(&admin, "reader-c", "cli-c", &c).await;
    add_credential(&admin, "reader-c2", "cli-c", &c2).await;
    for (bearer, name) in [
        (B, "First Name"),
        (b2.as_str(), "Second Name"),
        (c.as_str(), "Agreed Name"),
        (c2.as_str(), "Agreed Name"),
    ] {
        store
            .connections()
            .record_client_name(TENANT, PROJECT, bearer, name)
            .await
            .unwrap();
    }
    seed_request(&admin, "request-b", "cli-b", "reader-b").await;
    seed_request(&admin, "request-b2", "cli-b", "reader-b2").await;
    seed_request(&admin, "request-c", "cli-c", "reader-c").await;
    seed_development(&admin, "development-b", 80, Some("cli-b")).await;
    seed_development(&admin, "development-c", 81, Some("cli-c")).await;

    let requests = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&requests, "request-b")["client"],
        named("First Name")
    );
    assert_eq!(
        activity_item(&requests, "request-b2")["client"],
        named("Second Name")
    );
    assert_eq!(
        activity_item(&requests, "request-c")["client"],
        named("Agreed Name")
    );
    let development = store
        .query(TENANT, PROJECT, A, query("audit.development"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&development, "development-b")["client"],
        unnamed()
    );
    assert_eq!(
        activity_item(&development, "development-c")["client"],
        named("Agreed Name")
    );
}

// A request row must agree with its credential on actor and client id as well as on
// the credential id; a row that only borrows a named credential's id stays unnamed.
#[tokio::test]
async fn a_request_row_names_a_credential_only_when_the_whole_attribution_matches() {
    let (_guard, admin, _, store) = setup().await;
    full_directory_access(&admin).await;
    store
        .connections()
        .record_client_name(TENANT, PROJECT, B, "Exact Client")
        .await
        .unwrap();
    admin
        .batch_execute(
            "INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,
             credential_id,action,result) VALUES
             ('reader-tenant','reader-project','request-borrowed-id','agent','cli-a','reader-b','work.prepare','succeeded'),
             ('reader-tenant','reader-project','request-other-actor','reviewer','cli-b','reader-b','work.prepare','succeeded');
             INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,
             credential_id,action,result)
             VALUES('reader-tenant','reader-project','request-exact','agent','cli-b','reader-b','work.prepare','succeeded')",
        )
        .await
        .unwrap();
    let requests = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(
        activity_item(&requests, "request-exact")["client"],
        named("Exact Client")
    );
    for id in ["request-borrowed-id", "request-other-actor"] {
        assert_eq!(activity_item(&requests, id)["client"], unnamed(), "{id}");
    }
}

// Names resolve for a whole page in one statement: the page size does not multiply
// the reads.
#[tokio::test]
async fn a_full_page_resolves_client_names_without_per_row_reads() {
    let (_guard, admin, _, store) = setup().await;
    full_directory_access(&admin).await;
    store
        .connections()
        .record_client_name(TENANT, PROJECT, B, "Bulk Client")
        .await
        .unwrap();
    admin
        .batch_execute(
            "INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,
             credential_id,action,result)
             SELECT 'reader-tenant','reader-project','bulk-request-'||g,'agent','cli-b',
                    'reader-b','work.prepare','succeeded' FROM generate_series(1,60) g",
        )
        .await
        .unwrap();
    let mut q = query("audit.requests");
    q.limit = Some(60);
    let page = store.query(TENANT, PROJECT, A, q).await.unwrap();
    let items = page["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 60);
    assert!(
        items
            .iter()
            .all(|item| item["client"] == named("Bulk Client"))
    );
}
