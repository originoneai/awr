//! Developer assessment entry points: offline replay, shadow compare, advice kill-switch.
use awr_core::{Error, PolicyCeiling, Result};
use awr_runtime::{
    ASSESSMENT_ADVICE_MODE_CAPABILITY, ASSESSMENT_REPLAY_CAPABILITY,
    ASSESSMENT_SHADOW_COMPARE_CAPABILITY, AdviceDeliveryMode, AttachExplanationOptions,
    apply_advice_delivery_mode, apply_policy_change, attach_according_to_advice_mode,
    hard_protections_after_disable, parse_replay_snapshot, preview_policy_change,
    replay_assessment_from_bytes, shadow_compare, show_policy,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum AssessmentCommand {
    /// Recompute an assessment from a frozen snapshot (offline; no store/tools/model).
    Replay(ReplayArgs),
    /// Compare baseline vs candidate snapshots on the same frozen inputs.
    Compare(CompareArgs),
    /// Inspect or apply advice delivery mode (kill-switch / shadow / enabled).
    AdviceMode(AdviceModeArgs),
    /// The project's assessment policy: show it, preview a change, write a confirmed change (DEC-040).
    Policy {
        #[command(subcommand)]
        command: PolicyCommand,
    },
}

pub const ASSESSMENT_POLICY_CAPABILITY: &str = "assessment.policy";

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    /// The policy in force and the versions the file holds. Reads only.
    Show(PolicyShowArgs),
    /// What a candidate policy file would change. Writes nothing; prints what to send back to confirm.
    Preview(PolicyChangeArgs),
    /// Write a previewed candidate, if the file still holds the bytes that were read.
    Apply(PolicyApplyArgs),
}

#[derive(Debug, Args)]
pub struct PolicyShowArgs {
    /// The `as_of` to resolve the policy for; default the project revision (what an explanation uses).
    #[arg(long)]
    as_of: Option<i64>,
}

#[derive(Debug, Args)]
pub struct PolicyChangeArgs {
    /// The candidate `assessment-policy.toml`: the whole file as it should be, with the versions already in use unchanged.
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    as_of: Option<i64>,
}

#[derive(Debug, Args)]
pub struct PolicyApplyArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    as_of: Option<i64>,
    /// `current_fingerprint` of the preview, or `absent` when the preview found no file.
    #[arg(long)]
    expected_current: String,
    /// `candidate_fingerprint` of the preview.
    #[arg(long)]
    expected_candidate: String,
}

#[derive(Debug, Args)]
pub struct ReplayArgs {
    /// Path to a replay snapshot JSON artifact. Omit to demonstrate not-replayable.
    #[arg(long)]
    snapshot: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CompareArgs {
    #[arg(long)]
    baseline: PathBuf,
    #[arg(long)]
    candidate: PathBuf,
}

#[derive(Debug, Args)]
pub struct AdviceModeArgs {
    /// disabled (kill-switch) | shadow | enabled
    #[arg(long, default_value = "disabled")]
    mode: String,
    /// Optional prepare/assess receipt JSON to attach according to mode.
    #[arg(long)]
    receipt: Option<PathBuf>,
}

pub fn run(root: &Path, command: &AssessmentCommand, json_output: bool) -> Result<()> {
    match command {
        AssessmentCommand::Replay(args) => replay(args, json_output),
        AssessmentCommand::Compare(args) => compare(args, json_output),
        AssessmentCommand::AdviceMode(args) => advice_mode(args, json_output),
        AssessmentCommand::Policy { command } => policy(root, command, json_output),
    }
}

/// The `as_of` an explanation would use: the project revision, read without refreshing anything.
fn as_of_or_revision(root: &Path, as_of: Option<i64>) -> Result<i64> {
    match as_of {
        Some(as_of) => Ok(as_of),
        None => {
            let query = crate::query::QueryProject::open_read(root, true)?;
            i64::try_from(query.project.project_revision)
                .map_err(|_| Error::InvalidInput("project revision out of range".into()))
        }
    }
}

fn policy(root: &Path, command: &PolicyCommand, json_output: bool) -> Result<()> {
    let ceiling = PolicyCeiling::builtin();
    match command {
        PolicyCommand::Show(args) => {
            let as_of = as_of_or_revision(root, args.as_of)?;
            let shown = show_policy(root, as_of, &ceiling);
            print_json(
                &json!({"ok": true, "capability": ASSESSMENT_POLICY_CAPABILITY, "policy": shown}),
                json_output,
            )
        }
        PolicyCommand::Preview(args) => {
            let as_of = as_of_or_revision(root, args.as_of)?;
            let preview = preview_policy_change(root, &read_file(&args.input)?, as_of, &ceiling)?;
            let ok = preview.ok;
            print_json(
                &json!({"ok": ok, "capability": ASSESSMENT_POLICY_CAPABILITY, "preview": preview}),
                json_output,
            )?;
            if ok { Ok(()) } else { std::process::exit(1) }
        }
        PolicyCommand::Apply(args) => {
            let as_of = as_of_or_revision(root, args.as_of)?;
            let expected_current =
                (args.expected_current != "absent").then_some(args.expected_current.as_str());
            let applied = apply_policy_change(
                root,
                &read_file(&args.input)?,
                expected_current,
                &args.expected_candidate,
                as_of,
                &ceiling,
            )?;
            print_json(
                &json!({"ok": true, "capability": ASSESSMENT_POLICY_CAPABILITY, "applied": applied}),
                json_output,
            )
        }
    }
}

fn print_json(value: &Value, _json_output: bool) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| Error::InvalidInput(format!("read {}: {e}", path.display())))
}

fn mode_str(mode: AdviceDeliveryMode) -> &'static str {
    match mode {
        AdviceDeliveryMode::Disabled => "disabled",
        AdviceDeliveryMode::Shadow => "shadow",
        AdviceDeliveryMode::Enabled => "enabled",
    }
}

fn replay(args: &ReplayArgs, json_output: bool) -> Result<()> {
    let report = match &args.snapshot {
        None => replay_assessment_from_bytes(None)?,
        Some(path) => replay_assessment_from_bytes(Some(&read_file(path)?))?,
    };
    let value = json!({
        "ok": true,
        "capability": ASSESSMENT_REPLAY_CAPABILITY,
        "report": report,
        "notes": [
            "offline_recompute_only",
            "does_not_reread_production_state",
            "does_not_rerun_tools",
            "does_not_call_model_or_network",
        ],
    });
    print_json(&value, json_output)
}

fn compare(args: &CompareArgs, json_output: bool) -> Result<()> {
    let baseline = parse_replay_snapshot(&read_file(&args.baseline)?)?;
    let candidate = parse_replay_snapshot(&read_file(&args.candidate)?)?;
    let report = shadow_compare(&baseline, &candidate)?;
    let value = json!({
        "ok": report.passed,
        "capability": ASSESSMENT_SHADOW_COMPARE_CAPABILITY,
        "report": report,
        "notes": [
            "same_input_compare_of_reasons_advice_hard_rejects_costs",
            "differences_explained_by_rule_version",
            "failed_samples_retained_in_full",
            "shadow_does_not_change_execution_or_context_adoption",
            "no_background_daemon",
        ],
    });
    print_json(&value, json_output)
}

fn advice_mode(args: &AdviceModeArgs, json_output: bool) -> Result<()> {
    let mode = AdviceDeliveryMode::parse(&args.mode)?;
    let effect = apply_advice_delivery_mode(mode);
    let protections = hard_protections_after_disable();
    let mut value = json!({
        "ok": true,
        "capability": ASSESSMENT_ADVICE_MODE_CAPABILITY,
        "mode": mode_str(mode),
        "effect": effect,
        "hard_protections_after_disable": protections,
        "notes": [
            "disable_restores_prior_advice_behavior_only",
            "hard_protections_remain",
            "no_background_daemon",
        ],
    });
    if let Some(path) = &args.receipt {
        let receipt: Value = serde_json::from_slice(&read_file(path)?)
            .map_err(|e| Error::InvalidInput(format!("receipt json: {e}")))?;
        let enabled = matches!(
            mode,
            AdviceDeliveryMode::Shadow | AdviceDeliveryMode::Enabled
        );
        let attached = attach_according_to_advice_mode(
            receipt,
            mode,
            AttachExplanationOptions {
                enabled,
                ..Default::default()
            },
        )?;
        value["receipt"] = attached;
    }
    print_json(&value, json_output)
}
