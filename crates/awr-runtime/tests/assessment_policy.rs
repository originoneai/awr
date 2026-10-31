//! DEC-040 in the runtime: the policy in force, the preview that writes nothing, the one confirmed write, and the
//! explanation that carries the policy that shaped it.
use awr_core::*;
use awr_runtime::*;
use awr_source::{ASSESSMENT_POLICY_FILE, fingerprint};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-policy-rt-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
    fn path(&self) -> PathBuf {
        self.0.join(ASSESSMENT_POLICY_FILE)
    }
    fn write(&self, text: &str) {
        fs::write(self.path(), text).unwrap();
    }
    fn text(&self) -> String {
        fs::read_to_string(self.path()).unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ceiling() -> PolicyCeiling {
    PolicyCeiling::builtin()
}

const ONE: &str = "format = 1\npolicy_id = \"local-advice\"\n\n[[version]]\nversion = 1\n\n[version.advice]\ncontinue_prepare_current_work = false\n";
const TWO: &str = "format = 1\npolicy_id = \"local-advice\"\n\n[[version]]\nversion = 1\n\n[version.advice]\ncontinue_prepare_current_work = false\n\n[[version]]\nversion = 2\neffective_from = 50\n\n[version.advice]\nrefresh_sources = false\n";

fn sample_prepare() -> Value {
    json!({
        "version": 1,
        "stage": "prepared",
        "project_revision": 7,
        "work": {
            "id": "01WORK",
            "external_key": "W",
            "status": "ready",
            "revision": 1,
            "source_ref": {"source_revision": "3"},
            "next_action": "Do it"
        },
        "ready": true,
        "diagnostics": [],
        "active_claims": [],
        "context": {
            "completeness": {"complete": true, "issues": [], "branch_id": "main"},
            "work_context": {
                "rendered_context": "REQUIRED CONTEXT BODY THAT IS LONG ENOUGH TO DETECT DUPLICATION IF COPIED INTO THE ENVELOPE ACCIDENTALLY",
                "context_hash": "abc"
            }
        },
        "context_consumed": false,
        "completion_claimed": false,
        "management": {
            "contract_fingerprint": "c1",
            "decision": {
                "version": 1,
                "mode": "lightweight",
                "reasons": [],
                "unknown_observations": [],
                "reevaluation_signals": [],
                "required_actions": [
                    "preserve_identity_intent_scope_and_current_state",
                    "consume_required_context_and_hard_rules",
                    "retain_completion_basis_and_actual_outcome",
                    "check_source_versions_permissions_claims_and_request_identity"
                ],
                "optional_maintenance": [],
                "completion_policy": "unchanged_source_policy",
                "execution_admission": "not_granted_by_management_classification"
            },
            "observation_basis": "host_assertion_not_independently_verified",
            "admission_gaps": [],
            "record_required": false,
            "next_action": "follow_required_actions_and_existing_workflow"
        },
        "continuity": {"state": "available", "waits": []},
        "next_action": "consume_context_then_start_or_resume_session"
    })
}

fn explain(root: Option<&Project>) -> Value {
    let out = attach_assessment_explanation(
        sample_prepare(),
        &AttachExplanationOptions {
            enabled: true,
            policy_root: root.map(|p| p.0.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    out[ASSESSMENT_EXPLAIN_FIELD].clone()
}

fn codes(view: &Value) -> Vec<String> {
    view["envelope"]["advisory_actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["code"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn without_a_file_the_builtin_policy_applies_and_nothing_is_said_about_it() {
    let p = Project::new();
    let in_force = policy_in_force(&p.0, 7, &ceiling());
    assert_eq!(in_force.resolved.source, PolicySource::Builtin);
    assert_eq!(in_force.resolved.assessment, AssessmentPolicy::default());
    assert!(in_force.diagnostics.is_empty() && in_force.file_fingerprint.is_none());
    assert_eq!(in_force.notice(), None);
    let view = explain(Some(&p));
    assert!(view.get("policy").is_none(), "{view}");
    assert_eq!(codes(&view), vec!["continue_prepare_current_work"]);
    // The same as an explanation that never looked for a file.
    assert_eq!(
        view["envelope"]["assessment_hash"],
        explain(None)["envelope"]["assessment_hash"]
    );
}

#[test]
fn a_file_is_in_force_by_as_of_and_the_notice_names_the_policy_and_the_file() {
    let p = Project::new();
    p.write(TWO);
    let at_7 = policy_in_force(&p.0, 7, &ceiling());
    assert_eq!(at_7.resolved.version, Some(1));
    assert_eq!(
        at_7.resolved.disabled_advice,
        vec!["continue_prepare_current_work"]
    );
    let at_50 = policy_in_force(&p.0, 50, &ceiling());
    assert_eq!(at_50.resolved.version, Some(2));
    assert_eq!(at_50.resolved.disabled_advice, vec!["refresh_sources"]);
    let notice = at_7.notice().unwrap();
    assert_eq!(
        (
            notice.source.as_str(),
            notice.policy_id.as_str(),
            notice.version
        ),
        ("project", "local-advice", Some(1))
    );
    assert_eq!(notice.file_fingerprint, Some(fingerprint(TWO.as_bytes())));
    assert_eq!(notice.policy_hash, at_7.resolved.assessment.policy_hash);
    assert!(notice.diagnostics.is_empty());
}

#[test]
fn an_unusable_file_is_never_a_partial_policy() {
    let p = Project::new();
    for (text, code) in [
        (
            "format = 1\npolicy_id = \"p\"\n[[version]]\nversion = 1\n[version.advice]\nquery_original_operation_result = false\ncollect_user_reply = false\n",
            "project_policy_refused:protected_rule",
        ),
        (
            "format = 9\npolicy_id = \"p\"\n",
            "project_policy_newer_format",
        ),
        (
            "format = 1\npolicy_id = \"p\"\nsurprise = true\n",
            "project_policy_invalid",
        ),
        ("not toml {", "project_policy_invalid"),
    ] {
        p.write(text);
        let in_force = policy_in_force(&p.0, 7, &ceiling());
        assert_eq!(in_force.resolved.source, PolicySource::Builtin, "{code}");
        assert_eq!(
            in_force.resolved.assessment,
            AssessmentPolicy::default(),
            "{code}"
        );
        assert_eq!(in_force.diagnostics, vec![code.to_string()]);
        let shown = serde_json::to_string(&in_force.notice().unwrap()).unwrap();
        assert!(
            shown.contains(code) && !shown.contains("surprise"),
            "{shown}"
        );
        // The advice is the built-in advice, including the one the half-read file tried to switch off.
        let view = explain(Some(&p));
        assert_eq!(codes(&view), vec!["continue_prepare_current_work"]);
        assert_eq!(view["policy"]["diagnostics"], json!([code]));
    }
    // A link in the place of the file is unreadable, not followed.
    #[cfg(unix)]
    {
        fs::remove_file(p.path()).unwrap();
        let elsewhere = p.0.join("elsewhere.toml");
        fs::write(&elsewhere, ONE).unwrap();
        std::os::unix::fs::symlink(&elsewhere, p.path()).unwrap();
        let in_force = policy_in_force(&p.0, 7, &ceiling());
        assert_eq!(
            in_force.diagnostics,
            vec!["project_policy_unreadable".to_string()]
        );
        assert_eq!(in_force.resolved.assessment, AssessmentPolicy::default());
    }
}

#[test]
fn an_explanation_carries_the_policy_that_shaped_it_and_the_advice_it_withheld() {
    let p = Project::new();
    p.write(ONE);
    let view = explain(Some(&p));
    assert_eq!(codes(&view), vec!["no_advisory"]);
    assert_eq!(
        view["envelope"]["advisory_actions"][0]["when"],
        "advice_withheld_by_policy"
    );
    assert_eq!(view["envelope"]["limits"]["omitted_count"], 1);
    assert_eq!(
        view["envelope"]["limits"]["truncation_reason"],
        "advice_withheld_by_policy"
    );
    assert_eq!(view["envelope"]["identity"]["policy_id"], "local-advice");
    assert_eq!(view["envelope"]["identity"]["policy_version"], 1);
    let hash = view["envelope"]["identity"]["policy_hash"]
        .as_str()
        .unwrap();
    assert_eq!(view["policy"]["policy_hash"], hash);
    assert_eq!(view["policy"]["source"], "project");
    assert_eq!(view["policy"]["version"], 1);
    // The hard gate and the layers are not the policy's to change.
    assert_eq!(
        view["envelope"]["hard_gate"],
        explain(None)["envelope"]["hard_gate"]
    );
    // Asked again, the same explanation.
    assert_eq!(
        explain(Some(&p))["envelope"]["assessment_hash"],
        view["envelope"]["assessment_hash"]
    );
    // A policy handed in already resolved wins over the file, and the explanation says nothing of the file.
    let mut custom = AssessmentPolicy::default();
    custom.policy_id = "handed-in".into();
    custom.policy_version = 3;
    custom.disabled_advice = vec!["collect_user_reply".into()];
    let handed = attach_assessment_explanation(
        sample_prepare(),
        &AttachExplanationOptions {
            enabled: true,
            policy_root: Some(p.0.clone()),
            policy: Some(custom),
            ..Default::default()
        },
    )
    .unwrap();
    let handed = &handed[ASSESSMENT_EXPLAIN_FIELD];
    assert!(handed.get("policy").is_none());
    assert_eq!(handed["envelope"]["identity"]["policy_id"], "handed-in");
    assert_eq!(handed["envelope"]["identity"]["policy_version"], 3);
    // Not the file's advice (the file withholds this one), and not the built-in policy's identity.
    assert_eq!(codes(handed), vec!["continue_prepare_current_work"]);
}

#[test]
fn a_preview_writes_nothing_and_names_what_would_change_and_what_to_confirm() {
    let p = Project::new();
    let before = ceiling();
    let preview = preview_policy_change(&p.0, ONE.as_bytes(), 7, &before).unwrap();
    assert!(
        preview.ok && !preview.writes && !preview.unchanged,
        "{preview:?}"
    );
    assert_eq!(preview.current_fingerprint, None);
    assert_eq!(preview.candidate_fingerprint, fingerprint(ONE.as_bytes()));
    assert_eq!(preview.appended_versions, vec![1]);
    let fields: Vec<&str> = preview.changes.iter().map(|c| c.field.as_str()).collect();
    assert_eq!(
        fields,
        vec!["policy", "advice.continue_prepare_current_work"]
    );
    assert!(!p.path().exists(), "a preview must not create the file");
    p.write(ONE);
    let again = preview_policy_change(&p.0, TWO.as_bytes(), 60, &before).unwrap();
    assert!(again.ok);
    assert_eq!(again.current_fingerprint, Some(fingerprint(ONE.as_bytes())));
    assert_eq!(again.appended_versions, vec![2]);
    assert_eq!(again.in_force_before.version, Some(1));
    assert_eq!(again.in_force_after.as_ref().unwrap().version, Some(2));
    assert_eq!(p.text(), ONE, "a preview must not change the file");
    // Previewing the file as it is says so.
    let same = preview_policy_change(&p.0, ONE.as_bytes(), 7, &before).unwrap();
    assert!(same.unchanged && same.ok && same.changes.is_empty());
}

#[test]
fn a_change_is_written_only_with_both_fingerprints_and_a_valid_candidate() {
    let p = Project::new();
    let theirs = fingerprint(ONE.as_bytes());
    // The candidate is not the one previewed.
    let error = apply_policy_change(
        &p.0,
        ONE.as_bytes(),
        None,
        &fingerprint(b"other"),
        7,
        &ceiling(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    // The file is not what the caller read.
    let error = apply_policy_change(
        &p.0,
        ONE.as_bytes(),
        Some("sha256:aa"),
        &theirs,
        7,
        &ceiling(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    assert!(!p.path().exists());
    // Invalid: every code is told, and nothing is written.
    let invalid = "format = 1\npolicy_id = \"p\"\n[[version]]\nversion = 1\n[version.advice]\nno_advisory = false\nmade_up = true\n[version.limits]\nmax_advisory = 0\n";
    let error = apply_policy_change(
        &p.0,
        invalid.as_bytes(),
        None,
        &fingerprint(invalid.as_bytes()),
        7,
        &ceiling(),
    )
    .unwrap_err();
    let shown = error.to_string();
    assert!(matches!(error, Error::InvalidInput(_)));
    for code in ["protected_rule", "unknown_rule", "limit_out_of_range"] {
        assert!(shown.contains(code), "{shown}");
    }
    assert!(!shown.contains("made_up"), "{shown}");
    assert!(!p.path().exists());
    // Confirmed: written exactly.
    let applied = apply_policy_change(&p.0, ONE.as_bytes(), None, &theirs, 7, &ceiling()).unwrap();
    assert!(applied.wrote);
    assert_eq!(
        (
            applied.before_fingerprint,
            applied.after_fingerprint.as_str()
        ),
        (None, theirs.as_str())
    );
    assert_eq!(p.text(), ONE);
    // Confirming what is already there writes nothing.
    let again =
        apply_policy_change(&p.0, ONE.as_bytes(), Some(&theirs), &theirs, 7, &ceiling()).unwrap();
    assert!(!again.wrote);
}

#[test]
fn a_stale_preview_is_refused_after_the_file_moved_on() {
    let p = Project::new();
    p.write(ONE);
    let old = fingerprint(ONE.as_bytes());
    let two = fingerprint(TWO.as_bytes());
    apply_policy_change(&p.0, TWO.as_bytes(), Some(&old), &two, 60, &ceiling()).unwrap();
    // Someone else previewed a change on top of ONE; it no longer applies.
    let other = format!("{ONE}\n[[version]]\nversion = 2\neffective_from = 70\n");
    let error = apply_policy_change(
        &p.0,
        other.as_bytes(),
        Some(&old),
        &fingerprint(other.as_bytes()),
        60,
        &ceiling(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    assert_eq!(p.text(), TWO);
}

#[test]
fn the_versions_already_in_the_file_cannot_be_rewritten_but_a_rollback_is_a_new_version() {
    let p = Project::new();
    p.write(TWO);
    let seen = fingerprint(TWO.as_bytes());
    let edited = TWO.replace(
        "continue_prepare_current_work = false",
        "continue_prepare_current_work = true",
    );
    let error = apply_policy_change(
        &p.0,
        edited.as_bytes(),
        Some(&seen),
        &fingerprint(edited.as_bytes()),
        60,
        &ceiling(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("history_changed"), "{error}");
    let dropped = ONE;
    let error = apply_policy_change(
        &p.0,
        dropped.as_bytes(),
        Some(&seen),
        &fingerprint(dropped.as_bytes()),
        60,
        &ceiling(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("history_changed"), "{error}");
    assert_eq!(p.text(), TWO);
    // Rolling back to the advice of version 1 is version 3 saying so.
    let rollback = format!("{TWO}\n[[version]]\nversion = 3\neffective_from = 90\nextends = 1\n");
    apply_policy_change(
        &p.0,
        rollback.as_bytes(),
        Some(&seen),
        &fingerprint(rollback.as_bytes()),
        90,
        &ceiling(),
    )
    .unwrap();
    let v3 = policy_in_force(&p.0, 90, &ceiling());
    assert_eq!(v3.resolved.version, Some(3));
    assert_eq!(
        v3.resolved.disabled_advice,
        policy_in_force(&p.0, 10, &ceiling())
            .resolved
            .disabled_advice
    );
    // The past still resolves as it did.
    assert_eq!(
        policy_in_force(&p.0, 55, &ceiling()).resolved.version,
        Some(2)
    );
    assert_eq!(
        policy_in_force(&p.0, 10, &ceiling()).resolved.version,
        Some(1)
    );
}

#[test]
fn a_change_beyond_the_hosts_ceiling_is_refused_by_preview_and_by_apply() {
    let p = Project::new();
    let mut host = ceiling();
    host.max_return_bytes = 8192;
    let candidate = "format = 1\npolicy_id = \"p\"\n[[version]]\nversion = 1\n[version.limits]\nmax_return_bytes = 65536\n";
    let preview = preview_policy_change(&p.0, candidate.as_bytes(), 7, &host).unwrap();
    assert!(!preview.ok);
    assert_eq!(
        preview
            .issues
            .iter()
            .map(|i| i.code.as_str())
            .collect::<Vec<_>>(),
        vec!["exceeds_ceiling"]
    );
    assert!(preview.in_force_after.is_none() && preview.changes.is_empty());
    assert!(
        apply_policy_change(
            &p.0,
            candidate.as_bytes(),
            None,
            &fingerprint(candidate.as_bytes()),
            7,
            &host
        )
        .is_err()
    );
    assert!(!p.path().exists());
    // The same file in force under that ceiling is refused too, and the built-in policy narrowed by the host applies.
    p.write(candidate);
    let in_force = policy_in_force(&p.0, 7, &host);
    assert_eq!(
        in_force.diagnostics,
        vec!["project_policy_refused:exceeds_ceiling".to_string()]
    );
    assert_eq!(in_force.resolved.limits.max_return_bytes, 8192);
}

#[test]
fn show_lists_the_versions_the_file_holds_and_the_one_in_force() {
    let p = Project::new();
    let none = show_policy(&p.0, 7, &ceiling());
    assert!(none.versions.is_empty() && none.file_fingerprint.is_none());
    assert_eq!(none.in_force.source, PolicySource::Builtin);
    p.write(&format!(
        "{TWO}\n[[version]]\nversion = 3\neffective_from = 90\nextends = \"builtin\"\n"
    ));
    let shown = show_policy(&p.0, 60, &ceiling());
    assert_eq!(
        shown
            .versions
            .iter()
            .map(|v| (v.version, v.effective_from, v.extends.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, 0, "builtin"), (2, 50, "builtin"), (3, 90, "builtin")]
    );
    assert_eq!(shown.in_force.version, Some(2));
    assert_eq!(shown.as_of, 60);
    assert!(shown.file_fingerprint.is_some() && shown.diagnostics.is_empty());
}
