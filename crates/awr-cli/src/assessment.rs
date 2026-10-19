//! Developer assessment entry points: offline replay, shadow compare, advice kill-switch.
use awr_core::{Error, Result};
use awr_runtime::{
    ASSESSMENT_ADVICE_MODE_CAPABILITY, ASSESSMENT_REPLAY_CAPABILITY,
    ASSESSMENT_SHADOW_COMPARE_CAPABILITY, AdviceDeliveryMode, AttachExplanationOptions,
    apply_advice_delivery_mode, attach_according_to_advice_mode, hard_protections_after_disable,
    parse_replay_snapshot, replay_assessment_from_bytes, shadow_compare,
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

pub fn run(command: &AssessmentCommand, json_output: bool) -> Result<()> {
    match command {
        AssessmentCommand::Replay(args) => replay(args, json_output),
        AssessmentCommand::Compare(args) => compare(args, json_output),
        AssessmentCommand::AdviceMode(args) => advice_mode(args, json_output),
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
