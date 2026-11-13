//! The project policy file: read and switch the sensitive-admission mode.
//!
//! `awr policy secrets <strict|warn>` is the single command that switches the
//! mode; `show` prints the active mode with its provenance, and `log` lists the
//! receipts recorded for every warn-mode admission. Strict is the default and
//! the behaviour AWR always had; warn admits a flagged source with a receipt
//! instead of stopping the takeover.
use awr_core::{Error, Result};
use clap::{Args, Subcommand};
use std::{io::Write, path::Path};

#[derive(Debug, Args)]
pub struct PolicyArgs {
    #[command(subcommand)]
    pub command: PolicyCommand,
}

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    /// Read or switch the sensitive-content admission mode for this project.
    Secrets {
        #[command(subcommand)]
        command: SecretsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum SecretsCommand {
    /// Reject sensitive sources (the default; identical to no policy file).
    Strict {
        /// Name recorded as the operator who switched the mode.
        #[arg(long)]
        actor: Option<String>,
    },
    /// Admit flagged sources with a recorded receipt instead of stopping.
    Warn {
        /// Name recorded as the operator who enabled warn mode.
        #[arg(long)]
        actor: Option<String>,
    },
    /// Print the active mode and who last switched it.
    Show,
    /// List warn-mode admission receipts (most recent first).
    Log {
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

pub fn run(project: &Path, command: &PolicyCommand, json: bool) -> Result<()> {
    let root = project.canonicalize()?;
    match command {
        PolicyCommand::Secrets { command } => match command {
            SecretsCommand::Strict { actor } => switch(&root, "strict", actor.clone(), json),
            SecretsCommand::Warn { actor } => switch(&root, "warn", actor.clone(), json),
            SecretsCommand::Show => show(&root, json),
            SecretsCommand::Log { limit } => log(&root, *limit, json),
        },
    }
}

fn switch(root: &Path, mode: &str, actor: Option<String>, json: bool) -> Result<()> {
    let runtime = crate::source::runtime_dir(root, true)?;
    let directory = awr_source::open_dir_exact(&runtime)?;
    let lock_path = runtime.join("policy.lock");
    let lock = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            awr_source::open_file_exact(&lock_path)?
        }
        Err(error) => return Err(error.into()),
    };
    lock.try_lock().map_err(|_| {
        Error::SourceConflict(
            "another policy switch is in progress; retry after it finishes".into(),
        )
    })?;
    awr_source::SecretsPolicy::load(root)?;
    let previous = match directory.read("policy.toml") {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let text = previous
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .map_err(|_| Error::InvalidInput("policy must be UTF-8".into()))?
        .map(str::to_owned)
        .unwrap_or_else(|| format!("version = {}\n", awr_source::POLICY_VERSION));
    let enabled_by = actor
        .or_else(|| {
            std::env::var("USER")
                .ok()
                .or_else(|| std::env::var("USERNAME").ok())
        })
        .unwrap_or_else(|| "unknown".into());
    if enabled_by.trim().is_empty() || enabled_by.len() > 128 {
        return Err(Error::InvalidInput(
            "policy actor must contain 1..128 bytes".into(),
        ));
    }
    awr_core::ensure_public_text(&enabled_by)?;
    let enabled_at = awr_core::now_millis()?;
    let text = write_secrets_table(&text, mode, &enabled_by, enabled_at)?;
    let stage = format!("policy-{}.tmp", awr_core::Id::new());
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let result: Result<()> = (|| {
        let mut file = directory.open_with(&stage, &options)?.into_std();
        if previous.is_some() {
            let current = awr_source::open_file_exact(&runtime.join("policy.toml"))?;
            let permissions = current.metadata()?.permissions();
            if permissions.readonly() {
                return Err(Error::RuleViolation("policy file is read-only".into()));
            }
            file.set_permissions(permissions)?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        let current = match directory.read("policy.toml") {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if current != previous {
            return Err(Error::SourceConflict(
                "policy changed during the switch; read it again".into(),
            ));
        }
        directory.rename(&stage, &directory, "policy.toml")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = directory.remove_file(&stage);
    }
    result?;
    awr_source::SecretsPolicy::load(root)?.apply();
    let summary = serde_json::json!({
        "mode": mode,
        "enabled_by": enabled_by,
        "enabled_at": enabled_at,
        "policy_file": awr_source::POLICY_FILE,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("Sensitive admission mode: {mode}");
        println!("Switched by {enabled_by}");
        println!("Policy file: {}", awr_source::POLICY_FILE);
    }
    Ok(())
}

/// Replace or append the `[secrets]` table, preserving the rest of the file.
fn write_secrets_table(
    text: &str,
    mode: &str,
    enabled_by: &str,
    enabled_at: i64,
) -> Result<String> {
    let mut value: toml::Value = toml::from_str(text)
        .map_err(|_| Error::InvalidInput("policy must be valid TOML".into()))?;
    let table = value
        .as_table_mut()
        .ok_or_else(|| Error::InvalidInput("policy must be a TOML table".into()))?;
    table.insert(
        "secrets".into(),
        toml::Value::Table(toml::Table::from_iter([
            ("mode".into(), toml::Value::String(mode.into())),
            ("enabled_by".into(), toml::Value::String(enabled_by.into())),
            ("enabled_at".into(), toml::Value::Integer(enabled_at)),
        ])),
    );
    toml::to_string_pretty(&value)
        .map_err(|_| Error::InvalidInput("could not serialize project policy".into()))
}

fn show(root: &Path, json: bool) -> Result<()> {
    let policy = awr_source::SecretsPolicy::load(root)?;
    let mode = match policy.mode {
        awr_core::SecretsMode::Strict => "strict",
        awr_core::SecretsMode::Warn => "warn",
    };
    let value = serde_json::json!({
        "mode": mode,
        "declared": policy.declared,
        "enabled_by": policy.enabled_by,
        "enabled_at": policy.enabled_at,
        "policy_file": awr_source::POLICY_FILE,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("Sensitive admission mode: {mode}");
        match (&policy.enabled_by, policy.enabled_at) {
            (Some(by), Some(at)) => println!("Last switched by {by} at {at}"),
            _ => println!(
                "No switch recorded{}",
                if policy.declared {
                    ""
                } else {
                    " (no [secrets] table in the policy file)"
                }
            ),
        }
    }
    Ok(())
}

fn log(root: &Path, limit: usize, json: bool) -> Result<()> {
    let database = crate::source::runtime_dir(root, false)?.join("state.db");
    let store = awr_store::Store::open_existing(&database)?;
    let project = store.project_by_root(root)?;
    let events = store.secrets_warned_events(project.id, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&events)?);
    } else if events.is_empty() {
        println!("No warn-mode admission receipts recorded.");
    } else {
        for event in &events {
            let findings = event["findings"].as_array().map(Vec::len).unwrap_or(0);
            println!(
                "{}  {} finding(s)  enabled by {}",
                event["created_at"].as_i64().unwrap_or_default(),
                findings,
                event["enabled_by"].as_str().unwrap_or("unknown")
            );
            for finding in event["findings"].as_array().unwrap_or(&vec![]) {
                let line = finding["line"]
                    .as_u64()
                    .map(|l| format!(":{l}"))
                    .unwrap_or_default();
                println!(
                    "  {}{line}  {}",
                    finding["locator"].as_str().unwrap_or("?"),
                    finding["category"].as_str().unwrap_or("unknown")
                );
            }
        }
    }
    Ok(())
}
