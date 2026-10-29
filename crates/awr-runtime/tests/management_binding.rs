use awr_core::*;
use awr_runtime::{AssessManagementRequest, ManageWorkRequest, assess_management, manage_work};
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
    session: Id,
}

impl Fixture {
    fn new(ledger: &Value) -> Self {
        Self::with_adapter(ledger, "yaml-ledger-v1")
    }

    fn with_adapter(ledger: &Value, adapter: &str) -> Self {
        Self::build(ledger, adapter, None)
    }

    /// The primary ledger plus a supporting Markdown goal source.
    fn with_supporting_goals(ledger: &Value, goals: &str) -> Self {
        Self::build(ledger, "yaml-ledger-v1", Some(goals))
    }

    fn build(ledger: &Value, adapter: &str, supporting_goals: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!("awr-management-binding-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut manifest = format!(
            "[project]\nname='Management binding fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.json'\nadapter='{adapter}'\n"
        );
        if let Some(goals) = supporting_goals {
            manifest.push_str(
                "[[sources]]\ndomain='goal'\nrole='supporting'\npath='GOALS.md'\nadapter='markdown-heading-v1'\n",
            );
            fs::write(root.join("GOALS.md"), goals).unwrap();
        }
        fs::write(root.join(".awr/project.toml"), manifest).unwrap();
        fs::write(
            root.join("work.json"),
            serde_json::to_vec_pretty(ledger).unwrap(),
        )
        .unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let indexed =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(indexed.ok, "{indexed:?}");
        let session = store
            .start_session(
                indexed.project_id,
                indexed.project_revision,
                SessionDraft {
                    work_item_key: Some("W".into()),
                    agent_id: "fixture-agent".into(),
                    provider: "fixture".into(),
                    model: "fixture".into(),
                    branch_id: None,
                    claim: false,
                    claim_ttl_ms: None,
                },
            )
            .unwrap()
            .0
            .session
            .id;
        Self {
            root,
            store,
            project: indexed.project_id,
            session,
        }
    }

    fn revision(&self) -> Revision {
        self.store.project(self.project).unwrap().project_revision
    }

    fn replace(&mut self, ledger: &Value) {
        fs::write(
            self.root.join("work.json"),
            serde_json::to_vec_pretty(ledger).unwrap(),
        )
        .unwrap();
        self.reindex();
    }

    fn replace_supporting_goals(&mut self, goals: &str) {
        fs::write(self.root.join("GOALS.md"), goals).unwrap();
        self.reindex();
    }

    fn reindex(&mut self) {
        let indexed = index_project(
            &mut self.store,
            &self.root,
            &Manifest::load(&self.root).unwrap(),
            false,
        )
        .unwrap();
        assert!(indexed.ok, "{indexed:?}");
    }

    fn assess(&self) -> Value {
        assess_management(
            &self.store,
            &self.root,
            &AssessManagementRequest {
                work: "W".into(),
                branch: None,
            },
        )
        .unwrap()
    }

    fn record(&mut self, request_key: &str, fingerprint: &str) -> Value {
        let expected_revision = self.revision();
        manage_work(
            &mut self.store,
            &self.root,
            &ManageWorkRequest {
                work: "W".into(),
                session: self.session,
                expected_revision,
                request_key: request_key.into(),
                contract_fingerprint: fingerprint.into(),
                observation: observation(),
            },
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn observation() -> ManagementObservation {
    ManagementObservation {
        observed_at: now_millis().unwrap(),
        note: "One bounded outcome with a current plan".into(),
        single_outcome: Some(true),
        bounded_scope: Some(true),
        single_executor: Some(true),
        no_deferred_wait: Some(true),
        independently_schedulable_units: Some(1),
        plan_valid: Some(true),
        outcome_known: Some(true),
        ..Default::default()
    }
}

fn goal(id: &str, title: &str) -> Value {
    json!({
        "id": id,
        "title": title,
        "status": "active",
        "summary": format!("Deliver {title}"),
        "success_criteria": [format!("{title} is accepted")]
    })
}

fn work(goal_keys: Option<&[&str]>) -> Value {
    let mut work = json!({
        "id": "W",
        "title": "Deliver the result",
        "status": "ready",
        "summary": "Produce the requested result",
        "acceptance": ["The result is reviewed"],
        "next_action": "Produce the result"
    });
    if let Some(keys) = goal_keys {
        work["goals"] = json!(keys);
    }
    work
}

fn linked_ledger() -> Value {
    json!({
        "goals": [goal("G", "Primary outcome"), goal("OTHER", "Unrelated outcome")],
        "work_items": [work(Some(&["G"]))]
    })
}

fn goal_mut<'a>(ledger: &'a mut Value, id: &str) -> &'a mut Value {
    ledger["goals"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|goal| goal["id"] == id)
        .unwrap()
}

fn work_mut<'a>(ledger: &'a mut Value, id: &str) -> &'a mut Value {
    ledger["work_items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|work| work["id"] == id)
        .unwrap()
}

fn fingerprint(assessment: &Value) -> &str {
    assessment["contract_fingerprint"].as_str().unwrap()
}

fn legacy_fingerprint(fixture: &Fixture) -> String {
    let work = fixture.store.work_item(fixture.project, "W").unwrap();
    let mut dependencies = fixture
        .store
        .work_dependency_links(fixture.project)
        .unwrap()
        .into_iter()
        .filter(|edge| edge.from_key == "W")
        .collect::<Vec<_>>();
    dependencies.sort_by(|a, b| (&a.to_key, a.required, a.id).cmp(&(&b.to_key, b.required, b.id)));
    let dependencies = dependencies
        .iter()
        .map(|edge| json!({"key":edge.to_key,"required":edge.required}))
        .collect::<Vec<_>>();
    let mut goals = fixture
        .store
        .work_goal_links(fixture.project)
        .unwrap()
        .into_iter()
        .filter(|edge| edge.from_key == "W")
        .collect::<Vec<_>>();
    goals.sort_by(|a, b| (&a.to_key, a.required, a.id).cmp(&(&b.to_key, b.required, b.id)));
    let goals = goals
        .iter()
        .map(|edge| json!({"key":edge.to_key,"required":edge.required}))
        .collect::<Vec<_>>();
    let item = &work.item;
    let mut contract = json!({"id":item.meta.id,
        "title":item.title,"summary":item.summary,"kind":item.kind,"acceptance":item.acceptance,
        "paths":item.paths,"tags":item.tags,"goals":goals,"dependencies":dependencies,
        "ordinary_work_policy":work.source.config["adapter_options"]["ordinary_work_policy"]});
    if !item.spec_refs.is_empty() {
        contract["spec_refs"] = json!(item.spec_refs);
    }
    awr_source::fingerprint(&serde_json::to_vec(&contract).unwrap())
}

fn gaps(assessment: &Value) -> Vec<&str> {
    assessment["admission_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect()
}

#[test]
fn linked_goal_identity_and_semantics_bind_observations_and_stale_writes_are_rejected() {
    for field in ["key", "title", "status", "summary", "success_criteria"] {
        let base = linked_ledger();
        let mut fixture = Fixture::new(&base);
        let original = fixture.assess();
        let mut changed = base.clone();
        match field {
            "key" => {
                goal_mut(&mut changed, "G")["id"] = json!("G2");
                work_mut(&mut changed, "W")["goals"] = json!(["G2"]);
            }
            "title" => goal_mut(&mut changed, "G")["title"] = json!("Changed outcome"),
            "status" => goal_mut(&mut changed, "G")["status"] = json!("confirmed"),
            "summary" => goal_mut(&mut changed, "G")["summary"] = json!("Changed intent"),
            "success_criteria" => {
                goal_mut(&mut changed, "G")["success_criteria"] = json!(["Changed success"])
            }
            _ => unreachable!(),
        }
        fixture.replace(&changed);
        assert_ne!(
            fingerprint(&fixture.assess()),
            fingerprint(&original),
            "linked goal {field} must bind the management contract"
        );
    }

    let mut fixture = Fixture::new(&linked_ledger());
    let initial = fixture.assess();
    fixture.record("initial-observation", fingerprint(&initial));
    let mut changed = linked_ledger();
    goal_mut(&mut changed, "G")["summary"] = json!("A materially changed intended outcome");
    fixture.replace(&changed);
    let current = fixture.assess();
    assert_ne!(fingerprint(&current), fingerprint(&initial));
    assert!(current["observation"].is_null());
    assert_eq!(current["record_required"], true);
    assert!(
        current["decision"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason["code"] == "work_contract_changed")
    );

    let expected_revision = fixture.revision();
    let error = manage_work(
        &mut fixture.store,
        &fixture.root,
        &ManageWorkRequest {
            work: "W".into(),
            session: fixture.session,
            expected_revision,
            request_key: "stale-observation".into(),
            contract_fingerprint: fingerprint(&initial).into(),
            observation: observation(),
        },
    )
    .unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
}

#[test]
fn unrelated_edits_reordering_and_progress_do_not_invalidate_linked_goal_observations() {
    let base = json!({
        "goals": [goal("G", "Primary outcome"), goal("H", "Second outcome"), goal("OTHER", "Unrelated outcome")],
        "work_items": [
            {"id":"A","title":"A","status":"completed","acceptance":["A done"]},
            {"id":"B","title":"B","status":"completed","acceptance":["B done"]},
            {
                "id":"W","title":"Deliver the result","status":"ready",
                "summary":"Produce the requested result","acceptance":["The result is reviewed"],
                "next_action":"Produce the result","goals":["G","H"],"depends_on":["A","B"]
            }
        ]
    });
    let mut fixture = Fixture::new(&base);
    let initial = fixture.assess();
    fixture.record("stable-observation", fingerprint(&initial));

    let mut changed = base.clone();
    changed["goals"].as_array_mut().unwrap().reverse();
    work_mut(&mut changed, "W")["goals"] = json!(["H", "G"]);
    work_mut(&mut changed, "W")["depends_on"] = json!(["B", "A"]);
    work_mut(&mut changed, "W")["status"] = json!("in_progress");
    work_mut(&mut changed, "W")["next_action"] = json!("Continue after review");
    goal_mut(&mut changed, "OTHER")["summary"] = json!("Unrelated goal changed");
    fixture.replace(&changed);

    let current = fixture.assess();
    assert_eq!(fingerprint(&current), fingerprint(&initial));
    assert_eq!(current["observation"]["note"], observation().note);
    assert!(
        !current["decision"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason["code"] == "work_contract_changed")
    );

    let source = fixture
        .store
        .sources(fixture.project)
        .unwrap()
        .into_iter()
        .find(|source| source.domain == "ledger")
        .unwrap();
    fixture
        .store
        .mark_source_freshness(&source, Freshness::Stale)
        .unwrap();
    let stale = fixture.assess();
    assert_eq!(fingerprint(&stale), fingerprint(&initial));
    assert!(gaps(&stale).contains(&"linked_goal_source_not_fresh"));
}

#[test]
fn missing_linked_goal_is_diagnosed_even_when_an_unrelated_goal_exists() {
    let ledger = json!({
        "goals": [goal("OTHER", "Unrelated outcome")],
        "work_items": [work(Some(&["MISSING"]))]
    });
    let fixture = Fixture::new(&ledger);
    let assessment = fixture.assess();
    assert!(gaps(&assessment).contains(&"linked_goal_not_found"));
    assert!(!gaps(&assessment).contains(&"goal_context_missing"));
}

#[test]
fn legacy_no_link_work_binds_inherited_primary_goals() {
    let base = json!({
        "goals": [goal("G", "Inherited outcome"), goal("DONE", "Historical outcome")],
        "work_items": [work(None)]
    });
    let mut historical = base.clone();
    goal_mut(&mut historical, "DONE")["status"] = json!("completed");
    let mut fixture = Fixture::new(&historical);
    let initial = fixture.assess();
    assert!(!gaps(&initial).contains(&"goal_context_missing"));
    assert!(!gaps(&initial).contains(&"linked_goal_not_found"));
    fixture.record("inherited-observation", fingerprint(&initial));

    let mut changed = historical.clone();
    goal_mut(&mut changed, "G")["summary"] = json!("Changed inherited intent");
    goal_mut(&mut changed, "DONE")["summary"] = json!("Changed historical detail");
    fixture.replace(&changed);
    let current = fixture.assess();
    assert_ne!(fingerprint(&current), fingerprint(&initial));
    assert!(current["observation"].is_null());
    assert!(
        current["decision"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason["code"] == "work_contract_changed")
    );
}

#[test]
fn cancelled_goals_never_join_the_inherited_goal_binding() {
    let mut base = json!({
        "goals": [goal("G", "Inherited outcome"), goal("DROPPED", "Abandoned outcome")],
        "work_items": [work(None)]
    });
    goal_mut(&mut base, "DROPPED")["status"] = json!("cancelled");
    let mut fixture = Fixture::new(&base);
    let initial = fixture.assess();
    fixture.record("inherited-observation", fingerprint(&initial));

    // Editing only the cancelled goal leaves the binding and the saved observation current.
    let mut changed = base.clone();
    goal_mut(&mut changed, "DROPPED")["summary"] = json!("Changed abandoned detail");
    fixture.replace(&changed);
    let current = fixture.assess();
    assert_eq!(fingerprint(&current), fingerprint(&initial));
    assert!(!current["observation"].is_null());

    // Cancelled goals alone provide no inherited goal context.
    let mut only_cancelled = json!({
        "goals": [goal("DROPPED", "Abandoned outcome")],
        "work_items": [work(None)]
    });
    goal_mut(&mut only_cancelled, "DROPPED")["status"] = json!("cancelled");
    assert!(gaps(&Fixture::new(&only_cancelled).assess()).contains(&"goal_context_missing"));
}

#[test]
fn inherited_goals_come_only_from_primary_goal_sources() {
    let ledger = json!({
        "goals": [goal("G", "Inherited outcome")],
        "work_items": [work(None)]
    });
    let supporting =
        |detail: &str| format!("# Reference outcome {{#reference status=active}}\n\n{detail}\n");
    let mut fixture = Fixture::with_supporting_goals(&ledger, &supporting("Background material"));
    let initial = fixture.assess();
    fixture.record("primary-goal-observation", fingerprint(&initial));

    // A goal from a supporting source changes without touching the inherited binding.
    fixture.replace_supporting_goals(&supporting("Changed background material"));
    let current = fixture.assess();
    assert_eq!(fingerprint(&current), fingerprint(&initial));
    assert!(!current["observation"].is_null());
}

#[test]
fn legacy_fingerprint_requires_reassessment_without_forcing_continuous_management() {
    let mut fixture = Fixture::new(&linked_ledger());
    let current = fixture.assess();
    let legacy = legacy_fingerprint(&fixture);
    assert_ne!(legacy, fingerprint(&current));
    let observation = observation();
    let mut legacy_assessment = current.clone();
    legacy_assessment["contract_fingerprint"] = json!(legacy);
    legacy_assessment["decision"] =
        serde_json::to_value(decide_management(Some(&observation), vec![], false)).unwrap();
    legacy_assessment["observation"] = json!(observation);
    legacy_assessment["observer"] = json!("fixture-agent");
    legacy_assessment["record_required"] = json!(false);
    legacy_assessment
        .as_object_mut()
        .unwrap()
        .remove("goal_binding_upgrade_required");
    let input = json!({"work":"W","session":fixture.session,
        "contract_fingerprint":legacy,"observation":observation});
    fixture
        .store
        .record_management(
            fixture.project,
            fixture.revision(),
            fixture.session,
            fixture.store.work_identity(fixture.project, "W").unwrap(),
            json!({"version":1,"request_key":"legacy-v1","input":input,
                "observer":"fixture-agent","assessment":legacy_assessment}),
        )
        .unwrap();

    let upgrade = fixture.assess();
    assert_eq!(upgrade["goal_binding_upgrade_required"], true);
    assert_eq!(upgrade["decision"]["mode"], "undetermined");
    assert!(upgrade["observation"].is_null());
    assert!(upgrade["observer"].is_null());
    assert_eq!(upgrade["record_required"], true);
    assert!(
        !upgrade["decision"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason["code"] == "work_contract_changed")
    );

    let saved = fixture.record("goal-binding-v2", fingerprint(&upgrade));
    assert_eq!(saved["assessment"]["decision"]["mode"], "lightweight");
    assert_eq!(saved["assessment"]["goal_binding_upgrade_required"], false);
    let settled = fixture.assess();
    assert_eq!(settled["goal_binding_upgrade_required"], false);
    assert_eq!(settled["decision"]["mode"], "lightweight");
    assert_eq!(settled["observation"]["plan_valid"], true);
}

#[test]
fn inherited_fallback_uses_the_owning_workstream_goal_scope() {
    let ledger = json!({
        "workstreams": {
            "version": 1,
            "definitions": [
                {"id":"01K00000000000000000000001","external_key":"api","title":"API",
                    "state":"active","authority_version":1,"goal_keys":["G"],"acceptance_contracts":[]},
                {"id":"01K00000000000000000000002","external_key":"client","title":"Client",
                    "state":"active","authority_version":1,"goal_keys":["OTHER"],"acceptance_contracts":[]}
            ]
        },
        "goals": [goal("G", "API outcome"), goal("OTHER", "Client outcome")],
        "work_items": [{
            "id":"W","title":"Deliver API","status":"ready","workstream":"api",
            "summary":"Produce the API","acceptance":["The API is reviewed"],
            "next_action":"Produce the API"
        }]
    });
    let mut fixture = Fixture::with_adapter(&ledger, "yaml-workstream-ledger-v1");
    let initial = fixture.assess();

    let mut unrelated = ledger.clone();
    goal_mut(&mut unrelated, "OTHER")["summary"] = json!("Changed private client intent");
    fixture.replace(&unrelated);
    assert_eq!(fingerprint(&fixture.assess()), fingerprint(&initial));

    let mut relevant = unrelated;
    goal_mut(&mut relevant, "G")["summary"] = json!("Changed API intent");
    fixture.replace(&relevant);
    assert_ne!(fingerprint(&fixture.assess()), fingerprint(&initial));
}

#[test]
fn explicit_goal_links_cannot_read_or_bind_another_workstream_goal() {
    let ledger = json!({
        "workstreams": {
            "version": 1,
            "definitions": [
                {"id":"01K00000000000000000000001","external_key":"api","title":"API",
                    "state":"active","authority_version":1,"goal_keys":["G"],"acceptance_contracts":[]},
                {"id":"01K00000000000000000000002","external_key":"client","title":"Client",
                    "state":"active","authority_version":1,"goal_keys":["OTHER"],"acceptance_contracts":[]}
            ]
        },
        "goals": [goal("G", "API outcome"), goal("OTHER", "Client outcome")],
        "work_items": [{
            "id":"W","title":"Deliver API","status":"ready","workstream":"api",
            "summary":"Produce the API","acceptance":["The API is reviewed"],
            "next_action":"Produce the API","goals":["OTHER"]
        }]
    });
    let mut fixture = Fixture::with_adapter(&ledger, "yaml-workstream-ledger-v1");
    let unavailable = fixture.assess();
    assert!(gaps(&unavailable).contains(&"linked_goal_unavailable"));
    assert!(!gaps(&unavailable).contains(&"linked_goal_not_found"));

    let mut off_scope_changed = ledger.clone();
    goal_mut(&mut off_scope_changed, "OTHER")["title"] = json!("Private client title");
    goal_mut(&mut off_scope_changed, "OTHER")["summary"] = json!("Private client intent");
    goal_mut(&mut off_scope_changed, "OTHER")["success_criteria"] =
        json!(["Private client acceptance"]);
    fixture.replace(&off_scope_changed);
    assert_eq!(fingerprint(&fixture.assess()), fingerprint(&unavailable));

    let mut allowed = off_scope_changed;
    work_mut(&mut allowed, "W")["goals"] = json!(["G"]);
    fixture.replace(&allowed);
    let allowed_assessment = fixture.assess();
    assert!(!gaps(&allowed_assessment).contains(&"linked_goal_unavailable"));
    let allowed_fingerprint = fingerprint(&allowed_assessment).to_owned();

    goal_mut(&mut allowed, "G")["summary"] = json!("Changed API intent");
    fixture.replace(&allowed);
    assert_ne!(fingerprint(&fixture.assess()), allowed_fingerprint);
}
