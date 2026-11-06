use awr_core::Result;
use clap::{Args, Subcommand};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Args)]
pub struct OverviewArgs {
    #[command(subcommand)]
    command: Option<OverviewCommand>,
    /// Print the address without opening a browser.
    #[arg(long)]
    no_open: bool,
}

#[derive(Debug, Subcommand)]
enum OverviewCommand {
    /// Read progress from one SQLite snapshot. Source files are synchronized only with --refresh.
    Snapshot {
        #[arg(long)]
        refresh: bool,
    },
    /// Start or reuse this user's background project hub.
    Start,
    /// Inspect the verified running service without starting it.
    Status,
    /// Stop this user's service; the next ordinary AWR call can start it again.
    Stop,
    /// Register initialized project directories without scanning the filesystem.
    Register {
        #[arg(required = true)]
        roots: Vec<PathBuf>,
    },
    /// List the local project catalog, including unavailable directories.
    Projects,
    /// Read the current directory's connection state and Agent intake/repair actions.
    Inspect,
    /// Discover existing AWR-managed projects, with explicit coverage reporting.
    Discover {
        #[arg(long = "root")]
        roots: Vec<PathBuf>,
        /// Apply a complete, reviewed discovery configuration JSON.
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        interval_seconds: Option<u64>,
    },
    #[command(hide = true)]
    Serve,
}

fn print<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("rundll32.exe");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

pub fn run(root: &Path, args: &OverviewArgs, json_output: bool) -> Result<()> {
    let dir = awr_local::state_dir()?;
    match &args.command {
        Some(OverviewCommand::Snapshot { refresh }) => {
            print(&awr_runtime::progress_snapshot(root, *refresh)?)
        }
        Some(OverviewCommand::Serve) => awr_local::serve(&dir, &awr_local::executable()?),
        Some(OverviewCommand::Status) => {
            print(&serde_json::json!({"ok":true,"service":awr_local::running(&dir)?}))
        }
        Some(OverviewCommand::Stop) => {
            awr_local::stop(&dir)?;
            print(&serde_json::json!({"ok":true,"service":null}))
        }
        Some(OverviewCommand::Inspect) => print(&awr_local::project_intake(root)?),
        Some(OverviewCommand::Projects) => print(
            &serde_json::json!({"ok":true,"projects":awr_local::list(&dir)?,"discovery":awr_local::discovery_status(&dir)?,"catalog_scope":if awr_local::is_default_state_dir(&dir) { "user" } else { "isolated" }}),
        ),
        Some(OverviewCommand::Discover {
            roots,
            config,
            interval_seconds,
        }) => {
            let mut settings = if let Some(path) = config {
                serde_json::from_slice::<awr_local::DiscoveryConfig>(&awr_source::read_capped(
                    path,
                    128 * 1024,
                )?)?
            } else {
                awr_local::discovery_config(&dir)?
            };
            if !roots.is_empty() {
                settings.roots = roots
                    .iter()
                    .map(std::path::absolute)
                    .collect::<std::io::Result<_>>()?;
            }
            if let Some(interval) = interval_seconds {
                settings.interval_seconds = *interval;
            }
            if config.is_some() || !roots.is_empty() || interval_seconds.is_some() {
                awr_local::configure(&dir, settings)?;
            }
            let report = match awr_local::discover(&dir) {
                Ok(report) => report,
                Err(awr_core::Error::SourceConflict(_)) => awr_local::discovery_status(&dir)?,
                Err(error) => return Err(error),
            };
            let service = awr_local::ensure(&dir, &awr_local::executable()?)?;
            print(&serde_json::json!({"ok":true,"discovery":report,"service":service}))
        }
        Some(OverviewCommand::Register { roots }) => {
            let mut entries = Vec::new();
            for root in roots {
                entries.push(awr_local::register(&dir, root)?);
            }
            let service = awr_local::ensure(&dir, &awr_local::executable()?)?;
            print(&serde_json::json!({"ok":true,"projects":entries,"service":service}))
        }
        Some(OverviewCommand::Start) | None => {
            let _ = awr_local::register(&dir, root);
            let status = awr_local::ensure(&dir, &awr_local::executable()?)?;
            if args.command.is_none() && !args.no_open && !json_output {
                open_browser(&status.url)?;
            }
            if json_output {
                print(&serde_json::json!({"ok":true,"service":status}))
            } else {
                println!("AWR Project Hub: {}", status.url);
                Ok(())
            }
        }
    }
}
