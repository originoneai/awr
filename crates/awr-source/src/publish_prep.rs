//! First-round Team publish preparation (AWR-TMCP-020).
//!
//! Maps a server-held YAML workstream ledger plus referenced Markdown/JSON
//! specs into a `workstreams.json` candidate. Preview identity, dependency,
//! acceptance and source diffs before ingest. Missing fields and unsupported
//! formats hard-reject. Source status / historical `done` stay source notes
//! only and never become completion receipts. Role or membership fields are
//! never invented from the ledger.
use crate::locator::fingerprint;
use awr_core::{
    Error, Id, Result, WORKSTREAM_CATALOG_VERSION, Workstream, WorkstreamCatalog, WorkstreamState,
};
use awr_team::{
    DependencyAcceptanceMode, WorkContract, WorkId, WorkstreamBundle, WorkstreamContract,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

/// First-round supported ledger adapter id for Team publish preparation.
pub const SUPPORTED_LEDGER_ADAPTER: &str = "yaml-workstream-ledger-v1";

/// Mapping-default completion policy when the ledger omits one. This is a
/// contract field default, not a role or membership grant.
pub const DEFAULT_COMPLETION_POLICY: &str = "independent_review";

pub const SOURCE_BINDING_FILE: &str = "source_binding.json";
pub const WORKSTREAMS_FILE: &str = "workstreams.json";
pub const SOURCE_PROVENANCE_FILE: &str = "source_provenance.json";
pub const PARSER_VERSION: &str = "awr-team-workstreams/1";
pub const PARSER_VERSION_V2: &str = "awr-team-workstreams/2";
pub const PARSER_VERSION_V3: &str = "awr-team-workstreams/3";
pub const PARSER_VERSION_V4: &str = "awr-team-workstreams/4";
pub const PARSER_VERSION_V5: &str = "awr-team-workstreams/5";
pub const PARSER_VERSION_V6: &str = "awr-team-workstreams/6";

const SUPPORTED_SPEC_EXTENSIONS: &[&str] = &["json", "md", "markdown"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum SoleSourceKind {
    ServerDirectory,
    PrivateManagementRepo,
}

/// Team project's sole authoritative source location. Developers do not need
/// author-laptop paths or ledger directory write access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoleSourceLocation {
    pub kind: SoleSourceKind,
    /// Absolute server directory, or private management-repo URL with pinned rev.
    pub locator: String,
    /// Subdirectory inside the bound location that holds the ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_relative_path: Option<String>,
}

impl SoleSourceLocation {
    pub fn server_directory(root: impl AsRef<Path>, ledger_relative_path: &str) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(Error::InvalidInput(
                "sole source server directory must be an absolute path".into(),
            ));
        }
        validate_relative_path(ledger_relative_path, "ledger")?;
        Ok(Self {
            kind: SoleSourceKind::ServerDirectory,
            locator: root.to_string_lossy().into_owned(),
            ledger_relative_path: Some(ledger_relative_path.into()),
        })
    }

    pub fn private_management_repo(
        locator: impl Into<String>,
        ledger_relative_path: &str,
    ) -> Result<Self> {
        let locator = locator.into();
        if locator.trim().is_empty() || locator.contains('\0') {
            return Err(Error::InvalidInput(
                "private management repo locator required".into(),
            ));
        }
        if !(locator.starts_with("git://")
            || locator.starts_with("https://")
            || locator.starts_with("ssh://"))
        {
            return Err(Error::InvalidInput(
                "private management repo locator must be git://, https://, or ssh://".into(),
            ));
        }
        validate_relative_path(ledger_relative_path, "ledger")?;
        Ok(Self {
            kind: SoleSourceKind::PrivateManagementRepo,
            locator,
            ledger_relative_path: Some(ledger_relative_path.into()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceStatusNote {
    pub work_external_key: String,
    pub raw_status: String,
    /// Source meaning only. Never a PG completion receipt.
    pub meaning: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FieldDiff {
    pub external_key: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PublishPreview {
    pub identity_added: Vec<String>,
    pub identity_removed: Vec<String>,
    pub identity_unchanged: Vec<String>,
    pub dependency_diffs: Vec<FieldDiff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_acceptance_diffs: Vec<FieldDiff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub execution_settlement_diffs: Vec<FieldDiff>,
    /// Explicit simulation policy changes only; older previews keep their wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completion_policy_diffs: Vec<FieldDiff>,
    pub acceptance_diffs: Vec<FieldDiff>,
    pub source_diffs: Vec<FieldDiff>,
    pub workstream_identity_added: Vec<String>,
    pub workstream_identity_removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferencedSpec {
    pub path: String,
    pub digest: String,
    pub bytes: usize,
    /// Exact bytes observed through the root-confined open used for validation.
    #[serde(skip)]
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishPackageFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Immutable original-source provenance persisted beside the generated candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenance {
    pub source_version_digest: String,
    pub ledger_relative_path: String,
    pub source_status_notes: Vec<SourceStatusNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPublishPackage {
    pub project_id: String,
    pub source_location: SoleSourceLocation,
    pub source_version_digest: String,
    pub ledger_identity_digest: String,
    pub bundle_digest: String,
    pub graph_digest: String,
    pub preview: PublishPreview,
    pub source_status_notes: Vec<SourceStatusNote>,
    pub referenced_specs: Vec<ReferencedSpec>,
    pub files: Vec<PublishPackageFile>,
    pub parser_version: String,
}

impl TeamPublishPackage {
    pub fn workstreams_bytes(&self) -> Result<&[u8]> {
        self.files
            .iter()
            .find(|f| f.path == WORKSTREAMS_FILE)
            .map(|f| f.bytes.as_slice())
            .ok_or_else(|| Error::InvalidInput("publish package missing workstreams.json".into()))
    }

    pub fn bundle(&self) -> Result<WorkstreamBundle> {
        serde_json::from_slice(self.workstreams_bytes()?)
            .map_err(|e| Error::InvalidInput(format!("invalid workstreams.json candidate: {e}")))
    }
}

#[derive(Debug, Clone, Default)]
pub struct PublishPrepOptions {
    /// Optional previously activated bundle for preview diffs. First publish
    /// passes `None`.
    pub baseline: Option<WorkstreamBundle>,
    /// Override mapping-default completion policy. Empty rejects.
    pub completion_policy: Option<String>,
}

/// Prepare a Team publish candidate from a bound server directory.
pub fn prepare_publish_from_server_directory(
    root: &Path,
    ledger_relative_path: &str,
    project_id: &str,
    options: &PublishPrepOptions,
) -> Result<TeamPublishPackage> {
    let location = SoleSourceLocation::server_directory(root, ledger_relative_path)?;
    let bytes = crate::read_under_root(root, ledger_relative_path).map_err(|e| {
        Error::InvalidInput(format!(
            "cannot read sole-source ledger {ledger_relative_path} under bound root: {e}"
        ))
    })?;
    prepare_publish_from_ledger_bytes(&location, root, &bytes, project_id, options)
}

/// Prepare a candidate from already-loaded ledger bytes under a bound root
/// (server directory or materialized private management repo checkout).
pub fn prepare_publish_from_ledger_bytes(
    location: &SoleSourceLocation,
    content_root: &Path,
    ledger_bytes: &[u8],
    project_id: &str,
    options: &PublishPrepOptions,
) -> Result<TeamPublishPackage> {
    if project_id.trim().is_empty() || project_id.len() > 128 {
        return Err(Error::InvalidInput(
            "project_id required for publish prep".into(),
        ));
    }
    let relative = location
        .ledger_relative_path
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("ledger_relative_path required".into()))?;
    validate_relative_path(relative, "ledger")?;
    if !relative.ends_with(".yaml") && !relative.ends_with(".yml") {
        return Err(Error::InvalidInput(format!(
            "unsupported ledger format for first-round Team publish: {relative}; only YAML workstream ledgers are supported"
        )));
    }

    // Preserve the YAML mapping's duplicate-key checks before converting to
    // JSON. Deserializing directly into JSON Value silently overwrites keys,
    // hiding ambiguous policies from subsequent strict contract validation.
    let document = serde_yaml_ng::from_slice::<serde_yaml_ng::Value>(ledger_bytes)
        .and_then(Value::deserialize)
        .map_err(|e| {
            Error::InvalidInput(format!(
                "unsupported or invalid YAML ledger for Team publish: {e}"
            ))
        })?;
    reject_role_invention(&document)?;

    let (catalog, key_to_id) = map_catalog(&document, project_id)?;
    let (contracts, status_notes) = map_contracts(&document, &key_to_id, options)?;
    let bundle = WorkstreamBundle {
        codec: if contracts
            .iter()
            .any(|entry| entry.contract.codec == WorkContract::CODEC_V6)
        {
            WorkstreamBundle::CODEC_V6
        } else if contracts
            .iter()
            .any(|entry| entry.contract.codec == WorkContract::CODEC_V5)
        {
            WorkstreamBundle::CODEC_V5
        } else if contracts
            .iter()
            .any(|entry| entry.contract.codec == WorkContract::CODEC_V4)
        {
            WorkstreamBundle::CODEC_V4
        } else if contracts
            .iter()
            .any(|entry| entry.contract.codec == WorkContract::CODEC_V3)
        {
            WorkstreamBundle::CODEC_V3
        } else if contracts
            .iter()
            .any(|entry| entry.contract.codec == WorkContract::CODEC_V2)
        {
            WorkstreamBundle::CODEC_V2
        } else {
            WorkstreamBundle::CODEC
        }
        .into(),
        catalog,
        contracts,
    };
    bundle
        .validate(project_id)
        .map_err(|e| Error::InvalidInput(e.to_string()))?;

    let referenced = load_referenced_specs(content_root, &bundle)?;
    let preview = preview_against_baseline(&bundle, options.baseline.as_ref(), location);

    let workstreams_bytes =
        serde_json::to_vec_pretty(&bundle).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let binding_bytes =
        serde_json::to_vec_pretty(location).map_err(|e| Error::InvalidInput(e.to_string()))?;

    let source_version_digest = fingerprint(ledger_bytes);
    let provenance = SourceProvenance {
        source_version_digest: source_version_digest.clone(),
        ledger_relative_path: relative.into(),
        source_status_notes: status_notes.clone(),
    };
    let provenance_bytes =
        serde_json::to_vec_pretty(&provenance).map_err(|e| Error::InvalidInput(e.to_string()))?;

    let mut files = vec![
        PublishPackageFile {
            path: WORKSTREAMS_FILE.into(),
            bytes: workstreams_bytes,
        },
        PublishPackageFile {
            path: SOURCE_BINDING_FILE.into(),
            bytes: binding_bytes,
        },
        PublishPackageFile {
            path: SOURCE_PROVENANCE_FILE.into(),
            bytes: provenance_bytes,
        },
        PublishPackageFile {
            // Persist the exact original ledger bytes so distinct source revisions
            // remain distinguishable after ingest (TMCP-020).
            path: relative.into(),
            bytes: ledger_bytes.to_vec(),
        },
    ];
    for spec in &referenced {
        files.push(PublishPackageFile {
            path: spec.path.clone(),
            bytes: spec.content.clone(),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(TeamPublishPackage {
        project_id: project_id.into(),
        source_location: location.clone(),
        source_version_digest,
        ledger_identity_digest: identity_digest(&bundle)?,
        bundle_digest: bundle
            .hash()
            .map_err(|e| Error::InvalidInput(e.to_string()))?,
        graph_digest: graph_digest(&bundle)?,
        preview,
        source_status_notes: status_notes,
        referenced_specs: referenced,
        files,
        parser_version: match bundle.codec.as_str() {
            WorkstreamBundle::CODEC_V6 => PARSER_VERSION_V6,
            WorkstreamBundle::CODEC_V5 => PARSER_VERSION_V5,
            WorkstreamBundle::CODEC_V4 => PARSER_VERSION_V4,
            WorkstreamBundle::CODEC_V3 => PARSER_VERSION_V3,
            WorkstreamBundle::CODEC_V2 => PARSER_VERSION_V2,
            _ => PARSER_VERSION,
        }
        .into(),
    })
}

fn validate_relative_path(path: &str, label: &str) -> Result<()> {
    if path.is_empty() || path.starts_with('/') || path.contains('\0') {
        return Err(Error::InvalidInput(format!(
            "{label} path must be a non-empty relative path"
        )));
    }
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::InvalidInput(format!(
            "{label} path must stay within the bound source location"
        )));
    }
    Ok(())
}

fn reject_role_invention(document: &Value) -> Result<()> {
    for key in ["members", "memberships", "grants", "permissions", "roles"] {
        if document.get(key).is_some() {
            return Err(Error::InvalidInput(format!(
                "ledger field `{key}` cannot be mapped into Team publish; do not invent role relationships from source"
            )));
        }
    }
    Ok(())
}

fn map_catalog(
    document: &Value,
    project_id: &str,
) -> Result<(WorkstreamCatalog, BTreeMap<String, Id>)> {
    let workstreams = document.get("workstreams").ok_or_else(|| {
        Error::InvalidInput(
            "YAML ledger missing workstreams; first-round Team publish requires yaml-workstream-ledger-v1".into(),
        )
    })?;
    let version = workstreams
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::InvalidInput("workstreams.version required".into()))?;
    if version != u64::from(WORKSTREAM_CATALOG_VERSION) {
        return Err(Error::InvalidInput(format!(
            "unsupported workstreams.version {version}"
        )));
    }
    let definitions = workstreams
        .get("definitions")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidInput("workstreams.definitions required".into()))?;
    if definitions.is_empty() {
        return Err(Error::InvalidInput(
            "workstreams.definitions must not be empty".into(),
        ));
    }
    let legacy_default = match workstreams.get("legacy_default") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) => Some(parse_id(raw, "workstreams.legacy_default")?),
        Some(_) => {
            return Err(Error::InvalidInput(
                "workstreams.legacy_default must be a ULID string".into(),
            ));
        }
    };

    let mut streams = Vec::new();
    let mut key_to_id = BTreeMap::new();
    for (index, def) in definitions.iter().enumerate() {
        let pointer = format!("/workstreams/definitions/{index}");
        let id_raw = required_string(def, "id", &pointer)?;
        let external_key = required_string(def, "external_key", &pointer)?;
        let title = required_string(def, "title", &pointer)?;
        let state_raw = required_string(def, "state", &pointer)?;
        let state: WorkstreamState = serde_json::from_value(Value::String(state_raw.clone()))
            .map_err(|_| {
                Error::InvalidInput(format!("{pointer}/state: unsupported state {state_raw}"))
            })?;
        let authority_version = def
            .get("authority_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::InvalidInput(format!("{pointer}/authority_version required")))?;
        if authority_version == 0 {
            return Err(Error::InvalidInput(format!(
                "{pointer}/authority_version must be > 0"
            )));
        }
        let goal_keys = string_list(def, "goal_keys", &pointer)?;
        let acceptance_contracts = string_list(def, "acceptance_contracts", &pointer)?;
        let id = parse_id(&id_raw, &format!("{pointer}/id"))?;
        if key_to_id.insert(external_key.clone(), id).is_some() {
            return Err(Error::InvalidInput(format!(
                "duplicate workstream external_key {external_key}"
            )));
        }
        streams.push(Workstream {
            id,
            project_id: project_id.into(),
            external_key,
            title,
            state,
            authority_version,
            goal_keys,
            acceptance_contracts,
        });
    }
    let catalog = WorkstreamCatalog {
        version: WORKSTREAM_CATALOG_VERSION,
        project_id: project_id.into(),
        legacy_default,
        workstreams: streams,
    };
    catalog
        .validate()
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    Ok((catalog, key_to_id))
}

fn map_contracts(
    document: &Value,
    key_to_id: &BTreeMap<String, Id>,
    options: &PublishPrepOptions,
) -> Result<(Vec<WorkstreamContract>, Vec<SourceStatusNote>)> {
    let items = document
        .get("work_items")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidInput("work_items required for Team publish".into()))?;
    if items.is_empty() {
        return Err(Error::InvalidInput("work_items must not be empty".into()));
    }
    let completion_policy = options
        .completion_policy
        .clone()
        .unwrap_or_else(|| DEFAULT_COMPLETION_POLICY.into());
    if completion_policy.trim().is_empty() {
        return Err(Error::InvalidInput("completion_policy required".into()));
    }

    let mut contracts = Vec::new();
    let mut notes = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/work_items/{index}");
        let external_key = required_string(item, "id", &pointer)?;
        if !seen.insert(external_key.clone()) {
            return Err(Error::InvalidInput(format!(
                "duplicate work id {external_key}"
            )));
        }
        let _title = required_string(item, "title", &pointer)?;
        let workstream_key = required_string(item, "workstream", &pointer)?;
        let workstream_id = *key_to_id.get(&workstream_key).ok_or_else(|| {
            Error::InvalidInput(format!(
                "{pointer}/workstream: unknown workstream key {workstream_key}"
            ))
        })?;
        let acceptance = string_list(item, "acceptance", &pointer)?;
        if acceptance.is_empty() {
            return Err(Error::InvalidInput(format!(
                "{pointer}/acceptance required for Team contract candidates"
            )));
        }
        let goals = optional_string_list(item, &["goals", "goal"], &pointer)?;
        let scope_paths = optional_string_list(item, &["paths", "deliverables"], &pointer)?;
        let required_dependencies =
            optional_string_list(item, &["depends_on", "dependencies"], &pointer)?;
        let hard_rules = optional_string_list(item, &["hard_rules"], &pointer)?;
        let verification_requirements =
            optional_string_list(item, &["verification_requirements"], &pointer)?;
        let dependency_acceptance: BTreeMap<String, DependencyAcceptanceMode> = item
            .get("dependency_acceptance")
            .map(|raw| serde_json::from_value(raw.clone()))
            .transpose()
            .map_err(|_| {
                Error::InvalidInput(format!(
                    "{pointer}/dependency_acceptance: invalid policy map"
                ))
            })?
            .unwrap_or_default();
        let execution_settlement = item
            .get("execution_settlement")
            .map(|raw| serde_json::from_value(raw.clone()))
            .transpose()
            .map_err(|_| {
                Error::InvalidInput(format!(
                    "{pointer}/execution_settlement: invalid explicit settlement policy"
                ))
            })?;
        if execution_settlement.is_some() && item.get("completion_policy").is_some() {
            // A mapping default cannot conceal a malformed settlement policy.
            required_string(item, "completion_policy", &pointer)?;
        }
        let item_policy = item
            .get("completion_policy")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| completion_policy.clone());

        if let Some(raw_status) = item.get("status").and_then(Value::as_str) {
            notes.push(SourceStatusNote {
                work_external_key: external_key.clone(),
                raw_status: raw_status.into(),
                meaning: "source_only".into(),
            });
        }

        let work_id = WorkId::new(&external_key)
            .map_err(|e| Error::InvalidInput(format!("{pointer}/id: {e}")))?;
        let contract = WorkContract {
            codec: if dependency_acceptance
                .values()
                .any(|mode| matches!(mode, DependencyAcceptanceMode::CrossWorkstream(_)))
            {
                WorkContract::CODEC_V6
            } else if dependency_acceptance
                .values()
                .any(|mode| *mode == DependencyAcceptanceMode::SimulatedMemberIndependent)
            {
                WorkContract::CODEC_V5
            } else if item_policy
                == awr_team::ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY
            {
                WorkContract::CODEC_V4
            } else if execution_settlement.is_some() {
                WorkContract::CODEC_V3
            } else if item.get("dependency_acceptance").is_some() {
                WorkContract::CODEC_V2
            } else {
                WorkContract::CODEC
            }
            .into(),
            work_id,
            external_key: external_key.clone(),
            goals,
            hard_rules,
            scope_paths,
            acceptance,
            required_dependencies,
            completion_policy: item_policy,
            verification_requirements,
            dependency_acceptance,
            execution_settlement,
        };
        contract
            .validate()
            .map_err(|e| Error::InvalidInput(format!("{pointer}: {e}")))?;
        contracts.push(WorkstreamContract {
            workstream_id,
            contract,
        });
    }
    Ok((contracts, notes))
}

fn load_referenced_specs(root: &Path, bundle: &WorkstreamBundle) -> Result<Vec<ReferencedSpec>> {
    let mut paths = BTreeSet::new();
    for stream in &bundle.catalog.workstreams {
        for path in &stream.acceptance_contracts {
            paths.insert(path.clone());
        }
    }
    let mut specs = Vec::new();
    for path in paths {
        validate_relative_path(&path, "referenced spec")?;
        let ext = Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !SUPPORTED_SPEC_EXTENSIONS.contains(&ext.as_str()) {
            return Err(Error::InvalidInput(format!(
                "unsupported referenced spec format `{path}`; first-round supports Markdown/JSON only"
            )));
        }
        let bytes = crate::read_under_root(root, &path).map_err(|e| {
            Error::InvalidInput(format!("missing or unsafe referenced spec `{path}`: {e}"))
        })?;
        if ext == "json" {
            let _: Value = serde_json::from_slice(&bytes).map_err(|e| {
                Error::InvalidInput(format!("referenced JSON spec `{path}` is invalid: {e}"))
            })?;
        }
        specs.push(ReferencedSpec {
            path,
            digest: fingerprint(&bytes),
            bytes: bytes.len(),
            content: bytes,
        });
    }
    Ok(specs)
}

fn preview_against_baseline(
    candidate: &WorkstreamBundle,
    baseline: Option<&WorkstreamBundle>,
    location: &SoleSourceLocation,
) -> PublishPreview {
    let mut preview = PublishPreview::default();
    for entry in &candidate.contracts {
        let after = &entry.contract;
        let prior_contract = baseline.and_then(|bundle| {
            bundle
                .contracts
                .iter()
                .find(|prior| prior.contract.external_key == after.external_key)
                .map(|entry| &entry.contract)
        });
        if (matches!(
            after.codec.as_str(),
            WorkContract::CODEC_V4 | WorkContract::CODEC_V5 | WorkContract::CODEC_V6
        ) || prior_contract.is_some_and(|prior| {
            matches!(
                prior.codec.as_str(),
                WorkContract::CODEC_V4 | WorkContract::CODEC_V5 | WorkContract::CODEC_V6
            )
        })) && prior_contract.map(|prior| &prior.completion_policy)
            != Some(&after.completion_policy)
        {
            preview.completion_policy_diffs.push(FieldDiff {
                external_key: after.external_key.clone(),
                before: prior_contract.map(|prior| serde_json::json!(prior.completion_policy)),
                after: Some(serde_json::json!(after.completion_policy)),
            });
        }
        let before = baseline
            .and_then(|bundle| {
                bundle
                    .contracts
                    .iter()
                    .find(|prior| prior.contract.external_key == after.external_key)
            })
            .map(|entry| &entry.contract.dependency_acceptance);
        if before != Some(&after.dependency_acceptance)
            && (before.is_some_and(|m| !m.is_empty()) || !after.dependency_acceptance.is_empty())
        {
            preview.dependency_acceptance_diffs.push(FieldDiff {
                external_key: after.external_key.clone(),
                before: before.map(|m| serde_json::json!(m)),
                after: Some(serde_json::json!(after.dependency_acceptance)),
            });
        }
        let before = baseline
            .and_then(|bundle| {
                bundle
                    .contracts
                    .iter()
                    .find(|prior| prior.contract.external_key == after.external_key)
            })
            .and_then(|entry| entry.contract.execution_settlement.as_ref());
        if before != after.execution_settlement.as_ref() {
            preview.execution_settlement_diffs.push(FieldDiff {
                external_key: after.external_key.clone(),
                before: before.map(|policy| serde_json::json!(policy)),
                after: after
                    .execution_settlement
                    .as_ref()
                    .map(|policy| serde_json::json!(policy)),
            });
        }
    }
    let after_keys: BTreeSet<_> = candidate
        .contracts
        .iter()
        .map(|c| c.contract.external_key.clone())
        .collect();
    let after_streams: BTreeSet<_> = candidate
        .catalog
        .workstreams
        .iter()
        .map(|s| s.external_key.clone())
        .collect();
    let Some(baseline) = baseline else {
        preview.identity_added = after_keys.into_iter().collect();
        preview.workstream_identity_added = after_streams.into_iter().collect();
        preview.source_diffs.push(FieldDiff {
            external_key: "*".into(),
            before: None,
            after: Some(serde_json::json!({
                "kind": location.kind,
                "locator": location.locator,
                "ledger_relative_path": location.ledger_relative_path,
            })),
        });
        return preview;
    };

    let before_map: BTreeMap<_, _> = baseline
        .contracts
        .iter()
        .map(|c| (c.contract.external_key.clone(), &c.contract))
        .collect();
    let after_map: BTreeMap<_, _> = candidate
        .contracts
        .iter()
        .map(|c| (c.contract.external_key.clone(), &c.contract))
        .collect();
    for key in after_map.keys() {
        if before_map.contains_key(key) {
            preview.identity_unchanged.push(key.clone());
        } else {
            preview.identity_added.push(key.clone());
        }
    }
    for key in before_map.keys() {
        if !after_map.contains_key(key) {
            preview.identity_removed.push(key.clone());
        }
    }
    for (key, after) in &after_map {
        let Some(before) = before_map.get(key) else {
            continue;
        };
        if before.required_dependencies != after.required_dependencies {
            preview.dependency_diffs.push(FieldDiff {
                external_key: key.clone(),
                before: Some(serde_json::json!(before.required_dependencies)),
                after: Some(serde_json::json!(after.required_dependencies)),
            });
        }
        if before.acceptance != after.acceptance {
            preview.acceptance_diffs.push(FieldDiff {
                external_key: key.clone(),
                before: Some(serde_json::json!(before.acceptance)),
                after: Some(serde_json::json!(after.acceptance)),
            });
        }
    }
    let before_streams: BTreeSet<_> = baseline
        .catalog
        .workstreams
        .iter()
        .map(|s| s.external_key.clone())
        .collect();
    for key in &after_streams {
        if !before_streams.contains(key) {
            preview.workstream_identity_added.push(key.clone());
        }
    }
    for key in &before_streams {
        if !after_streams.contains(key) {
            preview.workstream_identity_removed.push(key.clone());
        }
    }
    preview.source_diffs.push(FieldDiff {
        external_key: "*".into(),
        before: Some(serde_json::json!({"baseline_codec": baseline.codec})),
        after: Some(serde_json::json!({
            "kind": location.kind,
            "locator": location.locator,
            "ledger_relative_path": location.ledger_relative_path,
            "candidate_codec": candidate.codec,
        })),
    });
    preview
}

fn identity_digest(bundle: &WorkstreamBundle) -> Result<String> {
    let mut identities = bundle
        .contracts
        .iter()
        .map(|c| {
            (
                c.contract.work_id.as_str().to_owned(),
                c.contract.external_key.clone(),
                c.workstream_id.to_string(),
            )
        })
        .collect::<Vec<_>>();
    identities.sort();
    let bytes = serde_json::to_vec(&identities).map_err(|e| Error::InvalidInput(e.to_string()))?;
    Ok(fingerprint(&bytes))
}

fn graph_digest(bundle: &WorkstreamBundle) -> Result<String> {
    let mut edges = Vec::new();
    for entry in &bundle.contracts {
        for dep in &entry.contract.required_dependencies {
            edges.push((entry.contract.work_id.as_str().to_owned(), dep.clone()));
        }
    }
    edges.sort();
    let bytes = serde_json::to_vec(&edges).map_err(|e| Error::InvalidInput(e.to_string()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn parse_id(raw: &str, pointer: &str) -> Result<Id> {
    raw.parse::<Id>()
        .map_err(|_| Error::InvalidInput(format!("{pointer}: invalid ULID `{raw}`")))
}

fn required_string(value: &Value, field: &str, pointer: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::InvalidInput(format!("{pointer}/{field} required")))
}

fn string_list(value: &Value, field: &str, pointer: &str) -> Result<Vec<String>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                item.as_str()
                    .map(str::to_owned)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        Error::InvalidInput(format!("{pointer}/{field}/{i} must be a string"))
                    })
            })
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(vec![s.clone()]),
        Some(_) => Err(Error::InvalidInput(format!(
            "{pointer}/{field} must be a string list"
        ))),
    }
}

fn optional_string_list(value: &Value, fields: &[&str], pointer: &str) -> Result<Vec<String>> {
    for field in fields {
        if value.get(*field).is_some() {
            return string_list(value, field, pointer);
        }
    }
    Ok(Vec::new())
}

/// Source status notes never authorize completion receipt writes.
pub fn source_status_notes_are_completion_receipts() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/team-mcp/publish-prep")
    }

    #[test]
    fn happy_path_yaml_maps_to_workstreams_candidate_with_preview() {
        let root = fixture_root();
        let baseline: WorkstreamBundle = serde_json::from_str(
            &fs::read_to_string(root.join("baseline-workstreams.json")).unwrap(),
        )
        .unwrap();
        let package = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo-project",
            &PublishPrepOptions {
                baseline: Some(baseline),
                completion_policy: None,
            },
        )
        .unwrap();
        let bundle = package.bundle().unwrap();
        assert_eq!(bundle.codec, WorkstreamBundle::CODEC);
        assert_eq!(bundle.contracts.len(), 2);
        assert_eq!(bundle.contracts[0].contract.external_key, "API-1");
        assert_eq!(
            bundle.contracts[1].contract.required_dependencies,
            vec!["API-1".to_string()]
        );
        assert!(
            package
                .preview
                .acceptance_diffs
                .iter()
                .any(|d| d.external_key == "API-1")
        );
        assert!(
            package
                .source_status_notes
                .iter()
                .any(|n| n.work_external_key == "API-1"
                    && n.raw_status == "completed"
                    && n.meaning == "source_only")
        );
        assert!(!source_status_notes_are_completion_receipts());
        assert!(
            package
                .files
                .iter()
                .any(|f| f.path == "contracts/api-v1.json")
        );
        assert!(
            package
                .files
                .iter()
                .any(|f| f.path == "contracts/client-v1.md")
        );
        assert_eq!(
            package.source_location.kind,
            SoleSourceKind::ServerDirectory
        );
        assert!(!package.bundle_digest.is_empty());
        assert!(!package.graph_digest.is_empty());
    }

    #[test]
    fn explicit_source_policy_versions_only_selected_contract_and_previews_changes() {
        let root = fixture_root();
        let original = fs::read(root.join("ledger.yaml")).unwrap();
        let baseline = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap();
        assert_eq!(baseline.parser_version, PARSER_VERSION);
        assert!(baseline.preview.dependency_acceptance_diffs.is_empty());
        let mut doc: Value = serde_yaml_ng::from_slice(&original).unwrap();
        doc["work_items"][1]["workstream"] = json!("api");
        doc["work_items"][1]["dependency_acceptance"] =
            json!({"API-1":"agent_reviewed_caller_asserted_reconciled"});
        let prepare = |doc: &Value, before: Option<WorkstreamBundle>| {
            prepare_publish_from_ledger_bytes(
                &baseline.source_location,
                &root,
                serde_yaml_ng::to_string(doc).unwrap().as_bytes(),
                "demo-project",
                &PublishPrepOptions {
                    baseline: before,
                    completion_policy: None,
                },
            )
        };
        let candidate = prepare(&doc, Some(baseline.bundle().unwrap())).unwrap();
        let bundle = candidate.bundle().unwrap();
        assert_eq!(candidate.parser_version, PARSER_VERSION_V2);
        assert_eq!(bundle.codec, WorkstreamBundle::CODEC_V2);
        assert_eq!(
            bundle.contracts[0].contract,
            baseline.bundle().unwrap().contracts[0].contract
        );
        assert_eq!(bundle.contracts[1].contract.codec, WorkContract::CODEC_V2);
        let diffs = &candidate.preview.dependency_acceptance_diffs;
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].before, Some(json!({})));
        assert_eq!(
            diffs[0].after,
            Some(doc["work_items"][1]["dependency_acceptance"].clone())
        );
        assert!(
            prepare(&doc, Some(bundle.clone()))
                .unwrap()
                .preview
                .dependency_acceptance_diffs
                .is_empty()
        );
        assert_eq!(
            prepare(&doc, None)
                .unwrap()
                .preview
                .dependency_acceptance_diffs[0]
                .before,
            None
        );
        let mut removed = doc.clone();
        removed["work_items"][1]
            .as_object_mut()
            .unwrap()
            .remove("dependency_acceptance");
        let tightened = prepare(&removed, Some(bundle)).unwrap();
        assert_eq!(tightened.parser_version, PARSER_VERSION);
        assert_eq!(
            tightened.preview.dependency_acceptance_diffs[0].after,
            Some(json!({}))
        );
        doc["work_items"][1]["workstream"] = json!("client");
        assert!(
            prepare(&doc, None)
                .unwrap_err()
                .to_string()
                .contains("same workstream")
        );
    }

    #[test]
    fn simulated_dependency_source_selects_v5_and_previews_exact_edges() {
        let root = fixture_root();
        let original = fs::read(root.join("ledger.yaml")).unwrap();
        let baseline = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap();
        let old_bundle = baseline.bundle().unwrap();
        let mut doc: Value = serde_yaml_ng::from_slice(&original).unwrap();
        doc["work_items"][1]["workstream"] = json!("api");
        doc["work_items"][1]["dependency_acceptance"] =
            json!({"API-1":"simulated_member_independent"});
        let prepare = |doc: &Value, before: Option<WorkstreamBundle>| {
            prepare_publish_from_ledger_bytes(
                &baseline.source_location,
                &root,
                serde_yaml_ng::to_string(doc).unwrap().as_bytes(),
                "demo-project",
                &PublishPrepOptions {
                    baseline: before,
                    completion_policy: None,
                },
            )
        };
        let candidate = prepare(&doc, Some(old_bundle.clone())).unwrap();
        let bundle = candidate.bundle().unwrap();
        assert_eq!(candidate.parser_version, PARSER_VERSION_V5);
        assert_eq!(bundle.codec, WorkstreamBundle::CODEC_V5);
        assert_eq!(bundle.contracts[1].contract.codec, WorkContract::CODEC_V5);
        assert_eq!(
            bundle.contracts[1].contract.completion_policy,
            "independent_review"
        );
        assert!(bundle.contracts[1].contract.execution_settlement.is_none());
        assert_eq!(
            bundle.contracts[0].contract,
            old_bundle.contracts[0].contract
        );
        assert_eq!(
            bundle.contracts[0].contract.hash().unwrap(),
            old_bundle.contracts[0].contract.hash().unwrap()
        );
        assert_eq!(
            candidate.preview.dependency_acceptance_diffs[0].after,
            Some(json!({"API-1":"simulated_member_independent"}))
        );
        assert!(
            prepare(&doc, Some(bundle.clone()))
                .unwrap()
                .preview
                .dependency_acceptance_diffs
                .is_empty()
        );
        let mut removed = doc.clone();
        removed["work_items"][1]
            .as_object_mut()
            .unwrap()
            .remove("dependency_acceptance");
        let changed = prepare(&removed, Some(bundle)).unwrap();
        assert_eq!(
            changed.preview.dependency_acceptance_diffs[0].after,
            Some(json!({}))
        );
        for invalid in [
            Value::Null,
            json!({}),
            json!({"API-1":"unknown"}),
            json!({"absent":"simulated_member_independent"}),
            json!({"CLIENT-1":"simulated_member_independent"}),
        ] {
            let mut invalid_doc = doc.clone();
            invalid_doc["work_items"][1]["dependency_acceptance"] = invalid;
            assert!(prepare(&invalid_doc, None).is_err());
        }
        doc["work_items"][1]["workstream"] = json!("client");
        assert!(prepare(&doc, None).is_err());
        assert_eq!(fs::read(root.join("ledger.yaml")).unwrap(), original);
    }

    #[test]
    fn cross_stream_source_selects_v6_and_previews_both_policy_dimensions() {
        let root = fixture_root();
        let original = fs::read(root.join("ledger.yaml")).unwrap();
        let baseline = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap();
        let old = baseline.bundle().unwrap();
        let mut doc: Value = serde_yaml_ng::from_slice(&original).unwrap();
        let prepare = |doc: &Value, before: Option<WorkstreamBundle>| {
            prepare_publish_from_ledger_bytes(
                &baseline.source_location,
                &root,
                serde_yaml_ng::to_string(doc).unwrap().as_bytes(),
                "demo-project",
                &PublishPrepOptions {
                    baseline: before,
                    completion_policy: None,
                },
            )
        };
        for assurance in ["team_independent", "simulated_member_independent"] {
            for version in ["fixed_delivery", "current_contract"] {
                doc["work_items"][1]["dependency_acceptance"] = json!({"API-1":{
                    "cross_workstream":{"review_assurance":assurance,"version_policy":version}
                }});
                let candidate = prepare(&doc, Some(old.clone())).unwrap();
                let bundle = candidate.bundle().unwrap();
                assert_eq!(candidate.parser_version, PARSER_VERSION_V6);
                assert_eq!(bundle.codec, WorkstreamBundle::CODEC_V6);
                assert_eq!(bundle.contracts[1].contract.codec, WorkContract::CODEC_V6);
                assert_eq!(
                    bundle.contracts[0].contract.hash().unwrap(),
                    old.contracts[0].contract.hash().unwrap()
                );
                assert_eq!(
                    bundle.contracts[1].contract.completion_policy,
                    old.contracts[1].contract.completion_policy
                );
                assert_eq!(
                    candidate.preview.dependency_acceptance_diffs[0].after,
                    Some(doc["work_items"][1]["dependency_acceptance"].clone())
                );
                assert!(
                    prepare(&doc, Some(bundle))
                        .unwrap()
                        .preview
                        .dependency_acceptance_diffs
                        .is_empty()
                );
            }
        }
        let valid = doc.clone();
        doc["work_items"][1]["workstream"] = json!("api");
        assert!(prepare(&doc, None).is_err());
        for bad in [
            Value::Null,
            json!({}),
            json!({"API-1":{"cross_workstream":{"review_assurance":"unknown","version_policy":"fixed_delivery"}}}),
        ] {
            let mut invalid = valid.clone();
            invalid["work_items"][1]["dependency_acceptance"] = bad;
            assert!(prepare(&invalid, None).is_err());
        }
        let mut cycle = valid;
        cycle["work_items"][0]["depends_on"] = json!(["CLIENT-1"]);
        cycle["work_items"][0]["dependency_acceptance"] = json!({"CLIENT-1":{
            "cross_workstream":{"review_assurance":"team_independent","version_policy":"current_contract"}
        }});
        assert!(
            prepare(&cycle, None)
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
        assert_eq!(fs::read(root.join("ledger.yaml")).unwrap(), original);
    }

    #[test]
    fn independent_workspace_policy_is_explicit_versioned_and_previewed() {
        let root = fixture_root();
        let original = fs::read(root.join("ledger.yaml")).unwrap();
        let baseline = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap();
        assert!(
            json!(baseline.preview)
                .get("execution_settlement_diffs")
                .is_none()
        );
        let mut doc: Value = serde_yaml_ng::from_slice(&original).unwrap();
        doc["work_items"][0]["completion_policy"] =
            json!(awr_team::ExecutionSettlementPolicy::COMPLETION_POLICY);
        doc["work_items"][0]["verification_requirements"] =
            json!(["independently verify artifact bytes"]);
        doc["work_items"][0]["execution_settlement"] =
            json!({"mode":"independent_workspace_v1","workspace_id":"worker-a"});
        let prepare = |doc: &Value, before: Option<WorkstreamBundle>| {
            prepare_publish_from_ledger_bytes(
                &baseline.source_location,
                &root,
                serde_yaml_ng::to_string(doc).unwrap().as_bytes(),
                "demo-project",
                &PublishPrepOptions {
                    baseline: before,
                    completion_policy: None,
                },
            )
        };
        let candidate = prepare(&doc, Some(baseline.bundle().unwrap())).unwrap();
        let bundle = candidate.bundle().unwrap();
        assert_eq!(candidate.parser_version, PARSER_VERSION_V3);
        assert_eq!(bundle.codec, WorkstreamBundle::CODEC_V3);
        assert_eq!(bundle.contracts[0].contract.codec, WorkContract::CODEC_V3);
        assert_eq!(
            bundle.contracts[1].contract,
            baseline.bundle().unwrap().contracts[1].contract
        );
        assert_eq!(candidate.preview.execution_settlement_diffs.len(), 1);
        let diff = &candidate.preview.execution_settlement_diffs[0];
        assert_eq!(diff.external_key, "API-1");
        assert_eq!(diff.before, None);
        assert_eq!(
            diff.after,
            Some(doc["work_items"][0]["execution_settlement"].clone())
        );
        assert!(
            prepare(&doc, Some(bundle.clone()))
                .unwrap()
                .preview
                .execution_settlement_diffs
                .is_empty()
        );
        assert_eq!(
            prepare(&doc, None)
                .unwrap()
                .preview
                .execution_settlement_diffs[0]
                .after,
            diff.after
        );

        let mut changed = doc.clone();
        changed["work_items"][0]["execution_settlement"]["workspace_id"] = json!("worker-b");
        let moved = prepare(&changed, Some(bundle.clone())).unwrap();
        assert_ne!(moved.bundle_digest, candidate.bundle_digest);
        assert_eq!(
            moved.preview.execution_settlement_diffs[0].before,
            diff.after
        );
        assert_ne!(
            moved.preview.execution_settlement_diffs[0].after,
            diff.after
        );

        doc["work_items"][1]["workstream"] = json!("api");
        doc["work_items"][1]["dependency_acceptance"] =
            json!({"API-1":"agent_reviewed_caller_asserted_reconciled"});
        let mixed = prepare(&doc, None).unwrap();
        assert_eq!(
            mixed.bundle().unwrap().contracts[1].contract.codec,
            WorkContract::CODEC_V2
        );
        assert_eq!(mixed.parser_version, PARSER_VERSION_V3);

        let mut removed = changed.clone();
        removed["work_items"][0]
            .as_object_mut()
            .unwrap()
            .remove("execution_settlement");
        let tightened = prepare(&removed, Some(moved.bundle().unwrap())).unwrap();
        assert_eq!(tightened.parser_version, PARSER_VERSION);
        assert!(
            tightened.preview.execution_settlement_diffs[0]
                .before
                .is_some()
        );
        assert_eq!(tightened.preview.execution_settlement_diffs[0].after, None);
        assert_eq!(fs::read(root.join("ledger.yaml")).unwrap(), original);
    }

    #[test]
    fn simulated_member_source_policy_has_distinct_codec_and_reviewable_preview() {
        let root = fixture_root();
        let original = fs::read(root.join("ledger.yaml")).unwrap();
        let baseline = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap();
        let mut doc: Value = serde_yaml_ng::from_slice(&original).unwrap();
        doc["work_items"][0]["completion_policy"] =
            json!(awr_team::ExecutionSettlementPolicy::COMPLETION_POLICY);
        doc["work_items"][0]["verification_requirements"] = json!(["verify artifact bytes"]);
        doc["work_items"][0]["execution_settlement"] =
            json!({"mode":"independent_workspace_v1","workspace_id":"worker-a"});
        let prepare = |bytes: &[u8], before: Option<WorkstreamBundle>| {
            prepare_publish_from_ledger_bytes(
                &baseline.source_location,
                &root,
                bytes,
                "demo-project",
                &PublishPrepOptions {
                    baseline: before,
                    completion_policy: None,
                },
            )
        };
        let older = prepare(
            &serde_json::to_vec(&doc).unwrap(),
            Some(baseline.bundle().unwrap()),
        )
        .unwrap();
        assert_eq!(older.parser_version, PARSER_VERSION_V3);
        assert!(
            json!(older.preview)
                .get("completion_policy_diffs")
                .is_none()
        );
        doc["work_items"][0]["completion_policy"] =
            json!(awr_team::ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY);
        let candidate = prepare(
            &serde_json::to_vec(&doc).unwrap(),
            Some(older.bundle().unwrap()),
        )
        .unwrap();
        let bundle = candidate.bundle().unwrap();
        assert_eq!(candidate.parser_version, PARSER_VERSION_V4);
        assert_eq!(bundle.codec, WorkstreamBundle::CODEC_V4);
        assert_eq!(bundle.contracts[0].contract.codec, WorkContract::CODEC_V4);
        assert_eq!(
            bundle.contracts[1].contract.hash().unwrap(),
            baseline.bundle().unwrap().contracts[1]
                .contract
                .hash()
                .unwrap()
        );
        assert!(candidate.preview.execution_settlement_diffs.is_empty());
        assert_eq!(candidate.preview.completion_policy_diffs.len(), 1);
        let diff = &candidate.preview.completion_policy_diffs[0];
        assert_eq!(diff.external_key, "API-1");
        assert_eq!(
            diff.before,
            Some(json!(
                awr_team::ExecutionSettlementPolicy::COMPLETION_POLICY
            ))
        );
        assert_eq!(
            diff.after,
            Some(doc["work_items"][0]["completion_policy"].clone())
        );
        let yaml = serde_yaml_ng::to_string(&doc).unwrap();
        let same = prepare(yaml.as_bytes(), Some(bundle.clone())).unwrap();
        assert_eq!(
            same.bundle().unwrap().hash().unwrap(),
            bundle.hash().unwrap()
        );
        assert!(same.preview.completion_policy_diffs.is_empty());
        let initial = prepare(yaml.as_bytes(), None).unwrap();
        assert_eq!(initial.preview.completion_policy_diffs[0].before, None);
        assert_eq!(initial.preview.completion_policy_diffs[0].after, diff.after);
        doc["work_items"][0]["completion_policy"] =
            json!(awr_team::ExecutionSettlementPolicy::COMPLETION_POLICY);
        let changed = prepare(&serde_json::to_vec(&doc).unwrap(), Some(bundle)).unwrap();
        assert_eq!(changed.parser_version, PARSER_VERSION_V3);
        assert_eq!(
            changed.preview.completion_policy_diffs[0].before,
            diff.after
        );
        assert_eq!(
            changed.preview.completion_policy_diffs[0].after,
            diff.before
        );
        assert_eq!(
            changed.bundle().unwrap().hash().unwrap(),
            older.bundle().unwrap().hash().unwrap()
        );
        assert_eq!(fs::read(root.join("ledger.yaml")).unwrap(), original);
    }

    #[test]
    fn simulated_member_source_policy_never_conceals_invalid_settlement() {
        let root = fixture_root();
        let original = fs::read(root.join("ledger.yaml")).unwrap();
        let baseline = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap();
        let mut doc: Value = serde_yaml_ng::from_slice(&original).unwrap();
        doc["work_items"][0]["completion_policy"] =
            json!(awr_team::ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY);
        doc["work_items"][0]["verification_requirements"] = json!(["verify artifact bytes"]);
        let prepare = |doc: &Value| {
            prepare_publish_from_ledger_bytes(
                &baseline.source_location,
                &root,
                serde_json::to_vec(doc).unwrap().as_slice(),
                "demo-project",
                &PublishPrepOptions::default(),
            )
        };
        assert!(prepare(&doc).is_err());
        for policy in [
            Value::Null,
            json!({}),
            json!({"mode":"trusted_executor","workspace_id":"worker-a"}),
            json!({"mode":"independent_workspace_v1","workspace_id":"worker/path"}),
            json!({"mode":"independent_workspace_v1","workspace_id":"worker-a","human_approval":true}),
        ] {
            doc["work_items"][0]["execution_settlement"] = policy;
            assert!(prepare(&doc).is_err());
        }
        doc["work_items"][0]["execution_settlement"] =
            json!({"mode":"independent_workspace_v1","workspace_id":"worker-a"});
        assert!(prepare(&doc).is_ok());
        for field in ["paths", "verification_requirements"] {
            let mut empty = doc.clone();
            empty["work_items"][0][field] = json!([]);
            assert!(prepare(&empty).is_err());
        }
        for malformed in [Value::Null, json!(false), json!(42), json!("")] {
            let mut invalid = doc.clone();
            invalid["work_items"][0]["completion_policy"] = malformed;
            assert!(prepare(&invalid).is_err());
        }
        assert_eq!(fs::read(root.join("ledger.yaml")).unwrap(), original);
    }

    #[test]
    fn invalid_source_settlement_requests_are_rejected_instead_of_dropped() {
        let root = fixture_root();
        let baseline = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap();
        let mut doc: Value =
            serde_yaml_ng::from_slice(&fs::read(root.join("ledger.yaml")).unwrap()).unwrap();
        doc["work_items"][0]["completion_policy"] =
            json!(awr_team::ExecutionSettlementPolicy::COMPLETION_POLICY);
        doc["work_items"][0]["verification_requirements"] = json!(["verify artifact"]);
        let prepare = |doc: &Value| {
            prepare_publish_from_ledger_bytes(
                &baseline.source_location,
                &root,
                serde_yaml_ng::to_string(doc).unwrap().as_bytes(),
                "demo-project",
                &PublishPrepOptions::default(),
            )
        };
        for policy in [
            Value::Null,
            json!({}),
            json!(false),
            json!({"mode":"trusted_executor","workspace_id":"worker"}),
            json!({"mode":"independent_workspace_v1","workspace_id":""}),
            json!({"mode":"independent_workspace_v1","workspace_id":"../shared"}),
            json!({"mode":"independent_workspace_v1","workspace_id":"worker","trusted_executor":true}),
        ] {
            doc["work_items"][0]["execution_settlement"] = policy;
            assert!(prepare(&doc).is_err());
        }
        doc["work_items"][0]["execution_settlement"] =
            json!({"mode":"independent_workspace_v1","workspace_id":"worker"});
        prepare(&doc).unwrap();
        let wire = doc.to_string();
        for (before, after) in [
            (
                "\"workspace_id\":\"worker\"",
                "\"workspace_id\":\"other\",\"workspace_id\":\"worker\"",
            ),
            (
                "\"mode\":\"independent_workspace_v1\"",
                "\"mode\":\"trusted_executor\",\"mode\":\"independent_workspace_v1\"",
            ),
            (
                "\"execution_settlement\":",
                "\"execution_settlement\":null,\"execution_settlement\":",
            ),
            (
                "\"completion_policy\":",
                "\"completion_policy\":\"ordinary_confirm\",\"completion_policy\":",
            ),
        ] {
            let duplicate = wire.replace(before, after);
            assert_ne!(duplicate, wire);
            let err = prepare_publish_from_ledger_bytes(
                &baseline.source_location,
                &root,
                duplicate.as_bytes(),
                "demo-project",
                &PublishPrepOptions::default(),
            )
            .unwrap_err();
            assert!(err.to_string().contains("duplicate entry"), "{err}");
        }
        // Block-style YAML must retain the same strictness as JSON-as-YAML.
        let yaml = serde_yaml_ng::to_string(&doc).unwrap();
        let duplicate = yaml.replace(
            "workspace_id: worker",
            "workspace_id: other\n    workspace_id: worker",
        );
        assert_ne!(duplicate, yaml);
        let err = prepare_publish_from_ledger_bytes(
            &baseline.source_location,
            &root,
            duplicate.as_bytes(),
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate entry"), "{err}");
        for invalid in [Value::Null, json!(false), json!(42), json!(""), json!({})] {
            doc["work_items"][0]["completion_policy"] = invalid;
            let options = PublishPrepOptions {
                baseline: None,
                completion_policy: Some(
                    awr_team::ExecutionSettlementPolicy::COMPLETION_POLICY.into(),
                ),
            };
            assert!(
                prepare_publish_from_ledger_bytes(
                    &baseline.source_location,
                    &root,
                    serde_yaml_ng::to_string(&doc).unwrap().as_bytes(),
                    "demo-project",
                    &options,
                )
                .is_err()
            );
        }
        doc["work_items"][0]
            .as_object_mut()
            .unwrap()
            .remove("completion_policy");
        assert!(prepare(&doc).is_err());
        doc["work_items"][0]["completion_policy"] =
            json!(awr_team::ExecutionSettlementPolicy::COMPLETION_POLICY);
        doc["work_items"][0]["verification_requirements"] = json!([]);
        assert!(prepare(&doc).is_err());
    }

    #[test]
    fn missing_acceptance_hard_rejects() {
        let root = fixture_root();
        let err = prepare_publish_from_server_directory(
            &root,
            "missing-acceptance.yaml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("acceptance required"), "{err}");
    }

    #[test]
    fn unsupported_format_hard_rejects() {
        let root = fixture_root();
        let err = prepare_publish_from_server_directory(
            &root,
            "unsupported.xml",
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("unsupported ledger format"),
            "{err}"
        );
    }

    #[test]
    fn role_collections_are_not_invented_into_permissions() {
        let root = fixture_root();
        let mut doc: Value =
            serde_yaml_ng::from_slice(&fs::read(root.join("ledger.yaml")).unwrap()).unwrap();
        doc.as_object_mut()
            .unwrap()
            .insert("roles".into(), serde_json::json!([{"name":"admin"}]));
        let location = SoleSourceLocation::server_directory(&root, "ledger.yaml").unwrap();
        let bytes = serde_yaml_ng::to_string(&doc).unwrap().into_bytes();
        let err = prepare_publish_from_ledger_bytes(
            &location,
            &root,
            &bytes,
            "demo-project",
            &PublishPrepOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("roles"), "{err}");
    }

    #[test]
    fn private_repo_locator_requires_safe_scheme() {
        assert!(
            SoleSourceLocation::private_management_repo(
                "https://git.example/team/awr-ledger.git#rev",
                "ledger/work-ledger.yaml"
            )
            .is_ok()
        );
        assert!(
            SoleSourceLocation::private_management_repo("/tmp/not-a-repo", "ledger.yaml").is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn root_confined_open_accepts_in_root_files_and_refuses_escaping_symlinks() {
        let root = fixture_root();
        let package = prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            "demo",
            &PublishPrepOptions::default(),
        )
        .unwrap();
        assert!(
            package
                .files
                .iter()
                .any(|f| f.path == "contracts/api-v1.json" && !f.bytes.is_empty())
        );

        let tmp = std::env::temp_dir().join(format!("awr-tmcp020-symlink-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("contracts")).unwrap();
        // Copy fixture into disposable tree.
        fs::copy(root.join("ledger.yaml"), tmp.join("ledger.yaml")).unwrap();
        fs::copy(
            root.join("contracts/api-v1.json"),
            tmp.join("contracts/api-v1.json"),
        )
        .unwrap();
        fs::copy(
            root.join("contracts/client-v1.md"),
            tmp.join("contracts/client-v1.md"),
        )
        .unwrap();
        let outside = tmp.join("outside-secret.json");
        fs::write(&outside, br#"{"secret":"OUTSIDE_SENTINEL"}"#).unwrap();
        fs::remove_file(tmp.join("contracts/api-v1.json")).unwrap();
        std::os::unix::fs::symlink(&outside, tmp.join("contracts/api-v1.json")).unwrap();

        let err = prepare_publish_from_server_directory(
            &tmp,
            "ledger.yaml",
            "demo",
            &PublishPrepOptions::default(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("contracts/api-v1.json")
                || msg.contains("refused")
                || msg.contains("symlink")
                || msg.contains("RuleViolation")
                || msg.contains("unsafe"),
            "escaping symlink must fail: {msg}"
        );
        // Ensure outside sentinel never packages.
        assert!(!msg.contains("OUTSIDE_SENTINEL"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn distinct_original_yaml_revisions_differ_in_persisted_package() {
        let root = fixture_root();
        let location = SoleSourceLocation::server_directory(&root, "ledger.yaml").unwrap();
        let original = fs::read(root.join("ledger.yaml")).unwrap();
        let mut revised = original.clone();
        revised.extend_from_slice(b"\n# revision-marker-2\n");
        let options = PublishPrepOptions::default();
        let a = prepare_publish_from_ledger_bytes(&location, &root, &original, "demo", &options)
            .unwrap();
        let b = prepare_publish_from_ledger_bytes(&location, &root, &revised, "demo", &options)
            .unwrap();
        assert_ne!(a.source_version_digest, b.source_version_digest);
        let a_ledger = a
            .files
            .iter()
            .find(|f| f.path == "ledger.yaml")
            .expect("original ledger bytes");
        let b_ledger = b
            .files
            .iter()
            .find(|f| f.path == "ledger.yaml")
            .expect("original ledger bytes");
        assert_ne!(a_ledger.bytes, b_ledger.bytes);
        assert_eq!(a_ledger.bytes, original);
        assert_eq!(b_ledger.bytes, revised);
        let a_prov = a
            .files
            .iter()
            .find(|f| f.path == SOURCE_PROVENANCE_FILE)
            .expect("provenance");
        let b_prov = b
            .files
            .iter()
            .find(|f| f.path == SOURCE_PROVENANCE_FILE)
            .expect("provenance");
        assert_ne!(a_prov.bytes, b_prov.bytes);
        let a_meta: SourceProvenance = serde_json::from_slice(&a_prov.bytes).unwrap();
        assert_eq!(a_meta.source_version_digest, a.source_version_digest);
        assert_eq!(a_meta.ledger_relative_path, "ledger.yaml");
    }
}
