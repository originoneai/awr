use crate::registry;
use awr_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub service: String,
    pub protocol: u32,
    pub instance: String,
    pub pid: u32,
    pub url: String,
    pub executable: PathBuf,
    pub binary_stamp: String,
    pub version: String,
    pub started_at: i64,
}

pub(crate) fn stamp(path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path)?;
    Ok(format!("{}:{:?}", meta.len(), meta.modified()?))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_millis(500)))
        .build()
        .into()
}

fn endpoint(status: &ServiceStatus) -> Result<String> {
    let url = url::Url::parse(&status.url)
        .map_err(|_| Error::InvalidInput("invalid local service address".into()))?;
    if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") || url.port().is_none() {
        return Err(Error::InvalidInput(
            "local service must use loopback HTTP".into(),
        ));
    }
    Ok(format!("http://127.0.0.1:{}", url.port().unwrap()))
}

pub fn running(dir: &Path) -> Result<Option<ServiceStatus>> {
    let path = dir.join("service.json");
    if !path.exists() {
        return Ok(None);
    }
    let expected: ServiceStatus = registry::read_json(&path)?;
    let address = endpoint(&expected)?;
    let observed = (|| -> Option<ServiceStatus> {
        let mut response = agent().get(format!("{address}/api/health")).call().ok()?;
        let body = response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_string()
            .ok()?;
        let value: serde_json::Value = serde_json::from_str(&body).ok()?;
        serde_json::from_value(value.get("data")?.clone()).ok()
    })();
    Ok(observed.filter(|actual| {
        actual.service == "awr-project-hub"
            && actual.protocol == 1
            && actual.instance == expected.instance
            && actual.pid == expected.pid
    }))
}

pub fn stop(dir: &Path) -> Result<()> {
    let Some(status) = running(dir)? else {
        return Ok(());
    };
    agent()
        .post(format!("{}/api/service/stop", endpoint(&status)?))
        .header("x-awr-local-instance", &status.instance)
        .send_empty()
        .map_err(|_| Error::Storage("local service did not accept shutdown".into()))?;
    let start = Instant::now();
    while running(dir)?.is_some() {
        if start.elapsed() > Duration::from_secs(3) {
            return Err(Error::Storage(
                "local service is still draining requests".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _lease = registry::lock(dir, "server.lock", Duration::from_secs(3))?;
    Ok(())
}

pub fn ensure(dir: &Path, executable: &Path) -> Result<ServiceStatus> {
    let executable = executable.canonicalize()?;
    let current_stamp = stamp(&executable)?;
    let matches = |status: &ServiceStatus| {
        status.version == env!("CARGO_PKG_VERSION")
            && (status.executable != executable || status.binary_stamp == current_stamp)
    };
    if let Some(status) = running(dir)? {
        if matches(&status) {
            return Ok(status);
        }
    }
    let _start = registry::lock(dir, "start.lock", Duration::from_secs(5))?;
    if let Some(status) = running(dir)? {
        if matches(&status) {
            return Ok(status);
        }
        stop(dir)?;
    }
    let log = registry::private_file(&dir.join("service.log"))?;
    log.set_len(0)?;
    let mut command = Command::new(&executable);
    command
        .args(["overview", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // The background viewer only needs the user's local runtime environment.
    command.env_clear();
    for name in [
        "HOME",
        "USERPROFILE",
        "LOCALAPPDATA",
        "XDG_STATE_HOME",
        "PATH",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SystemRoot",
        "WINDIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("AWR_LOCAL_HOME", dir).env("AWR_LOCAL_WEB", "0");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x00000200);
    }
    let mut child = command.spawn()?;
    let start = Instant::now();
    loop {
        if let Some(status) = running(dir)? {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(status);
        }
        if child.try_wait()?.is_some() {
            return Err(Error::Storage(
                "local service exited during startup; inspect service.log".into(),
            ));
        }
        if start.elapsed() > Duration::from_secs(4) {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Err(Error::Storage(
                "local service startup is pending; inspect overview status before retrying".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
