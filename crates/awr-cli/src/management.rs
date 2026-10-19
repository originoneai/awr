use awr_core::*;
use awr_runtime::AttachExplanationOptions;
use clap::Args;
use std::path::{Path, PathBuf};
#[derive(Debug, Args)]
pub struct AssessArgs {
    work: String,
    #[arg(long)]
    branch: Option<String>,
    /// Opt-in assessment explanation (capability `assessment.explain`).
    #[arg(long, default_value_t = false)]
    explain: bool,
}
#[derive(Debug, Args)]
pub struct ManageArgs {
    #[arg(long)]
    input: PathBuf,
}
pub fn assess(root: &Path, args: &AssessArgs) -> Result<()> {
    let query = crate::query::QueryProject::open_read(root, false)?;
    query.finish()?;
    let mut value = awr_runtime::assess_management(
        &query.store,
        root,
        &awr_runtime::AssessManagementRequest {
            work: args.work.clone(),
            branch: args.branch.clone(),
        },
    )?;
    query.check_revision()?;
    for (k, v) in query.metadata().as_object().unwrap() {
        value[k] = v.clone();
    }
    value = awr_runtime::attach_assessment_explanation(
        value,
        &AttachExplanationOptions {
            enabled: args.explain,
            ..Default::default()
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
pub fn manage(root: &Path, args: &ManageArgs) -> Result<()> {
    let input = if args.input.is_absolute() {
        args.input.clone()
    } else {
        root.join(&args.input)
    };
    let request=serde_json::from_slice(&awr_source::read_capped(&input,64*1024)?).map_err(|_|Error::InvalidInput("management input requires work, session, expected_revision, request_key, contract_fingerprint and observation".into()))?;
    let mut store = awr_store::Store::open_existing(
        &crate::source::runtime_dir(root, false)?.join("state.db"),
    )?;
    let value = awr_runtime::manage_work(&mut store, root, &request)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
