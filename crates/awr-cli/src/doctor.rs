use awr_core::{Error, Id, Result, Revision};
use awr_store::{ReconcileAction, Store};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[arg(long)]
    database: Option<PathBuf>,
    /// Only inspect SQLite/schema; skip project sources and runtime/file diagnostics.
    #[arg(long)]
    database_only: bool,
    /// Maximum bytes to hash per registered artifact (sources have a separate 16 MiB cap).
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    max_bytes: u64,
    #[command(subcommand)]
    command: Option<DoctorCommand>,
}
#[derive(Debug, Subcommand)]
enum DoctorCommand {
    /// Apply exactly one named runtime repair, preserving sources and artifact files.
    Repair {
        #[command(subcommand)]
        action: RepairAction,
    },
}
#[derive(Debug, Args)]
struct RepairTarget {
    id: Id,
    #[arg(long)]
    expected_revision: Revision,
    #[arg(long)]
    reason: String,
}
#[derive(Debug, Subcommand)]
enum RepairAction {
    /// Mark a still-active claim expired only if its lease has actually elapsed.
    ExpireClaim(RepairTarget),
    /// Explicitly close a selected active session and release its claims.
    InterruptSession(RepairTarget),
    /// Close an unfinished save attempt; retain its draft and the previous checkpoint.
    AbandonCheckpoint(RepairTarget),
    /// Clear a current branch pointer only when the selected branch is invalid.
    ClearInvalidBranch(RepairTarget),
    /// Close the active sessions and release the claims that completed or cancelled work left behind.
    /// Without --accept only the plan and its fingerprint are shown; nothing is changed.
    FinishedWork(FinishedWorkArgs),
}
#[derive(Debug, Args)]
struct FinishedWorkArgs {
    /// Apply the plan that --expected-preview names.
    #[arg(long)]
    accept: bool,
    /// The fingerprint of the plan that was read; refused when the plan has changed since.
    #[arg(long)]
    expected_preview: Option<String>,
    #[arg(long)]
    expected_revision: Option<Revision>,
    /// Why; recorded as the note of every receipt.
    #[arg(long)]
    reason: Option<String>,
}

pub fn run(root: &Path, args: &DoctorArgs, json: bool) -> Result<()> {
    let path = args
        .database
        .clone()
        .unwrap_or_else(|| root.join(".awr/state.db"));
    if let Some(DoctorCommand::Repair {
        action: RepairAction::FinishedWork(finished),
    }) = &args.command
    {
        if args.database_only {
            return Err(Error::InvalidInput(
                "--database-only cannot be combined with a repair".into(),
            ));
        }
        return finished_work(root, &path, finished, json);
    }
    if let Some(DoctorCommand::Repair { action }) = &args.command {
        if args.database_only {
            return Err(Error::InvalidInput(
                "--database-only cannot be combined with a repair".into(),
            ));
        }
        let (target, action) = match action {
            RepairAction::ExpireClaim(target) => (
                target,
                ReconcileAction::ExpireClaim {
                    claim_id: target.id,
                },
            ),
            RepairAction::InterruptSession(target) => (
                target,
                ReconcileAction::InterruptSession {
                    session_id: target.id,
                },
            ),
            RepairAction::AbandonCheckpoint(target) => (
                target,
                ReconcileAction::AbandonCheckpoint {
                    attempt_id: target.id,
                },
            ),
            RepairAction::ClearInvalidBranch(target) => (
                target,
                ReconcileAction::ClearInvalidBranch {
                    branch_id: target.id,
                },
            ),
            RepairAction::FinishedWork(_) => unreachable!("handled above"),
        };
        if !Store::inspect(&path)?.ok {
            return Err(Error::Storage(
                "repair requires an intact current database; no migration was attempted".into(),
            ));
        }
        let root = root.canonicalize()?;
        let mut store = Store::open_existing(&path)?;
        let project = store.project_by_root(&root)?;
        let receipt =
            store.reconcile(project.id, target.expected_revision, action, &target.reason)?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"ok":true,"repair_applied":true,"read_only":false,"source_refresh_performed":false,"project_id":project.id,"project_revision":receipt.project_revision,"receipt":receipt,"remaining_problems_evaluated":false,"next_action":"Run doctor to inspect remaining findings."})
                )?
            );
        } else {
            println!(
                "Applied selected repair at revision {}. Receipt: {}\nRun doctor to inspect remaining findings.",
                receipt.project_revision, receipt.event.id
            );
        }
        return Ok(());
    }
    if args.database_only {
        let report = Store::inspect(&path)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "Database only\nSchema: {}\nIntegrity: {}\nForeign-key violations: {}",
                report.schema_version,
                report.integrity.join(", "),
                report.foreign_key_violations
            );
            for issue in &report.schema_issues {
                println!("Schema issue: {issue}");
            }
            if let Some(error) = &report.foreign_key_check_error {
                println!("Foreign-key check unavailable: {error}");
            }
        }
        return if report.ok {
            Ok(())
        } else {
            Err(Error::Storage(
                "doctor reported integrity or schema problems".into(),
            ))
        };
    }
    let report = awr_runtime::diagnose_project(root, &path, args.max_bytes)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Doctor (read only)\nDatabase intact: {}\nProject revision: {}\nSources checked: {}\nArtifacts checked: {}\nFindings: {}",
            report.database_ok,
            report
                .project_revision
                .map(|r| r.to_string())
                .unwrap_or_else(|| "unavailable".into()),
            report.sources_checked,
            report.artifacts_checked,
            report.findings.len()
        );
        for finding in &report.findings {
            println!(
                "[{}] {} {}:{} — {}",
                finding.severity,
                finding.code,
                finding.object_kind,
                finding.object_id,
                finding.message
            );
            if let Some(action) = &finding.repair {
                println!("  Repair candidate: {}", serde_json::to_string(action)?);
            }
        }
        println!(
            "No repair was applied. Select one named repair with --expected-revision and --reason when appropriate."
        );
    }
    if report.database.ok {
        Ok(())
    } else {
        Err(Error::Storage(
            "doctor found unresolved problems; inspect findings (no repairs applied)".into(),
        ))
    }
}

/// The explicit clean-up of the backlog: what completed or cancelled work left behind, shown first and
/// applied only on the plan that was read.
fn finished_work(root: &Path, path: &Path, args: &FinishedWorkArgs, json: bool) -> Result<()> {
    if !Store::inspect(path)?.ok {
        return Err(Error::Storage(
            "repair requires an intact current database; no migration was attempted".into(),
        ));
    }
    let root = root.canonicalize()?;
    let mut store = Store::open_existing(path)?;
    let project = store.project_by_root(&root)?;
    let plan = store.plan_finished_work(project.id)?;
    let fingerprint = plan.fingerprint();
    if !args.accept {
        if args.expected_preview.is_some()
            || args.expected_revision.is_some()
            || args.reason.is_some()
        {
            return Err(Error::InvalidInput(
                "--expected-preview, --expected-revision and --reason belong to --accept".into(),
            ));
        }
        let value = serde_json::json!({
            "ok": true, "preview": true, "read_only": true, "source_refresh_performed": false,
            "project_id": project.id, "project_revision": project.project_revision,
            "plan": plan, "preview_fingerprint": fingerprint,
            "basis": "the recorded projection; run source reindex first to judge the files as they are now",
            "accept_argv": ["doctor", "repair", "finished-work", "--accept", "--expected-preview", fingerprint,
                "--expected-revision", project.project_revision.to_string(), "--reason", "<why>"],
        });
        if json {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            println!(
                "Finished work left behind (nothing changed): {} session(s) to close, {} claim(s) to release, {} left alone.\nPlan: {fingerprint}",
                plan.sessions.len(),
                plan.claims.len(),
                plan.skipped.len()
            );
            for session in &plan.sessions {
                println!(
                    "  close session {} of {} on {} ({})",
                    session.session_id,
                    session.agent_id,
                    session.work_item_key,
                    session.work_status
                );
            }
            for claim in &plan.claims {
                println!(
                    "  release claim {} on {}",
                    claim.claim_id, claim.work_item_key
                );
            }
            for skipped in &plan.skipped {
                println!(
                    "  leave {} {} ({}): {}",
                    skipped.object_kind, skipped.object_id, skipped.work_item_key, skipped.reason
                );
            }
        }
        return Ok(());
    }
    let (Some(expected_preview), Some(expected_revision), Some(reason)) = (
        args.expected_preview.as_deref(),
        args.expected_revision,
        args.reason
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty()),
    ) else {
        return Err(Error::InvalidInput(
            "--accept needs --expected-preview, --expected-revision and a non-empty --reason"
                .into(),
        ));
    };
    if project.project_revision != expected_revision {
        return Err(Error::RevisionConflict {
            expected: expected_revision,
            actual: project.project_revision,
        });
    }
    if fingerprint != expected_preview {
        return Err(Error::MutationConflict(
            "the plan has changed since it was read; read it again before accepting".into(),
        ));
    }
    let report = store.apply_finished_work(project.id, &plan, reason)?;
    let live = store.project(project.id)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true, "repair_applied": true, "read_only": false, "source_refresh_performed": false,
                "project_id": project.id, "project_revision": live.project_revision, "applied": report,
            }))?
        );
    } else {
        println!(
            "Closed {} session(s) and released {} claim(s); {} left alone. Run doctor to inspect what remains.",
            report.interrupted_session_ids.len(),
            report.released_claim_ids.len(),
            report.skipped.len()
        );
    }
    Ok(())
}
