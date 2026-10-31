//! DEC-040: the bounded local policy. A document selects among rules the program already has; it can narrow, never
//! remove a hard protection, never exceed the host's ceiling, and is chosen by `as_of`, never by a clock.
use awr_core::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn registry() -> Registry {
    Registry::builtin()
}

fn ceiling() -> PolicyCeiling {
    PolicyCeiling::builtin()
}

fn doc(versions: Value) -> PolicyDocument {
    serde_json::from_value(json!({"format": 1, "policy_id": "local-advice", "version": versions}))
        .unwrap()
}

fn codes(document: &PolicyDocument) -> Vec<(&'static str, Option<u32>)> {
    validate_document(document, &registry(), &ceiling())
        .into_iter()
        .map(|issue| (issue.code.as_str(), issue.version))
        .collect()
}

fn resolve(document: &PolicyDocument, as_of: i64) -> ResolvedPolicy {
    resolve_policy(Some(document), &registry(), &ceiling(), as_of).unwrap()
}

fn soft_codes() -> Vec<String> {
    registry()
        .soft_reason_codes()
        .into_iter()
        .map(String::from)
        .collect()
}

#[test]
fn without_a_document_the_builtin_policy_applies_unchanged() {
    let resolved = resolve_policy(None, &registry(), &ceiling(), 10).unwrap();
    assert_eq!(resolved.source, PolicySource::Builtin);
    assert_eq!(resolved.assessment, AssessmentPolicy::default());
    assert_eq!(resolved.assessment.policy_hash, None);
    assert_eq!(resolved.version, None);
    // The serialized default is what it was before the policy existed: no new key, so no old snapshot changes its hash.
    let value = serde_json::to_value(AssessmentPolicy::default()).unwrap();
    assert!(value.get("disabled_advice").is_none(), "{value}");
    assert_eq!(value["policy_id"], ASSESSMENT_POLICY_ID);
}

#[test]
fn a_version_switches_advice_off_lowers_limits_and_hashes_the_policy_it_makes() {
    let document = doc(json!([{
        "version": 1,
        "advice": {"continue_prepare_current_work": false, "record_management_observations": false},
        "limits": {"max_advisory": 8, "max_return_bytes": 65536}
    }]));
    assert_eq!(codes(&document), vec![]);
    let resolved = resolve(&document, 5);
    assert_eq!(resolved.source, PolicySource::Project);
    assert_eq!(resolved.policy_id, "local-advice");
    assert_eq!(resolved.version, Some(1));
    assert_eq!(
        resolved.disabled_advice,
        vec![
            "continue_prepare_current_work",
            "record_management_observations"
        ]
    );
    assert_eq!(resolved.limits.max_advisory, 8);
    assert_eq!(resolved.limits.max_return_bytes, 65536);
    assert_eq!(resolved.limits.max_assessments, ASSESSMENT_MAX_ASSESSMENTS);
    let hash = resolved.assessment.policy_hash.clone().unwrap();
    assert!(hash.starts_with("sha256:") && hash.len() == 71, "{hash}");
    // The same document, ceiling and as_of give the same policy and hash.
    assert_eq!(resolve(&document, 5), resolved);
    // Another effective policy has another hash.
    let other = doc(json!([{"version": 1, "advice": {"collect_user_reply": false}}]));
    assert_ne!(
        resolve(&other, 5).assessment.policy_hash,
        resolved.assessment.policy_hash
    );
    // The resolved policy is one the compose step accepts.
    resolved.assessment.clone().capped().unwrap();
}

#[test]
fn protected_advice_cannot_be_named_at_all() {
    for name in PROTECTED_ADVICE {
        for value in [false, true] {
            let document = doc(json!([{"version": 1, "advice": {name: value}}]));
            let issues = validate_document(&document, &registry(), &ceiling());
            assert_eq!(issues.len(), 1, "{name}={value}");
            assert_eq!(issues[0].code, PolicyIssueCode::ProtectedRule);
            assert_eq!(issues[0].rule.as_deref(), Some(name));
            assert!(resolve_policy(Some(&document), &registry(), &ceiling(), 1).is_err());
        }
    }
}

#[test]
fn an_unknown_rule_is_refused_and_its_name_is_never_echoed() {
    let document = doc(json!([{"version": 1, "advice": {"rm -rf / ; secret-token-0123": false}}]));
    let issues = validate_document(&document, &registry(), &ceiling());
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, PolicyIssueCode::UnknownRule);
    let shown = format!(
        "{} {}",
        issues[0],
        serde_json::to_string(&issues[0]).unwrap()
    );
    assert!(
        !shown.contains("rm -rf") && !shown.contains("secret-token"),
        "{shown}"
    );
}

#[test]
fn keys_the_format_does_not_have_are_refused_so_there_is_no_script_or_expression() {
    for bad in [
        json!({"format": 1, "policy_id": "p", "script": "x"}),
        json!({"format": 1, "policy_id": "p", "version": [{"version": 1, "expression": "n > 3"}]}),
        json!({"format": 1, "policy_id": "p", "version": [{"version": 1, "limits": {"max_advisory": 3, "formula": "x"}}]}),
        json!({"format": 1, "policy_id": "p", "version": [{"version": 1, "order": {"hard_reason_codes": []}}]}),
        json!({"format": 1, "policy_id": "p", "version": [{"version": 1, "advice": {"collect_user_reply": "when x"}}]}),
        json!({"format": 1, "policy_id": "p", "version": [{"version": 1, "limits": {"max_advisory": "3 + 4"}}]}),
    ] {
        assert!(
            serde_json::from_value::<PolicyDocument>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn limits_can_only_be_lowered_and_never_be_zero() {
    let at = |key: &str, value: u64| doc(json!([{"version": 1, "limits": {key: value}}]));
    for (key, cap) in [
        ("max_assessments", ASSESSMENT_MAX_ASSESSMENTS),
        ("max_advisory", ASSESSMENT_MAX_ADVISORY),
        ("max_candidates", ASSESSMENT_MAX_CANDIDATES),
        ("max_return_bytes", ASSESSMENT_MAX_RETURN_BYTES),
    ] {
        assert_eq!(codes(&at(key, cap as u64)), vec![], "{key} at the cap");
        assert_eq!(codes(&at(key, 1)), vec![], "{key} at 1");
        assert_eq!(
            codes(&at(key, cap as u64 + 1)),
            vec![("limit_out_of_range", Some(1))],
            "{key} above"
        );
        assert_eq!(
            codes(&at(key, 0)),
            vec![("limit_out_of_range", Some(1))],
            "{key} zero"
        );
    }
}

#[test]
fn a_project_value_above_the_hosts_ceiling_is_refused_and_the_ceiling_wins_without_a_project() {
    let mut host = ceiling();
    host.max_return_bytes = 4096;
    let document = doc(json!([{"version": 1, "limits": {"max_return_bytes": 65536}}]));
    let issues = validate_document(&document, &registry(), &host);
    assert_eq!(
        issues.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(),
        vec!["exceeds_ceiling"]
    );
    assert!(resolve_policy(Some(&document), &registry(), &host, 1).is_err());
    // Within the ceiling it is fine.
    let within = doc(json!([{"version": 1, "limits": {"max_return_bytes": 2048}}]));
    assert_eq!(
        resolve_policy(Some(&within), &registry(), &host, 1)
            .unwrap()
            .limits
            .max_return_bytes,
        2048
    );
    // No project policy: the ceiling still narrows the built-in one, and the policy says so and gets its own hash.
    let builtin = resolve_policy(None, &registry(), &host, 1).unwrap();
    assert_eq!(builtin.limits.max_return_bytes, 4096);
    assert_eq!(builtin.ceiling_applied, vec!["max_return_bytes"]);
    assert!(builtin.assessment.policy_hash.is_some());
}

#[test]
fn advice_the_host_forced_off_stays_off_and_cannot_be_turned_on() {
    let mut host = ceiling();
    host.forced_off.insert("collect_user_reply".into());
    let silent = doc(json!([{"version": 1, "advice": {"refresh_sources": false}}]));
    let resolved = resolve_policy(Some(&silent), &registry(), &host, 1).unwrap();
    assert_eq!(
        resolved.disabled_advice,
        vec!["collect_user_reply", "refresh_sources"]
    );
    assert_eq!(resolved.ceiling_applied, vec!["collect_user_reply"]);
    let wants_on = doc(json!([{"version": 1, "advice": {"collect_user_reply": true}}]));
    let issues = validate_document(&wants_on, &registry(), &host);
    assert_eq!(
        issues.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(),
        vec!["conflicts_with_ceiling"]
    );
    // Saying `false` for it agrees with the host.
    let agrees = doc(json!([{"version": 1, "advice": {"collect_user_reply": false}}]));
    assert!(validate_document(&agrees, &registry(), &host).is_empty());
}

#[test]
fn hard_guards_keep_their_slots_and_the_soft_codes_are_a_full_permutation() {
    let default = registry().reason_code_order;
    let hard = registry().hard_reason_codes;
    let mut reversed = soft_codes();
    reversed.reverse();
    let document = doc(json!([{"version": 1, "order": {"soft_reason_codes": reversed}}]));
    assert_eq!(codes(&document), vec![]);
    let resolved = resolve(&document, 1);
    assert_eq!(resolved.reason_code_order.len(), default.len());
    for (slot, code) in default.iter().enumerate() {
        if hard.contains(code) {
            assert_eq!(
                resolved.reason_code_order[slot], *code,
                "hard guard moved from slot {slot}"
            );
        }
    }
    let soft_now: Vec<&String> = resolved
        .reason_code_order
        .iter()
        .filter(|c| !hard.contains(&c.as_str()))
        .collect();
    let wanted: Vec<&String> = reversed.iter().collect();
    assert_eq!(soft_now, wanted);
    assert_eq!(
        resolved.assessment.hard_reason_codes,
        hard.iter().map(|c| c.to_string()).collect::<Vec<_>>()
    );
    // Anything that is not a full permutation of the soft codes is refused.
    let mut short = soft_codes();
    short.pop();
    let mut twice = soft_codes();
    twice[1] = twice[0].clone();
    let mut with_hard = soft_codes();
    with_hard[0] = "claim_conflict".into();
    let mut unknown = soft_codes();
    unknown[0] = "made_up_code".into();
    for bad in [short, twice, with_hard, unknown] {
        let document = doc(json!([{"version": 1, "order": {"soft_reason_codes": bad}}]));
        assert_eq!(
            codes(&document),
            vec![("invalid_order", Some(1))],
            "{bad:?}"
        );
    }
}

#[test]
fn the_version_in_force_follows_as_of_and_not_the_clock() {
    let document = doc(json!([
        {"version": 1, "effective_from": 0, "advice": {"collect_user_reply": false}},
        {"version": 2, "effective_from": 100, "advice": {"refresh_sources": false}}
    ]));
    assert_eq!(resolve(&document, 0).version, Some(1));
    assert_eq!(resolve(&document, 99).version, Some(1));
    assert_eq!(resolve(&document, 100).version, Some(2));
    assert_eq!(resolve(&document, 10_000).version, Some(2));
    // Before every version: the built-in policy, said as such.
    let later = doc(json!([{"version": 1, "effective_from": 50}]));
    assert_eq!(resolve(&later, 49).source, PolicySource::Builtin);
    assert_eq!(resolve(&later, 50).source, PolicySource::Project);
    // The version chosen for an old as_of does not change when later versions are appended.
    let grown = doc(json!([
        {"version": 1, "effective_from": 0, "advice": {"collect_user_reply": false}},
        {"version": 2, "effective_from": 100, "advice": {"refresh_sources": false}},
        {"version": 3, "effective_from": 200, "advice": {"inspect_claim_conflict": false}}
    ]));
    assert_eq!(resolve(&grown, 99), resolve(&document, 99));
    assert_eq!(resolve(&grown, 150), resolve(&document, 150));
}

#[test]
fn inheritance_is_explicit_and_the_nearest_version_wins() {
    let document = doc(json!([
        {"version": 1, "advice": {"collect_user_reply": false}},
        {"version": 2, "extends": 1, "advice": {"refresh_sources": false}},
        {"version": 3, "extends": "builtin", "advice": {"inspect_claim_conflict": false}},
        {"version": 4, "extends": 2, "advice": {"collect_user_reply": true}},
        {"version": 5, "advice": {"repair_required_materials": false}}
    ]));
    assert_eq!(codes(&document), vec![]);
    let at = |version: i64| {
        let mut document = document.clone();
        for entry in &mut document.versions {
            entry.effective_from = if entry.version as i64 <= version {
                0
            } else {
                1_000
            };
        }
        resolve(&document, 0)
    };
    assert_eq!(at(1).disabled_advice, vec!["collect_user_reply"]);
    assert_eq!(
        at(2).disabled_advice,
        vec!["collect_user_reply", "refresh_sources"]
    );
    // `extends = "builtin"` starts again from the defaults, whatever came before.
    assert_eq!(at(3).disabled_advice, vec!["inspect_claim_conflict"]);
    // The nearest version in the chain decides: version 4 turns back on what version 1 turned off.
    assert_eq!(at(4).disabled_advice, vec!["refresh_sources"]);
    assert_eq!(at(4).chain, vec!["builtin", "v1", "v2", "v4"]);
    // No `extends` is the same as `builtin`.
    assert_eq!(at(5).disabled_advice, vec!["repair_required_materials"]);
    assert_eq!(at(5).chain, vec!["builtin", "v5"]);
}

#[test]
fn extends_points_back_to_an_earlier_version_or_to_builtin_and_nowhere_else() {
    for (extends, ok) in [
        (json!("builtin"), true),
        (json!(1), true),
        (json!(2), false),
        (json!(3), false),
        (json!(0), false),
        (json!(9), false),
        (json!("previous"), false),
        (json!(""), false),
    ] {
        let document = doc(json!([{"version": 1}, {"version": 2, "extends": extends.clone()}]));
        let expected: Vec<(&str, Option<u32>)> = if ok {
            vec![]
        } else {
            vec![("invalid_extends", Some(2))]
        };
        assert_eq!(codes(&document), expected, "{extends}");
    }
    // A version cannot extend itself or a later one.
    assert_eq!(
        codes(&doc(json!([{"version": 1, "extends": 1}]))),
        vec![("invalid_extends", Some(1))]
    );
}

#[test]
fn versions_are_numbered_from_one_without_gaps_and_effective_times_never_go_back() {
    assert_eq!(
        codes(&doc(json!([{"version": 2}]))),
        vec![("version_sequence", Some(2))]
    );
    assert_eq!(
        codes(&doc(json!([{"version": 1}, {"version": 1}]))),
        vec![("version_sequence", Some(1))]
    );
    assert_eq!(
        codes(&doc(json!([{"version": 1}, {"version": 3}]))),
        vec![("version_sequence", Some(3))]
    );
    assert_eq!(
        codes(&doc(
            json!([{"version": 1, "effective_from": 10}, {"version": 2, "effective_from": 9}])
        )),
        vec![("effective_from_order", Some(2))]
    );
    assert_eq!(
        codes(&doc(json!([{"version": 1, "effective_from": -1}]))),
        vec![("effective_from_order", Some(1))]
    );
    assert_eq!(
        codes(&doc(
            json!([{"version": 1, "effective_from": 10}, {"version": 2, "effective_from": 10}])
        )),
        vec![]
    );
    let many: Vec<Value> = (1..=MAX_POLICY_VERSIONS as u32 + 1)
        .map(|n| json!({"version": n}))
        .collect();
    assert_eq!(
        codes(&doc(Value::Array(many))),
        vec![("too_many_versions", None)]
    );
}

#[test]
fn the_format_the_scope_and_the_id_are_checked() {
    let mut newer = doc(json!([]));
    newer.format = 2;
    assert_eq!(codes(&newer), vec![("unsupported_format", None)]);
    let mut other_scope = doc(json!([]));
    other_scope.scope = "work".into();
    assert_eq!(codes(&other_scope), vec![("unsupported_scope", None)]);
    for id in [
        "",
        "Local",
        "-start",
        "has space",
        "semi;colon",
        &"x".repeat(MAX_POLICY_ID_BYTES + 1),
    ] {
        let mut named = doc(json!([]));
        named.policy_id = id.into();
        assert_eq!(codes(&named), vec![("invalid_policy_id", None)], "{id:?}");
    }
    for id in ["a", "local.advice-2_x", &"x".repeat(MAX_POLICY_ID_BYTES)] {
        let mut named = doc(json!([]));
        named.policy_id = id.into();
        assert_eq!(codes(&named), vec![], "{id:?}");
    }
    // Scope defaults to the only one there is.
    let absent: PolicyDocument =
        serde_json::from_value(json!({"format": 1, "policy_id": "p"})).unwrap();
    assert_eq!(absent.scope, POLICY_SCOPE_PROJECT);
    assert!(absent.versions.is_empty());
}

#[test]
fn a_note_is_bounded_plain_and_never_interpreted() {
    let ok = doc(
        json!([{"version": 1, "note": "Quiet the nudges while we stabilise the release", "advice": {}}]),
    );
    assert_eq!(codes(&ok), vec![]);
    // Text that looks like configuration changes nothing.
    let lure =
        doc(json!([{"version": 1, "note": "advice.collect_user_reply = false; max_advisory = 1"}]));
    assert_eq!(codes(&lure), vec![]);
    let resolved = resolve(&lure, 1);
    assert!(resolved.disabled_advice.is_empty());
    assert_eq!(resolved.limits.max_advisory, ASSESSMENT_MAX_ADVISORY);
    for bad in [
        "x".repeat(MAX_POLICY_NOTE_BYTES + 1),
        "line\nbreak".to_string(),
        "tab\there".to_string(),
    ] {
        let document = doc(json!([{"version": 1, "note": bad}]));
        assert_eq!(codes(&document), vec![("invalid_note", Some(1))], "{bad:?}");
    }
}

#[test]
fn a_rule_deprecated_in_a_version_cannot_be_set_from_then_on_but_older_versions_keep_their_meaning()
{
    let mut aged = registry();
    for rule in &mut aged.optional_advice {
        if rule.id == "refresh_sources" {
            rule.deprecated_in = Some(3);
        }
    }
    let document = doc(json!([
        {"version": 1},
        {"version": 2, "advice": {"refresh_sources": false}},
        {"version": 3, "extends": 2},
        {"version": 4, "extends": 2, "advice": {"refresh_sources": true}}
    ]));
    let issues: Vec<(&str, Option<u32>)> = validate_document(&document, &aged, &ceiling())
        .into_iter()
        .map(|i| (i.code.as_str(), i.version))
        .collect();
    assert_eq!(issues, vec![("deprecated_rule", Some(4))]);
    // Without the offending version the older setting is still honoured in version 3.
    let mut fine = document.clone();
    fine.versions.pop();
    let resolved = resolve_policy(Some(&fine), &aged, &ceiling(), 10).unwrap();
    assert_eq!(resolved.version, Some(3));
    assert_eq!(resolved.disabled_advice, vec!["refresh_sources"]);
    assert_eq!(
        resolve_policy(Some(&fine), &aged, &ceiling(), 0)
            .unwrap()
            .version,
        Some(3)
    );
}

#[test]
fn a_change_may_only_append_versions() {
    let current = doc(json!([
        {"version": 1, "advice": {"collect_user_reply": false}},
        {"version": 2, "extends": 1, "effective_from": 5}
    ]));
    let mut appended = current.clone();
    appended
        .versions
        .push(serde_json::from_value(json!({"version": 3, "extends": 1})).unwrap());
    assert!(check_history_preserved(&current, &appended).is_ok());
    assert!(check_history_preserved(&current, &current).is_ok());
    let refused = |candidate: &PolicyDocument| {
        check_history_preserved(&current, candidate)
            .unwrap_err()
            .code
            == PolicyIssueCode::HistoryChanged
    };
    let mut edited = appended.clone();
    edited.versions[0]
        .advice
        .insert("collect_user_reply".into(), true);
    assert!(refused(&edited));
    let mut shifted = appended.clone();
    shifted.versions[1].effective_from = 6;
    assert!(refused(&shifted));
    let mut removed = current.clone();
    removed.versions.pop();
    assert!(refused(&removed));
    let mut swapped = appended.clone();
    swapped.versions.swap(0, 1);
    assert!(refused(&swapped));
    let mut renamed = appended.clone();
    renamed.policy_id = "another-id".into();
    assert!(refused(&renamed));
    // A rollback is a new version that restates an older one, and it resolves like it.
    let mut rolled_back = current.clone();
    rolled_back.versions.push(
        serde_json::from_value(
            json!({"version": 3, "effective_from": 9, "advice": {"collect_user_reply": false}}),
        )
        .unwrap(),
    );
    assert!(check_history_preserved(&current, &rolled_back).is_ok());
    assert_eq!(
        resolve(&rolled_back, 9).disabled_advice,
        resolve(&current, 5).disabled_advice
    );
}

#[test]
fn a_change_is_described_field_by_field() {
    let before = resolve_policy(None, &registry(), &ceiling(), 1).unwrap();
    let document = doc(
        json!([{"version": 1, "advice": {"collect_user_reply": false}, "limits": {"max_advisory": 4}}]),
    );
    let after = resolve(&document, 1);
    let changes = describe_change(&before, &after);
    let fields: Vec<&str> = changes.iter().map(|c| c.field.as_str()).collect();
    assert_eq!(
        fields,
        vec!["policy", "advice.collect_user_reply", "limits.max_advisory"]
    );
    let advice = changes
        .iter()
        .find(|c| c.field == "advice.collect_user_reply")
        .unwrap();
    assert_eq!(
        (advice.before.as_str(), advice.after.as_str()),
        ("on", "off")
    );
    let limit = changes
        .iter()
        .find(|c| c.field == "limits.max_advisory")
        .unwrap();
    assert_eq!((limit.before.as_str(), limit.after.as_str()), ("32", "4"));
    assert!(describe_change(&after, &after).is_empty());
}

#[test]
fn a_policy_cannot_withhold_protected_or_unknown_advice_even_when_built_by_hand() {
    for name in PROTECTED_ADVICE.iter().chain(["made_up"].iter()) {
        let mut policy = AssessmentPolicy::default();
        policy.disabled_advice = vec![(*name).to_string()];
        assert!(policy.capped().is_err(), "{name}");
    }
    let mut policy = AssessmentPolicy::default();
    policy.disabled_advice = OPTIONAL_ADVICE.iter().map(|c| c.to_string()).collect();
    assert!(policy.clone().capped().is_ok());
    assert!(
        policy.withholds("collect_user_reply")
            && !policy.withholds("query_original_operation_result")
    );
}

#[test]
fn the_registry_agrees_with_the_closed_advice_set_and_keeps_hard_guards_out_of_reach() {
    let advice: BTreeSet<&str> = ADVISORY_ACTION_CODES.iter().copied().collect();
    for code in OPTIONAL_ADVICE.iter().chain(PROTECTED_ADVICE.iter()) {
        assert!(advice.contains(code), "{code} is not an advisory action");
    }
    for code in OPTIONAL_ADVICE {
        assert!(
            !PROTECTED_ADVICE.contains(&code),
            "{code} is both optional and protected"
        );
    }
    for code in registry().soft_reason_codes() {
        assert!(
            !DEFAULT_HARD_REASON_CODES.contains(&code),
            "{code} is a hard guard"
        );
    }
    // Every reason code is either hard or soft, so a permutation of the soft ones leaves nothing out.
    assert_eq!(
        registry().soft_reason_codes().len() + DEFAULT_HARD_REASON_CODES.len(),
        DEFAULT_REASON_CODE_ORDER.len()
    );
}

fn identity(policy: &AssessmentPolicy) -> AssessmentIdentity {
    AssessmentIdentity {
        project_id: Some("01M1YJBR5PW6QXYGABADJVAJPC".into()),
        work_key: Some("AWR-DEC-040".into()),
        work_id: None,
        branch_id: None,
        contract_fingerprint: Some("contract-a".into()),
        policy_id: policy.policy_id.clone(),
        policy_version: policy.policy_version,
        policy_hash: policy.policy_hash.clone(),
        input_summary: None,
        as_of: Some(7),
        verified_main_sha: None,
        fact_snapshot_hash: None,
    }
}

fn advice(code: &str) -> AdvisoryAction {
    AdvisoryAction {
        code: code.into(),
        when: Some("fixture".into()),
        query_refs: vec![],
        limits: vec![],
        reevaluation: None,
    }
}

fn compose(policy: AssessmentPolicy, offered: &[&str]) -> AssessmentEnvelope {
    compose_assessment_envelope(AssessmentComposeInput {
        identity: identity(&policy),
        as_of: 7,
        policy,
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: Default::default(),
        assessments: vec![],
        advisory_actions: offered.iter().map(|code| advice(code)).collect(),
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: Default::default(),
        legacy_source_views: vec![],
        candidates_seen: 0,
        scan_ops: None,
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap()
}

fn offered_codes(envelope: &AssessmentEnvelope) -> Vec<&str> {
    envelope
        .advisory_actions
        .iter()
        .map(|a| a.code.as_str())
        .collect()
}

#[test]
fn advice_a_policy_withholds_is_counted_and_said_and_the_rest_is_kept() {
    let document = doc(
        json!([{"version": 1, "advice": {"collect_user_reply": false, "record_management_observations": false}}]),
    );
    let policy = resolve(&document, 7).assessment;
    let envelope = compose(
        policy,
        &[
            "collect_user_reply",
            "query_original_operation_result",
            "record_management_observations",
            "refresh_sources",
        ],
    );
    assert_eq!(
        offered_codes(&envelope),
        vec!["query_original_operation_result", "refresh_sources"]
    );
    assert_eq!(envelope.limits.omitted_count, 2);
    assert_eq!(
        envelope.limits.truncation_reason.as_deref(),
        Some("advice_withheld_by_policy")
    );
    assert!(envelope.identity.policy_hash.is_some());
    // Nothing withheld: nothing counted, nothing said.
    let plain = compose(
        AssessmentPolicy::default(),
        &["collect_user_reply", "refresh_sources"],
    );
    assert_eq!(
        offered_codes(&plain),
        vec!["collect_user_reply", "refresh_sources"]
    );
    assert_eq!(plain.limits.omitted_count, 0);
    assert_eq!(plain.limits.truncation_reason, None);
}

#[test]
fn when_every_advice_is_withheld_the_envelope_says_no_advice_and_why() {
    let mut policy = AssessmentPolicy::default();
    policy.disabled_advice = OPTIONAL_ADVICE.iter().map(|c| c.to_string()).collect();
    let envelope = compose(
        policy,
        &["continue_prepare_current_work", "collect_user_reply"],
    );
    assert_eq!(offered_codes(&envelope), vec!["no_advisory"]);
    assert_eq!(
        envelope.advisory_actions[0].when.as_deref(),
        Some("advice_withheld_by_policy")
    );
    assert_eq!(envelope.limits.omitted_count, 2);
}

#[test]
fn protected_advice_survives_a_policy_that_withholds_everything_optional() {
    let mut policy = AssessmentPolicy::default();
    policy.disabled_advice = OPTIONAL_ADVICE.iter().map(|c| c.to_string()).collect();
    let envelope = compose(
        policy,
        &[
            "query_original_operation_result",
            "continue_prepare_current_work",
        ],
    );
    assert_eq!(
        offered_codes(&envelope),
        vec!["query_original_operation_result"]
    );
    assert_eq!(envelope.limits.omitted_count, 1);
}

#[test]
fn withholding_advice_is_not_a_resource_overrun_and_does_not_change_the_hard_gate() {
    let mut policy = AssessmentPolicy::default();
    policy.disabled_advice = vec!["collect_user_reply".into()];
    let withheld = compose(policy, &["collect_user_reply"]);
    let plain = compose(AssessmentPolicy::default(), &["collect_user_reply"]);
    assert_eq!(withheld.hard_gate, plain.hard_gate);
    // A real overrun still lowers a passing gate to unknown.
    let mut tight = AssessmentPolicy::default();
    tight.max_advisory = 1;
    let overrun = compose(tight, &["collect_user_reply", "refresh_sources"]);
    assert_eq!(
        overrun.limits.truncation_reason.as_deref(),
        Some("advisory_exceeded")
    );
    assert_ne!(overrun.hard_gate, HardGateOutcome::Pass);
}

#[test]
fn the_envelope_of_the_same_policy_and_input_is_the_same() {
    let document = doc(
        json!([{"version": 1, "advice": {"collect_user_reply": false}, "limits": {"max_advisory": 3}}]),
    );
    let policy = resolve(&document, 7).assessment;
    let first = compose(
        policy.clone(),
        &[
            "collect_user_reply",
            "refresh_sources",
            "continue_prepare_current_work",
        ],
    );
    let second = compose(
        policy,
        &[
            "collect_user_reply",
            "refresh_sources",
            "continue_prepare_current_work",
        ],
    );
    assert_eq!(first.assessment_hash, second.assessment_hash);
    // The policy that shaped the envelope is part of what its hash covers.
    let other = compose(
        AssessmentPolicy::default(),
        &[
            "collect_user_reply",
            "refresh_sources",
            "continue_prepare_current_work",
        ],
    );
    assert_ne!(first.assessment_hash, other.assessment_hash);
}
