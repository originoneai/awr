use awr_core::{AuthorityMode, Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// Adapter identifiers accepted by source manifests in this build.
pub const SOURCE_ADAPTERS: &[&str] = &[
    "yaml-ledger-v1",
    "yaml-workstream-ledger-v1",
    "markdown-ledger-v1",
    "markdown-heading-v1",
    "markdown-rules-v1",
    "markdown-directory-v1",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub project: ProjectConfig,
    pub sources: Vec<SourceSpec>,
    /// Runtime switch for Team surfaces. Omitted (and therefore off) unless set.
    #[serde(default, skip_serializing_if = "TeamConfig::is_default")]
    pub team: TeamConfig,
}
/// `[team]` table in `.awr/project.toml`. Team code is always compiled; this
/// switch only decides whether Team commands and MCP tools are offered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamConfig {
    #[serde(default)]
    pub enabled: bool,
}
impl TeamConfig {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Environment override for the Team switch; takes precedence over `[team]`.
pub const TEAM_ENABLED_ENV: &str = "AWR_TEAM_ENABLED";
/// Error detail returned when a Team surface is used while the switch is off.
pub const TEAM_DISABLED_MESSAGE: &str = "team features are disabled; enable them with `[team] enabled = true` in .awr/project.toml or AWR_TEAM_ENABLED=1";

/// Reads the `AWR_TEAM_ENABLED` override. The submitted value is never echoed.
pub fn team_enabled_override() -> Result<Option<bool>> {
    let Some(raw) = std::env::var_os(TEAM_ENABLED_ENV) else {
        return Ok(None);
    };
    match raw
        .to_str()
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("1" | "true" | "yes" | "on") => Ok(Some(true)),
        Some("" | "0" | "false" | "no" | "off") => Ok(Some(false)),
        _ => Err(Error::InvalidInput(format!(
            "{TEAM_ENABLED_ENV} must be one of 1, true, yes, on, 0, false, no or off"
        ))),
    }
}

/// Resolves the Team switch for a project: the environment override first,
/// then `[team] enabled` in `.awr/project.toml`, otherwise off.
pub fn team_enabled(root: &Path) -> Result<bool> {
    if let Some(enabled) = team_enabled_override()? {
        return Ok(enabled);
    }
    if !root.join(".awr/project.toml").is_file() {
        return Ok(false);
    }
    Ok(Manifest::load(root)?.team.enabled)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: String,
    pub external_key: Option<String>,
    #[serde(default = "source_first")]
    pub authority_mode: AuthorityMode,
    #[serde(default)]
    pub authorized_roots: Vec<PathBuf>,
    /// Minimal projects can omit separate rules/milestone sources. Configured rules still apply.
    #[serde(default, skip_serializing_if = "ContextProfile::is_standard")]
    pub context_profile: ContextProfile,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProfile {
    #[default]
    Standard,
    Minimal,
}
impl ContextProfile {
    fn is_standard(&self) -> bool {
        *self == Self::Standard
    }
}
/// The explicit manifest profile is bound to every indexed source configuration/revision.
pub fn minimal_context(sources: &[awr_core::Source]) -> bool {
    !sources.is_empty()
        && sources
            .iter()
            .all(|s| s.config["context_profile"] == "minimal")
}
/// Runtime-owned paths are machine state under `.awr/` that AWR never rewrites
/// through source mutations. Authored intake sources under `.awr/intake/` are
/// the documented exception: takeover proposes them as authoritative source
/// files (see docs/dev/TAKEOVER.md), so review and writeback paths treat them
/// like any other in-project source. Both `root` and `path` must be canonical.
pub fn is_runtime_owned_path(root: &Path, path: &Path) -> bool {
    path.starts_with(root.join(".awr")) && !path.starts_with(root.join(".awr/intake"))
}
fn source_first() -> AuthorityMode {
    AuthorityMode::SourceFirst
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    pub domain: String,
    pub role: String,
    pub path: Option<PathBuf>,
    pub locator: Option<String>,
    pub adapter: String,
    #[serde(default)]
    pub options: toml::Table,
}
impl Manifest {
    pub fn load(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let path = root
            .join(".awr/project.toml")
            .canonicalize()
            .map_err(|e| Error::SourceUnavailable(e.to_string()))?;
        if !path.starts_with(&root) {
            return Err(Error::RuleViolation("manifest escapes project root".into()));
        }
        let bytes = crate::read_source_capped(&path, 64 * 1024)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| Error::InvalidInput(e.to_string()))?;
        Self::parse(text)
    }
    pub fn parse(text: &str) -> Result<Self> {
        awr_core::ensure_public_text(text)?;
        let value: toml::Value = toml::from_str(text)
            .map_err(|_| Error::InvalidInput("invalid source manifest TOML".into()))?;
        awr_core::ensure_public_data(&value)?;
        let manifest: Self = value.try_into().map_err(|error| {
            Error::InvalidInput(format!(
                "source manifest does not match its schema: {}",
                awr_core::redact_schema_error_detail(&error.to_string())
            ))
        })?;
        manifest.validate()?;
        Ok(manifest)
    }
    pub fn validate(&self) -> Result<()> {
        awr_core::ensure_public_data(self)?;
        if self.project.name.trim().is_empty() || self.sources.is_empty() {
            return Err(Error::InvalidInput(
                "manifest needs a project name and at least one source".into(),
            ));
        }
        if self
            .project
            .external_key
            .as_ref()
            .is_some_and(|key| key.trim().is_empty())
        {
            return Err(Error::InvalidInput(
                "project external_key must not be empty".into(),
            ));
        }
        let mut unique = BTreeSet::new();
        for source in &self.sources {
            if !["goal", "plan", "rules", "ledger", "decisions"].contains(&source.domain.as_str())
                || !["primary", "supporting"].contains(&source.role.as_str())
            {
                return Err(Error::InvalidInput(
                    "source domain/role must be one of the supported domain and role combinations"
                        .into(),
                ));
            }
            if !SOURCE_ADAPTERS.contains(&source.adapter.as_str()) {
                return Err(Error::Unsupported(format!(
                    "source adapter must be one of: {}",
                    SOURCE_ADAPTERS.join(", ")
                )));
            }
            if source.path.is_some() == source.locator.is_some() {
                return Err(Error::InvalidInput(
                    "each source needs exactly one path or locator".into(),
                ));
            }
            if matches!(
                source.adapter.as_str(),
                "yaml-ledger-v1" | "yaml-workstream-ledger-v1" | "markdown-ledger-v1"
            ) {
                let mapping = crate::LedgerMapping::from_spec(source)?;
                if mapping.ordinary_work_policy.is_some()
                    && (source.domain != "ledger"
                        || self.project.context_profile != ContextProfile::Minimal)
                {
                    return Err(Error::InvalidInput("ordinary policy is scoped to a ledger and requires explicit context_profile = minimal; configured rules still apply".into()));
                }
            }
            if source.adapter == "yaml-workstream-ledger-v1"
                && (source.domain != "ledger" || source.role != "primary")
            {
                return Err(Error::InvalidInput(
                    "workstream ledger must be a primary ledger source".into(),
                ));
            }
            if source
                .path
                .as_ref()
                .is_some_and(|p| p.as_os_str().is_empty())
                || source.locator.as_ref().is_some_and(|p| p.is_empty())
            {
                return Err(Error::InvalidInput("empty source path/locator".into()));
            }
            let location = source
                .locator
                .clone()
                .unwrap_or_else(|| source.path.as_ref().unwrap().to_string_lossy().into_owned());
            if !unique.insert((source.domain.clone(), location)) {
                return Err(Error::InvalidInput(
                    "duplicate source domain/location".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn authorized_roots(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let root = root.canonicalize()?;
        let mut roots = vec![root.clone()];
        for extra in &self.project.authorized_roots {
            let path = if extra.is_absolute() {
                extra.clone()
            } else {
                root.join(extra)
            }
            .canonicalize()?;
            if !path.is_dir() {
                return Err(Error::InvalidInput(
                    "authorized root must be a directory".into(),
                ));
            }
            roots.push(path);
        }
        roots.sort();
        roots.dedup();
        Ok(roots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_text(context_profile: &str, adapter: &str) -> String {
        format!(
            "[project]\nname = 'P'\ncontext_profile = '{context_profile}'\n\n[[sources]]\ndomain = 'decisions'\nrole = 'supporting'\npath = 'decisions'\nadapter = '{adapter}'\n"
        )
    }

    #[test]
    fn manifest_schema_error_redacts_unknown_enum_variant() {
        let secret = "private-variant-9f8e7d6c5b4a";
        let error = Manifest::parse(&manifest_text(secret, "markdown-directory-v1")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("does not match its schema"), "{message}");
        assert!(message.contains("unknown variant `…`"), "{message}");
        assert!(!message.contains(secret), "{message}");
    }

    #[test]
    fn manifest_semantic_error_does_not_echo_submitted_adapter() {
        let secret = "private-variant-9f8e7d6c5b4a";
        let error = Manifest::parse(&manifest_text("minimal", secret)).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("source adapter must be one of"),
            "{message}"
        );
        assert!(!message.contains(secret), "{message}");
    }
}
