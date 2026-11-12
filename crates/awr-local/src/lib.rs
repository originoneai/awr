//! Per-user project discovery and the bundled loopback progress service.
mod discovery;
mod intake;
mod lifecycle;
mod registry;
mod server;

pub use discovery::config as discovery_config;
pub use discovery::{DiscoveryConfig, DiscoveryReport, configure, discover, discovery_status};
pub use intake::project_intake;
pub use lifecycle::{ServiceStatus, detach_background, ensure, running, stop};
pub use registry::{
    LocalProject, ProjectState, default_state_dir, get, inspect, is_default_state_dir, list,
    observe, register, state_dir, touch,
};

use std::path::{Path, PathBuf};

pub fn enabled() -> bool {
    std::env::var_os("AWR_LOCAL_WEB").is_none_or(|value| value != "0")
}

/// Whether ordinary commands add the projects they touch to the local catalog.
///
/// Off by default: registration is explicit (`awr overview register`). Setting
/// `AWR_LOCAL_AUTO_REGISTER=1` restores the earlier behavior of registering on use.
pub fn auto_register() -> bool {
    std::env::var_os("AWR_LOCAL_AUTO_REGISTER").is_some_and(|value| value == "1")
}

/// Observation is best-effort and must not change CLI or MCP command results.
///
/// A project that is already registered has its usage time refreshed. An unregistered one is
/// only added when `auto_register` is on.
pub fn maintain(project: Option<&Path>) {
    if !enabled() {
        return;
    }
    let Ok(dir) = state_dir() else { return };
    if let Some(root) = project {
        if auto_register() {
            let _ = observe(&dir, root);
        } else {
            let _ = touch(&dir, root);
        }
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
