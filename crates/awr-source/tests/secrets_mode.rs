//! Tiered sensitive-source admission end to end: strict rejects the refresh,
//! a project's warn policy admits the source with a queryable receipt, and
//! placeholders never block in either mode.
use awr_core::*;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use std::{fs, path::PathBuf};

const CREDENTIAL_LEDGER: &str = "work_items:\n- id: W\n  title: Carry a key\n  status: planned\n  next_action: Start\n  acceptance: [Deliver]\n  api_key: sk-ant-abcdefghijklmnopqrstuvwxyz012345\n";
const PLACEHOLDER_LEDGER: &str = "work_items:\n- id: W\n  title: Placeholders only\n  status: planned\n  next_action: Start\n  acceptance: [Deliver]\n  api_key: ${API_KEY}\n  other: [redacted]\n";

struct Fixture {
    root: PathBuf,
    store: Store,
}
impl Fixture {
    fn new(ledger: &str, policy: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!("awr-secrets-mode-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::write(root.join("ledger.yaml"), ledger).unwrap();
        if let Some(policy) = policy {
            fs::write(root.join(".awr/policy.toml"), policy).unwrap();
        }
        let store = Store::open(&root.join(".awr/state.db")).unwrap();
        Self { root, store }
    }
    fn manifest(&self) -> Manifest {
        Manifest::load(&self.root).unwrap_or_else(|_| {
            fs::write(
                self.root.join(".awr/project.toml"),
                "[project]\nname='Secrets mode'\nexternal_key='secrets-mode'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='ledger.yaml'\nadapter='yaml-ledger-v1'\n",
            )
            .unwrap();
            Manifest::load(&self.root).unwrap()
        })
    }
    fn index(&mut self) -> Result<awr_source::IndexReport> {
        let manifest = self.manifest();
        index_project(&mut self.store, &self.root, &manifest, false)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn strict_default_rejects_credential_ledger_and_rejects_it_again_after_switching_back() {
    let mut f = Fixture::new(CREDENTIAL_LEDGER, None);
    let report = f.index().unwrap();
    assert!(
        !report.ok
            && report
                .issues
                .iter()
                .any(|issue| issue.code == "RuleViolation"),
        "no policy file keeps the historical strict refusal: issues={:?}",
        report.issues
    );
    let mut f = Fixture::new(
        CREDENTIAL_LEDGER,
        Some(
            "version = 1\n\n[secrets]\nmode = \"warn\"\nenabled_by = \"operator-a\"\nenabled_at = 1794000000000\n",
        ),
    );
    let report = f.index().unwrap();
    assert!(report.ok, "{:?}", report.issues);
    assert_eq!(report.sensitive_warnings.len(), 1);
    let warning = &report.sensitive_warnings[0];
    assert!(warning.locator.contains("ledger.yaml"));
    assert_eq!(warning.category, "credential");
    assert_eq!(warning.line, Some(7));
    // The receipt is queryable and carries the operator provenance.
    let project = f.store.project_by_root(&f.root).unwrap();
    let events = f.store.secrets_warned_events(project.id, 10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["enabled_by"], "operator-a");
    let findings = events[0]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["category"], "credential");
    // Nothing sensitive is echoed by the receipt.
    assert!(!events[0].to_string().contains("sk-ant"));
}

#[test]
fn placeholders_never_block_in_strict_or_warn_mode() {
    let mut strict = Fixture::new(PLACEHOLDER_LEDGER, None);
    let report = strict.index().unwrap();
    assert!(report.ok && report.sensitive_warnings.is_empty());
    let mut warn = Fixture::new(
        PLACEHOLDER_LEDGER,
        Some(
            "version = 1\n\n[secrets]\nmode = \"warn\"\nenabled_by = \"operator-b\"\nenabled_at = 1794000000001\n",
        ),
    );
    let report = warn.index().unwrap();
    assert!(report.ok && report.sensitive_warnings.is_empty());
}

#[test]
fn strict_switch_restores_the_refusal() {
    let mut f = Fixture::new(
        CREDENTIAL_LEDGER,
        Some(
            "version = 1\n\n[secrets]\nmode = \"strict\"\nenabled_by = \"operator-c\"\nenabled_at = 1794000000002\n",
        ),
    );
    let report = f.index().unwrap();
    assert!(!report.ok && report.sensitive_warnings.is_empty());
}

#[test]
fn project_policy_and_warning_provenance_never_leak_between_calls() {
    set_secrets_mode(SecretsMode::Strict, None);
    let mut first = Fixture::new(
        CREDENTIAL_LEDGER,
        Some("version=1\n[secrets]\nmode='warn'\nenabled_by='first'\nenabled_at=1\n"),
    );
    let first_report = first.index().unwrap();
    assert!(first_report.ok);
    assert_eq!(secrets_mode(), SecretsMode::Strict);
    assert!(take_sensitive_warnings().is_empty());
    let mut invalid = Fixture::new(CREDENTIAL_LEDGER, Some("version=2\n"));
    let refused = invalid.index().unwrap();
    assert!(!refused.ok);
    assert!(refused.sensitive_warnings.is_empty());
    let mut second = Fixture::new(
        CREDENTIAL_LEDGER,
        Some("version=1\n[secrets]\nmode='warn'\nenabled_by='second'\nenabled_at=2\n"),
    );
    let second_report = second.index().unwrap();
    assert!(second_report.ok);
    let events = second
        .store
        .secrets_warned_events(second_report.project_id, 10)
        .unwrap();
    assert_eq!(events[0]["enabled_by"], "second");
    assert!(events[0]["findings"].as_array().unwrap().iter().all(|f| {
        f["locator"]
            .as_str()
            .unwrap()
            .contains(second.root.file_name().unwrap().to_str().unwrap())
    }));
    assert_eq!(secrets_mode(), SecretsMode::Strict);
    assert!(take_sensitive_warnings().is_empty());
}

#[test]
fn warn_redacts_projected_fields_and_preserves_original_source_bytes() {
    let ledger = CREDENTIAL_LEDGER.replace(
        "  next_action: Start",
        "  summary: 'api_key: sk-ant-abcdefghijklmnopqrstuvwxyz012345'\n  next_action: Start",
    );
    let mut f = Fixture::new(
        &ledger,
        Some("version=1\n[secrets]\nmode='warn'\nenabled_by='reviewer'\nenabled_at=1\n"),
    );
    let report = f.index().unwrap();
    assert!(report.ok, "{:?}", report.issues);
    let rows = f.store.work_items(report.project_id).unwrap();
    let json = serde_json::to_string(&rows).unwrap();
    assert!(!json.contains("sk-ant"));
    assert!(json.contains(SENSITIVE_CONTENT_WITHHELD));
    assert_eq!(
        fs::read_to_string(f.root.join("ledger.yaml")).unwrap(),
        ledger
    );
}

#[test]
fn warn_mode_does_not_admit_raw_source_bytes_to_mutation_archives() {
    let mut f = Fixture::new(
        CREDENTIAL_LEDGER,
        Some("version=1\n[secrets]\nmode='warn'\nenabled_by='reviewer'\nenabled_at=1\n"),
    );
    let report = f.index().unwrap();
    assert!(report.ok);
    let source = f.store.sources(report.project_id).unwrap().remove(0);
    let _scope = awr_source::SecretsPolicy::load(&f.root).unwrap().enter();
    assert!(awr_source::inspect_registered_source(&f.root, &source).is_err());
    assert_eq!(
        fs::read_to_string(f.root.join("ledger.yaml")).unwrap(),
        CREDENTIAL_LEDGER
    );
    assert!(!f.root.join(".awr/mutations").exists());
}
