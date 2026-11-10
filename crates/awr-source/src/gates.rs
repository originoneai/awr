//! Opt-in project gates. Reserved tables are extended by their respective consumers.
use awr_core::{Error, Result};
use globset::GlobBuilder;
use serde::Deserialize;
use std::path::Path;

pub const GATES_FILE: &str = ".awr/gates.toml";
const READ_CAP: u64 = 64 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatePolicy {
    pub version: u64,
    #[serde(default)]
    pub checks: CheckGates,
    #[serde(default)]
    pub hooks: HookGates,
    #[serde(default)]
    pub verify: VerifyGates,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckGates {}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyGates {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteGate {
    #[default]
    Off,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HookGates {
    pub write_gate: WriteGate,
    pub write_gate_exempt: Vec<String>,
    pub write_gate_paths: bool,
    pub completion_reminder: bool,
}

impl Default for GatePolicy {
    fn default() -> Self {
        Self {
            version: 1,
            checks: CheckGates::default(),
            hooks: HookGates::default(),
            verify: VerifyGates::default(),
        }
    }
}
impl GatePolicy {
    pub fn parse(text: &str) -> Result<Self> {
        let policy: Self = toml::from_str(text).map_err(|_| {
            Error::InvalidInput(format!(
                "{GATES_FILE}: expected version = 1 and known gate tables, keys and values"
            ))
        })?;
        if policy.version != 1 {
            return Err(Error::Unsupported(format!(
                "{GATES_FILE}: unsupported version; this build reads version 1"
            )));
        }
        for pattern in &policy.hooks.write_gate_exempt {
            glob(pattern)?;
        }
        Ok(policy)
    }
    pub fn load(root: &Path) -> Result<Self> {
        match std::fs::symlink_metadata(root.join(GATES_FILE)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
            Ok(meta) if meta.len() > READ_CAP => {
                return Err(Error::InvalidInput(format!("{GATES_FILE} exceeds 64 KiB")));
            }
            Ok(_) => (),
        }
        let bytes = crate::read_under_root(root, GATES_FILE)?;
        if bytes.len() as u64 > READ_CAP {
            return Err(Error::InvalidInput(format!("{GATES_FILE} exceeds 64 KiB")));
        }
        Self::parse(
            std::str::from_utf8(&bytes)
                .map_err(|_| Error::InvalidInput(format!("{GATES_FILE} must be UTF-8")))?,
        )
    }
}
fn glob(pattern: &str) -> Result<globset::Glob> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|_| Error::InvalidInput(format!("{GATES_FILE}: invalid path glob")))
}
impl HookGates {
    pub fn is_exempt(&self, relative: &Path) -> Result<bool> {
        for pattern in &self.write_gate_exempt {
            if glob(pattern)?.compile_matcher().is_match(relative) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Work paths may name a file, a directory subtree, or a project-relative glob.
pub fn hook_work_path_matches(patterns: &[String], relative: &Path) -> Result<bool> {
    for pattern in patterns {
        if relative == Path::new(pattern)
            || relative.starts_with(Path::new(pattern))
            || glob(pattern)?.compile_matcher().is_match(relative)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
