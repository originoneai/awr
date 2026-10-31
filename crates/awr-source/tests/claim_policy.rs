//! The optional `.awr/policy.toml` and `AWR_DEFAULT_CLAIM_TTL`: what they declare, their bounds, and
//! what a build that does not know a key or a version does with the file.
use awr_core::{Error, Id};
use awr_source::{ClaimPolicy, DefaultTtl, parse_default_ttl};
use std::{fs, path::PathBuf};

const MINUTE: u64 = 60_000;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

fn project(policy: Option<&str>) -> PathBuf {
    let root = std::env::temp_dir().join(format!("awr-claim-policy-{}", Id::new()));
    fs::create_dir_all(root.join(".awr")).unwrap();
    if let Some(text) = policy {
        fs::write(root.join(".awr/policy.toml"), text).unwrap();
    }
    root
}

#[test]
fn a_project_that_declares_nothing_keeps_the_behaviour_it_had() {
    let policy = ClaimPolicy::load_with(&project(None), None).unwrap();
    assert!(!policy.declared);
    assert_eq!(policy.default_ttl_ms(), None);
    assert_eq!(policy.renewal_ttl_ms(), None);
}

#[test]
fn the_file_declares_the_default_ttl_and_renewal_on_activity() {
    let root = project(Some(
        "version = 1\n\n[claims]\ndefault_ttl = \"4h\"\nrenew_on_activity = true\n",
    ));
    let policy = ClaimPolicy::load_with(&root, None).unwrap();
    assert!(policy.declared);
    assert_eq!(policy.default_ttl_ms(), Some(4 * HOUR));
    assert_eq!(policy.renewal_ttl_ms(), Some(4 * HOUR));
}

#[test]
fn renewal_is_on_with_a_default_unless_the_file_turns_it_off() {
    // A default that is not renewed drops the claim of a task that outlasts it; the file may still ask for that.
    let on = project(Some("version = 1\n[claims]\ndefault_ttl = \"90m\"\n"));
    let policy = ClaimPolicy::load_with(&on, None).unwrap();
    assert_eq!(policy.renewal_ttl_ms(), Some(90 * MINUTE));
    let off = project(Some(
        "version = 1\n[claims]\ndefault_ttl = \"90m\"\nrenew_on_activity = false\n",
    ));
    let policy = ClaimPolicy::load_with(&off, None).unwrap();
    assert_eq!(policy.default_ttl_ms(), Some(90 * MINUTE));
    assert_eq!(policy.renewal_ttl_ms(), None);
}

#[test]
fn a_file_without_claims_declares_no_default_and_none_turns_the_default_off() {
    let bare = project(Some("version = 1\n"));
    let policy = ClaimPolicy::load_with(&bare, None).unwrap();
    assert!(policy.declared);
    assert_eq!(policy.default_ttl_ms(), None);
    assert_eq!(policy.renewal_ttl_ms(), None);
    let none = project(Some("version = 1\n[claims]\ndefault_ttl = \"none\"\n"));
    let policy = ClaimPolicy::load_with(&none, None).unwrap();
    assert_eq!(policy.default_ttl_ms(), None);
    assert_eq!(policy.renewal_ttl_ms(), None);
}

#[test]
fn the_variable_overrides_the_file_and_alone_declares_a_policy() {
    let file = project(Some("version = 1\n[claims]\ndefault_ttl = \"4h\"\n"));
    assert_eq!(
        ClaimPolicy::load_with(&file, Some("30m"))
            .unwrap()
            .default_ttl_ms(),
        Some(30 * MINUTE)
    );
    assert_eq!(
        ClaimPolicy::load_with(&file, Some("none"))
            .unwrap()
            .default_ttl_ms(),
        None
    );
    // Without a file the variable is the whole policy, and a default is paired with renewal.
    let alone = ClaimPolicy::load_with(&project(None), Some("2h")).unwrap();
    assert!(alone.declared);
    assert_eq!(alone.default_ttl_ms(), Some(2 * HOUR));
    assert_eq!(alone.renewal_ttl_ms(), Some(2 * HOUR));
    // The file still decides renewal when both exist.
    let file = project(Some(
        "version = 1\n[claims]\ndefault_ttl = \"4h\"\nrenew_on_activity = false\n",
    ));
    let policy = ClaimPolicy::load_with(&file, Some("2h")).unwrap();
    assert_eq!(policy.default_ttl_ms(), Some(2 * HOUR));
    assert_eq!(policy.renewal_ttl_ms(), None);
}

#[test]
fn durations_are_whole_numbers_between_one_minute_and_seven_days() {
    for (text, expected) in [
        ("1m", MINUTE),
        ("60s", MINUTE),
        ("90s", 90_000),
        ("4h", 4 * HOUR),
        ("4H", 4 * HOUR),
        ("10080m", 7 * DAY),
        ("168h", 7 * DAY),
        ("7d", 7 * DAY),
        (" 2d ", 2 * DAY),
    ] {
        assert_eq!(
            parse_default_ttl(text).unwrap(),
            DefaultTtl::Millis(expected),
            "{text}"
        );
    }
    assert_eq!(parse_default_ttl("none").unwrap(), DefaultTtl::Off);
    assert_eq!(parse_default_ttl(" NONE ").unwrap(), DefaultTtl::Off);
    for text in [
        "",
        "0s",
        "59s",
        "0m",
        "0h",
        "8d",
        "169h",
        "10081m",
        "4",
        "h",
        "4 h",
        "-1h",
        "+4h",
        "1.5h",
        "4hours",
        "1w",
        "forever",
        "0x10m",
        "٤h",
        "99999999999999999999d",
    ] {
        match parse_default_ttl(text) {
            Err(Error::InvalidInput(message)) => {
                assert!(!message.is_empty(), "{text}");
            }
            other => panic!("{text:?} was accepted or refused with the wrong error: {other:?}"),
        }
    }
}

#[test]
fn a_key_or_a_version_this_build_does_not_know_points_to_a_newer_awr() {
    for text in [
        "version = 2\n",
        "version = 1\nprofile = \"delivery\"\n",
        "version = 1\n[claims]\nsomething_new = true\n",
        "version = 1\n[sessions]\nidle_ttl = \"12h\"\n",
    ] {
        match ClaimPolicy::load_with(&project(Some(text)), None) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("newer awr"), "{text}: {message}");
                assert!(message.contains(".awr/policy.toml"), "{text}: {message}");
            }
            other => panic!("{text:?}: expected Unsupported, got {other:?}"),
        }
    }
}

#[test]
fn a_file_that_is_not_a_policy_is_refused_without_echoing_it() {
    for text in [
        "",
        "[claims]\ndefault_ttl = \"4h\"\n",
        "version = \"one\"\n",
        "version = 1\n[claims]\nrenew_on_activity = \"yes\"\n",
        "version = 1\n[claims]\ndefault_ttl = 4\n",
        "version = 1\n[claims]\ndefault_ttl = \"S3CRET-forever\"\n",
        "version = 1\nthis is not toml S3CRET\n",
        "version = 1\n[claims\n",
    ] {
        match ClaimPolicy::load_with(&project(Some(text)), None) {
            Err(Error::InvalidInput(message)) => {
                assert!(message.contains(".awr/policy.toml"), "{text}: {message}");
                assert!(!message.contains("S3CRET"), "{text}: {message}");
            }
            other => panic!("{text:?}: expected InvalidInput, got {other:?}"),
        }
    }
}

#[test]
fn an_oversized_policy_or_one_that_is_a_link_is_refused() {
    let big = project(Some(&format!("version = 1\n# {}\n", "x".repeat(70 * 1024))));
    assert!(matches!(
        ClaimPolicy::load_with(&big, None),
        Err(Error::InvalidInput(_))
    ));
    #[cfg(unix)]
    {
        let root = project(None);
        let outside = std::env::temp_dir().join(format!("awr-claim-policy-{}.toml", Id::new()));
        fs::write(&outside, "version = 1\n[claims]\ndefault_ttl = \"4h\"\n").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".awr/policy.toml")).unwrap();
        assert!(ClaimPolicy::load_with(&root, None).is_err());
    }
}

#[test]
fn the_variable_is_checked_and_never_echoed() {
    for value in ["S3CRET", "0s", "8d", ""] {
        match ClaimPolicy::load_with(&project(None), Some(value)) {
            Err(Error::InvalidInput(message)) => {
                assert!(
                    message.contains("AWR_DEFAULT_CLAIM_TTL"),
                    "{value}: {message}"
                );
                assert!(!message.contains("S3CRET"), "{value}: {message}");
            }
            other => panic!("{value:?}: expected InvalidInput, got {other:?}"),
        }
    }
}
