//! Per-user project discovery and the bundled loopback progress service.
mod discovery;
mod lifecycle;
mod registry;
mod server;

pub use discovery::config as discovery_config;
pub use discovery::{DiscoveryConfig, DiscoveryReport, configure, discover, discovery_status};
pub use lifecycle::{ServiceStatus, detach_background, ensure, running, stop};
pub use registry::{
    LocalProject, ProjectState, default_state_dir, get, inspect, is_default_state_dir, list,
    observe, register, state_dir,
};

use std::path::{Path, PathBuf};

pub fn enabled() -> bool {
    std::env::var_os("AWR_LOCAL_WEB").is_none_or(|value| value != "0")
}

/// Observation is best-effort and must not change CLI or MCP command results.
pub fn maintain(project: Option<&Path>) {
    if !enabled() {
        return;
    }
    let Ok(dir) = state_dir() else { return };
    if let Some(root) = project {
        let _ = observe(&dir, root);
    }
    if let Ok(executable) = executable() {
        let _ = ensure(&dir, &executable);
    }
}

pub fn executable() -> awr_core::Result<PathBuf> {
    let current = std::env::current_exe()?;
    let path = if current.file_stem().is_some_and(|name| name == "awr-mcp") {
        current.with_file_name(if cfg!(windows) { "awr.exe" } else { "awr" })
    } else {
        current
    };
    Ok(path.canonicalize()?)
}

pub fn serve(dir: &Path, executable: &Path) -> awr_core::Result<()> {
    server::serve(dir, executable)
}
