//! The project policy file: read and switch the sensitive-admission mode.
//!
//! `awr policy secrets <strict|warn>` is the single command that switches the
//! mode; `show` prints the active mode with its provenance, and `log` lists the
//! receipts recorded for every warn-mode admission. Strict is the default and
//! the behaviour AWR always had; warn admits a flagged source with a receipt
//! instead of stopping the takeover.
use awr_core::{Error, Result};
use clap::{Args, Subcommand};
use std::path::Path;

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
    let policy_path = root.join(awr_source::POLICY_FILE);
    let text = match std::fs::read_to_string(&policy_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            format!("version = {}\n", awr_source::POLICY_VERSION)
        }
        Err(error) => return Err(Error::Io(error)),
    };
    if let Err(error) = awr_source::SecretsPolicy::load(root) {
        return Err(error);
    }
    let enabled_by = actor
        .or_else(|| {
            std::env::var("USER")
                .ok()
                .or_else(|| std::env::var("USERNAME").ok())
        })
        .unwrap_or_else(|| "unknown".into());
    let enabled_at = awr_core::now_millis()?;
    let text = write_secrets_table(&text, mode, &enabled_by, enabled_at)?;
    if let Some(parent) = policy_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&policy_path, text)?;
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
    let table = format!(
        "\n[secrets]\nmode = \"{mode}\"\nenabled_by = \"{enabled_by}\"\nenabled_at = {enabled_at}\n"
    );
    let mut lines: Vec<String> = Vec::new();
    let mut in_secrets = false;
    let mut replaced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if in_secrets {
                // Closing the existing table: the replacement is inserted before it.
                if !replaced {
                    lines.push(table.trim_end().to_owned());
                    replaced = true;
                }
                in_secrets = false;
            }
            if trimmed == "[secrets]" {
                in_secrets = true;
                continue;
            }
        }
        if !in_secrets {
            lines.push(line.to_owned());
        }
    }
    if in_secrets && !replaced {
        // The table was the last one in the file.
        lines.push(table.trim_end().to_owned());
        replaced = true;
    }
    let mut text = lines.join("\n");
    if !replaced {
        if !text.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&table);
    } else if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
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
    let mut store = awr_store::Store::open_existing(&database)?;
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
