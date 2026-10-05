use awr_team::*;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> Value {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("tests/fixtures/team-mcp");
    path.push(name);
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("fixture json")
}

#[test]
fn role_action_matrix_matches_frozen_contract() {
    let doc = fixture("role_action_matrix_v2.json");
    assert_eq!(doc["policy_id"], PERMISSION_POLICY_ID);
    assert_eq!(doc["policy_version"], PERMISSION_POLICY_VERSION);
    let matrix = doc["action_role_matrix"].as_object().unwrap();
    for action in Action::all() {
        let roles = matrix
            .get(action.as_str())
            .unwrap_or_else(|| panic!("missing matrix row {}", action.as_str()))
            .as_array()
            .unwrap();
        let expected: BTreeSet<_> = roles
            .iter()
            .map(|v| RoleTemplate::parse(v.as_str().unwrap()).unwrap())
            .collect();
        for role in RoleTemplate::all() {
            assert_eq!(
                action_allowed_for_template(role, action),
                expected.contains(&role),
                "{} vs {}",
                role.as_str(),
                action.as_str()
            );
        }
    }
    assert!(!template_actions(RoleTemplate::ProjectAdmin).contains(&Action::ReviewDecide));
    for role in RoleTemplate::all() {
        for special in SpecialAuthority::all() {
            assert!(!template_grants_special(role, special));
        }
    }
}

#[test]
fn migration_preview_fixture_cases() {
    let doc = fixture("migration_preview.json");
    for case in doc["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let role = case
            .get("legacy_role")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "reader" => LegacyRole::Reader,
                "reviewer" => LegacyRole::Reviewer,
                "worker" => LegacyRole::Worker,
                "admin" => LegacyRole::Admin,
                other => panic!("{id}: bad role {other}"),
            });
        let grant = match case.get("legacy_grant") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(match s.as_str() {
                "read" => LegacyGrant::Read,
                "write" => LegacyGrant::Write,
                "manage" => LegacyGrant::Manage,
                other => panic!("{id}: bad grant {other}"),
            }),
            _ => panic!("{id}: bad grant"),
        };
        let link = match case["person_link"].as_str().unwrap() {
            "verified" => PersonLinkStatus::Verified,
            "unknown" => PersonLinkStatus::Unknown,
            other => panic!("{id}: bad link {other}"),
        };
        let preview = preview_legacy_migration(role, grant, link);
        assert!(!preview.independent_review_granted, "{id}");
        match case.get("expect_template") {
            Some(Value::Null) | None => assert!(preview.suggested_template.is_none(), "{id}"),
            Some(Value::String(s)) => {
                assert_eq!(preview.suggested_template.unwrap().as_str(), s, "{id}")
            }
            _ => panic!("{id}: bad expect_template"),
        }
        if let Some(arr) = case.get("expect_actions").and_then(|v| v.as_array()) {
            let got: BTreeSet<_> = preview
                .granted_actions
                .iter()
                .map(|a| a.as_str().to_string())
                .collect();
            let exp: BTreeSet<_> = arr
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            assert_eq!(got, exp, "{id}");
        }
        if let Some(arr) = case
            .get("expect_actions_contains")
            .and_then(|v| v.as_array())
        {
            for v in arr {
                let a = Action::parse(v.as_str().unwrap()).unwrap();
                assert!(preview.granted_actions.contains(&a), "{id} missing {v}");
            }
        }
        if let Some(arr) = case
            .get("expect_actions_excludes")
            .and_then(|v| v.as_array())
        {
            for v in arr {
                let a = Action::parse(v.as_str().unwrap()).unwrap();
                assert!(!preview.granted_actions.contains(&a), "{id} has {v}");
            }
        }
        if let Some(arr) = case
            .get("expect_withheld_contains")
            .and_then(|v| v.as_array())
        {
            for v in arr {
                let a = Action::parse(v.as_str().unwrap()).unwrap();
                assert!(
                    preview.withheld_new_actions.contains(&a),
                    "{id} withheld missing {v}"
                );
            }
        }
    }
}

#[test]
fn v1_matrix_remains_frozen_and_cannot_authorize_assignment() {
    let legacy = fixture("role_action_matrix.json");
    assert_eq!(legacy["policy_id"], "awr-team-mcp-permission-v1");
    assert_eq!(legacy["policy_version"], 1);
    let matrix = legacy["action_role_matrix"].as_object().unwrap();
    assert!(matrix.get("work.assign").is_none());
    for (name, roles) in matrix {
        let action = Action::parse(name).unwrap();
        for role in RoleTemplate::all() {
            assert_eq!(
                template_actions(role).contains(&action),
                roles.as_array().unwrap().iter().any(|v| v == role.as_str())
            );
        }
    }
    let mut scope =
        authority_from_template(RoleTemplate::ProjectAdmin, "t", "p", "member", "client");
    scope.policy_version = 1;
    let resource = ResourceRef {
        tenant_id: "t".into(),
        project_id: "p".into(),
        workstream_id: None,
        work_id: None,
    };
    assert!(authorize_action(&scope, Action::WorkRead, &resource, 0).is_ok());
    scope.allowed_actions.insert(Action::WorkAssign);
    assert!(authorize_action(&scope, Action::WorkAssign, &resource, 0).is_err());
    scope.policy_version = PERMISSION_POLICY_VERSION;
    assert!(authorize_action(&scope, Action::WorkAssign, &resource, 0).is_ok());
    scope.policy_version += 1;
    assert!(authorize_action(&scope, Action::WorkRead, &resource, 0).is_err());
}

#[test]
fn allow_deny_pairs_fixture() {
    let doc = fixture("allow_deny_pairs.json");
    let base = &doc["base_resource"];
    for pair in doc["pairs"].as_array().unwrap() {
        let id = pair["id"].as_str().unwrap();
        let expect_allow = pair["expect"].as_str().unwrap() == "allow";

        if let Some(kind) = pair.get("deny_kind").and_then(|v| v.as_str()) {
            let action = Action::parse(pair["action"].as_str().unwrap()).unwrap();
            let err = match kind {
                "role_name_only" => deny_role_name_only("project_admin", action),
                "tool_visibility_only" => deny_tool_visibility_only("planning.publish", action),
                "model_self_report_only" => deny_model_self_report_only("i am admin", action),
                other => panic!("{id}: bad deny_kind {other}"),
            }
            .unwrap_err();
            assert!(matches!(err, TeamError::PermissionDenied(_)), "{id}");
            assert!(!expect_allow, "{id}");
            continue;
        }

        if let Some(special) = pair.get("special_authority").and_then(|v| v.as_str()) {
            let role = RoleTemplate::parse(pair["template"].as_str().unwrap()).unwrap();
            let special = match special {
                "database_owner" => SpecialAuthority::DatabaseOwner,
                other => panic!("{id}: {other}"),
            };
            assert!(!template_grants_special(role, special), "{id}");
            assert!(!expect_allow, "{id}");
            continue;
        }

        let action_name = pair["action"].as_str().unwrap();
        let action = match Action::parse(action_name) {
            Ok(a) => a,
            Err(err) => {
                assert!(matches!(err, TeamError::PermissionDenied(_)), "{id}");
                assert!(!expect_allow, "{id}");
                continue;
            }
        };
        let role = RoleTemplate::parse(pair["template"].as_str().unwrap()).unwrap();
        let mut scope = authority_from_template(
            role,
            base["tenant_id"].as_str().unwrap(),
            base["project_id"].as_str().unwrap(),
            "person-a",
            "client-a",
        );
        scope.workstream_id = base
            .get("workstream_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(wid) = base.get("work_id").and_then(|v| v.as_str()) {
            scope.work_ids.insert(wid.into());
        }
        if pair
            .get("independent_review_grant")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            scope = with_independent_review(scope, role).unwrap();
        }
        if let Some(exp) = pair.get("not_after_unix_ms").and_then(|v| v.as_u64()) {
            scope.not_after_unix_ms = Some(exp);
        }
        let mut resource = ResourceRef {
            tenant_id: base["tenant_id"].as_str().unwrap().into(),
            project_id: base["project_id"].as_str().unwrap().into(),
            workstream_id: base
                .get("workstream_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            work_id: base
                .get("work_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        };
        if let Some(over) = pair.get("resource_override") {
            if let Some(p) = over.get("project_id").and_then(|v| v.as_str()) {
                resource.project_id = p.into();
            }
        }
        let now = pair
            .get("now_unix_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(1_000_000);
        let result = authorize_action(&scope, action, &resource, now);
        if expect_allow {
            result.unwrap_or_else(|e| panic!("{id} expected allow, got {e}"));
        } else {
            let err = result.unwrap_err();
            assert!(
                matches!(err, TeamError::PermissionDenied(_)),
                "{id} got {err:?}"
            );
        }
    }
}

#[test]
fn project_admin_does_not_inherit_special_or_cross_project() {
    let scope = authority_from_template(
        RoleTemplate::ProjectAdmin,
        "tenant-a",
        "project-a",
        "person-a",
        "client-a",
    );
    let other = ResourceRef {
        tenant_id: "tenant-a".into(),
        project_id: "project-b".into(),
        workstream_id: None,
        work_id: None,
    };
    let err = authorize_action(&scope, Action::WorkRead, &other, 10).unwrap_err();
    assert!(matches!(err, TeamError::PermissionDenied(_)));
    for special in SpecialAuthority::all() {
        assert!(!template_grants_special(
            RoleTemplate::ProjectAdmin,
            special
        ));
    }
}

#[test]
fn exhaustive_role_action_cells_present() {
    let doc = fixture("allow_deny_pairs.json");
    assert_eq!(doc["exhaustive"], true);
    assert_eq!(doc["cell_count"], 52);
    let pairs = doc["pairs"].as_array().unwrap();
    let mut cells = std::collections::BTreeSet::new();
    for pair in pairs {
        if pair.get("deny_kind").is_some()
            || pair.get("special_authority").is_some()
            || pair.get("resource_override").is_some()
            || pair.get("not_after_unix_ms").is_some()
        {
            continue;
        }
        let action = pair["action"].as_str().unwrap();
        if Action::parse(action).is_err() {
            continue;
        }
        let role = pair["template"].as_str().unwrap();
        let grant = pair
            .get("independent_review_grant")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if grant {
            continue; // extras beyond base cells
        }
        cells.insert((role.to_string(), action.to_string()));
    }
    assert_eq!(cells.len(), 52, "expected 4 roles x 13 actions");
    let legacy = fixture("role_action_matrix.json");
    for role in RoleTemplate::all() {
        for name in legacy["actions"].as_array().unwrap() {
            let action = Action::parse(name.as_str().unwrap()).unwrap();
            assert!(
                cells.contains(&(role.as_str().to_string(), action.as_str().to_string())),
                "missing cell {} x {}",
                role.as_str(),
                action.as_str()
            );
        }
        assert!(!template_actions(role).contains(&Action::WorkAssign));
    }
}

#[test]
fn protocol_counterexamples_catalog_complete() {
    let doc = fixture("protocol_counterexamples.json");
    assert_eq!(doc["schema"], "awr-tmcp-050-protocol-counterexamples-v1");
    assert_eq!(doc["work"], "AWR-TMCP-050");
    assert_eq!(doc["required_count"], 18);
    assert_eq!(doc["matrix_exhaustive"], true);
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 18);
    let expected = [
        "role_action_matrix",
        "execution_not_planning",
        "cross_scope_identity",
        "read_surface_isolation",
        "revocation_race",
        "least_privilege_delegation",
        "scoped_admin_and_last_admin",
        "secret_delivery_boundary",
        "proposal_approval_binding",
        "source_cas_and_crash",
        "live_source_publication",
        "graph_integrity",
        "idempotent_outcome",
        "runtime_state_authority",
        "review_person_and_version",
        "audit_atomicity",
        "legacy_and_operator_separation",
        "discovery_is_not_authority",
    ];
    for (i, id) in expected.iter().enumerate() {
        assert_eq!(cases[i]["id"], *id);
        assert_eq!(cases[i]["positive_control_required"], true);
        assert!(cases[i]["entry_point"].as_str().unwrap().len() > 10);
        assert!(cases[i]["assertions"].as_array().unwrap().len() >= 2);
    }
    let iso = &doc["pg_isolation"];
    assert_eq!(iso["test_env_var"], "AWR_TEAM_TEST_DATABASE_URL");
    assert_eq!(iso["runtime_env_var"], "AWR_TEAM_DATABASE_URL");
    assert_eq!(iso["process_exclusive_db"], true);
    assert_eq!(iso["runtime_url_must_not_be_cleaned"], true);
    assert_eq!(iso["mocked_predicates_not_service_pass"], true);
}

#[test]
fn migration_intersects_mixed_role_and_grant() {
    let reader_write = preview_legacy_migration(
        Some(LegacyRole::Reader),
        Some(LegacyGrant::Write),
        PersonLinkStatus::Verified,
    );
    assert_eq!(
        reader_write.granted_actions,
        BTreeSet::from([Action::WorkRead])
    );
    assert!(
        !reader_write
            .granted_actions
            .contains(&Action::ClaimManageOwn)
    );

    let admin_read = preview_legacy_migration(
        Some(LegacyRole::Admin),
        Some(LegacyGrant::Read),
        PersonLinkStatus::Verified,
    );
    assert_eq!(
        admin_read.granted_actions,
        BTreeSet::from([Action::WorkRead])
    );
    assert!(
        !admin_read
            .granted_actions
            .contains(&Action::PlanningPublish)
    );
    assert!(
        !admin_read
            .granted_actions
            .contains(&Action::AccessManageProject)
    );

    let worker_write = preview_legacy_migration(
        Some(LegacyRole::Worker),
        Some(LegacyGrant::Write),
        PersonLinkStatus::Verified,
    );
    assert!(
        worker_write
            .granted_actions
            .contains(&Action::ClaimManageOwn)
    );
    assert!(
        !worker_write
            .granted_actions
            .contains(&Action::PlanningPropose)
    );
}

#[test]
fn migration_handles_missing_role_or_grant_and_always_withholds_new() {
    let role_only =
        preview_legacy_migration(Some(LegacyRole::Worker), None, PersonLinkStatus::Verified);
    assert!(role_only.granted_actions.contains(&Action::ClaimManageOwn));
    assert!(!role_only.granted_actions.contains(&Action::PlanningPropose));
    assert!(
        role_only
            .withheld_new_actions
            .contains(&Action::PlanningPropose)
    );

    let grant_only =
        preview_legacy_migration(None, Some(LegacyGrant::Write), PersonLinkStatus::Verified);
    assert!(grant_only.granted_actions.contains(&Action::ClaimManageOwn));
    assert!(
        !grant_only
            .granted_actions
            .contains(&Action::AccessManageProject)
    );

    let neither = preview_legacy_migration(None, None, PersonLinkStatus::Verified);
    assert!(neither.granted_actions.is_empty());
    assert!(neither.suggested_template.is_none());
    for action in Action::new_privileged_actions() {
        assert!(neither.withheld_new_actions.contains(&action));
    }
}
