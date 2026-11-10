use crate::session::RuntimeProject;
use awr_core::*;
use awr_runtime::{RunCheckRequest, check_status, run_check};
use clap::Args;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn print(value: &serde_json::Value, text: &str, json_output: bool) -> Result<()> {
    use std::io::Write;
    if json_output {
        writeln!(std::io::stdout(), "{value}")?;
    } else {
        writeln!(std::io::stdout(), "{text}")?;
    }
    Ok(())
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    work: String,
    #[arg(long, required_unless_present = "status")]
    session: Option<Id>,
    #[arg(long, required_unless_present = "status", value_parser = ["baseline", "acceptance", "mutation"])]
    phase: Option<String>,
    #[arg(long)]
    example: Vec<String>,
    /// Override each selected check's timeout in seconds.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=86400))]
    timeout: Option<u32>,
    /// Write generated completion input, when all required acceptance checks pass.
    #[arg(long)]
    write_input: Option<PathBuf>,
    /// Exit with status 1 if any selected check misses the phase expectation.
    #[arg(long)]
    require_met: bool,
    #[arg(long, required_unless_present = "status")]
    expected_revision: Option<Revision>,
    /// Read recorded examples and runs without refreshing sources or starting a child.
    #[arg(long, conflicts_with_all = ["session", "phase", "example", "timeout", "write_input", "require_met", "expected_revision"])]
    status: bool,
}

pub fn run(root: &Path, args: &CheckArgs, json_output: bool) -> Result<()> {
    if args.status {
        let db = RuntimeProject::open(root, false)?;
        let value = check_status(&db.store, &db.project.root, &args.work)?;
        let mut lines = Vec::new();
        for e in value["examples"].as_array().unwrap() {
            lines.push(format!(
                "{:<32} {} ({})",
                e["id"].as_str().unwrap_or("unidentified"),
                if e["has_check"] == true {
                    "check bound"
                } else {
                    "no check"
                },
                e["runs"].as_array().unwrap().len()
            ));
        }
        lines.push(format!("Next: {}", value["next_action"].as_str().unwrap()));
        return print(&value, &lines.join("\n"), json_output);
    }
    let expected = args.expected_revision.unwrap();
    let mut db = RuntimeProject::for_write(root, expected)?;
    let phase: CheckPhase = serde_json::from_value(serde_json::json!(args.phase))?;
    let value = run_check(
        &mut db.store,
        &db.project.root,
        &RunCheckRequest {
            work: args.work.clone(),
            session: args.session.unwrap(),
            phase,
            examples: args.example.clone(),
            timeout_seconds: args.timeout,
            expected_revision: expected,
        },
    )?;
    if let Some(path) = &args.write_input {
        if !value["completion_input"].is_null() {
            fs::write(path, serde_json::to_vec_pretty(&value["completion_input"])?)?;
        }
    }
    let mut lines = Vec::new();
    for result in value["results"].as_array().unwrap() {
        let exit = result["exit_code"]
            .as_i64()
            .map(|c| format!(" (exit {c})"))
            .unwrap_or_default();
        let expected_failure = if phase == CheckPhase::Baseline && result["outcome"] == "failed" {
            " as expected before the change"
        } else {
            ""
        };
        lines.push(format!(
            "{:<32} {:<10} {}{exit}{expected_failure}",
            result["example"].as_str().unwrap(),
            phase.as_str(),
            result["outcome"].as_str().unwrap()
        ));
    }
    lines.push(format!("Next: {}", value["next_action"].as_str().unwrap()));
    print(&value, &lines.join("\n"), json_output)?;
    if args.require_met && value["met"] != true {
        return Err(Error::RuleViolation(
            "selected checks did not meet the phase expectation".into(),
        ));
    }
    Ok(())
}
