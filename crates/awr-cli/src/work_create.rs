use awr_core::*;
use awr_runtime::{CreateWorkInput, CreationReport};
use awr_store::Store;
use clap::Args;
use std::path::{Path, PathBuf};

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Protected JSON with version, request_key, title and optional source_id.
    #[arg(long, conflicts_with_all=["title","request_key","source","acceptance","goal","depends_on","path","tag","kind","priority","summary","next_action","spec_ref","owner","milestone"])]
    input: Option<PathBuf>,
    #[arg(long, required_unless_present = "input")]
    title: Option<String>,
    /// Stable host request ID, reused only with identical creation content.
    #[arg(long, required_unless_present = "input")]
    request_key: Option<String>,
    #[arg(long)]
    source: Option<Id>,
    #[arg(long)]
    acceptance: Vec<String>,
    #[arg(long)]
    goal: Vec<String>,
    #[arg(long)]
    depends_on: Vec<String>,
    #[arg(long)]
    path: Vec<String>,
    #[arg(long)]
    tag: Vec<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    priority: Option<String>,
    #[arg(long)]
    summary: Option<String>,
    #[arg(long)]
    next_action: Option<String>,
    /// One specification reference as a JSON object; repeat for each reference.
    #[arg(long, value_parser = parse_spec_ref)]
    spec_ref: Vec<SpecificationReference>,
    #[arg(long)]
    owner: Option<String>,
    #[arg(long)]
    milestone: Option<String>,
    /// Attempt activation after creation; supply acceptance, a goal and --next-action (or fields.next_action in --input).
    #[arg(long)]
    activate: bool,
    #[arg(long)]
    accept: bool,
    /// Include the complete creation preview payload (JSON); preview only.
    #[arg(long, conflicts_with = "accept")]
    full: bool,
    #[arg(long)]
    expected_preview: Option<String>,
    #[arg(long)]
    expected_revision: Option<Revision>,
}
#[derive(Debug, Args)]
pub struct StatusArgs {
    #[arg(long, visible_alias = "request-key")]
    key: String,
}
#[derive(Debug, Args)]
pub struct RecoverArgs {
    #[arg(long, visible_alias = "request-key")]
    key: String,
    #[arg(long)]
    expected_revision: Revision,
}
fn output(report: CreationReport, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report.value)?);
    } else if report.value["status"] == "preview" {
        let request = &report.value["preview"]["plan"]["request"];
        let goals = request["fields"]
            .get("goal")
            .map(serde_json::Value::to_string)
            .unwrap_or_else(|| "none".into());
        println!(
            "Work creation: preview\nWork key: {}\nTitle: {}\nGoals: {goals}\nAcceptance: {}\nActivation requested: {}\nNext: {}",
            report.value["preview"]["plan"]["external_key"]
                .as_str()
                .unwrap(),
            request["title"].as_str().unwrap(),
            request["fields"]["acceptance"]
                .as_array()
                .map_or(0, Vec::len),
            report.value["activation_requested"],
            report.value["next_action"].as_str().unwrap()
        );
    } else {
        println!(
            "Work creation: {}\nWork key: {}\nReceipt: {}",
            report.value["status"],
            report.value["external_key"],
            report.value["recovery_directory"]
        );
        if let Some(activation) = report.value.get("activation").filter(|v| !v.is_null()) {
            println!("Activation: {}", activation["status"]);
            if let Some(message) = activation["error"]["message"].as_str() {
                println!("Reason: {message}");
            }
            if let Some(next) = crate::help::definition_error_next_action(&activation["error"])
                .or_else(|| activation["next_action"].as_str())
            {
                println!("Next: {next}");
            }
        }
    }
    match report.failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
pub fn create(root: &Path, args: &CreateArgs, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let input = if let Some(path) = &args.input {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        serde_json::from_slice(&awr_source::read_capped(&path, 64 * 1024)?).map_err(|_| {
            Error::InvalidInput(
                "creation JSON requires version, request_key, title and optional source_id".into(),
            )
        })?
    } else {
        CreateWorkInput {
            version: 1,
            request_key: args.request_key.clone().unwrap(),
            title: args.title.clone().unwrap(),
            source_id: args.source,
            fields: args.fields()?,
        }
    };
    let mut store =
        Store::open_existing(&crate::source::runtime_dir(&root, false)?.join("state.db"))?;
    let report = awr_runtime::create_work_with_activation(
        &mut store,
        &root,
        input.clone(),
        args.accept,
        args.expected_preview.as_deref(),
        args.expected_revision,
        args.activate,
    );
    match report {
        Ok(mut report) => {
            if report.value["status"] == "preview" {
                report.value["next_action"] = serde_json::json!(args.command(
                    &root,
                    &input,
                    Some((
                        report.value["preview"]["fingerprint"].as_str().unwrap(),
                        report.value["project_revision"].as_u64().unwrap()
                    ))
                )?);
                if !args.full {
                    compact_preview(&mut report.value)?;
                }
            }
            output(report, json_output)
        }
        Err(error) => Err(args.recovery_error(&root, &input, error)?),
    }
}

// Compact only the CLI view; runtime plans, fingerprints and MCP previews stay intact.
fn compact_preview(value: &mut serde_json::Value) -> Result<()> {
    let preview = &mut value["preview"];
    for field in ["before_text", "after_text", "new_record"] {
        let bytes = match preview[field].as_str() {
            Some(text) => text.len(),
            None => serde_json::to_vec(&preview[field])?.len(),
        };
        preview[field] = serde_json::json!({"bytes": bytes, "omitted": true});
    }
    let plan = preview["plan"].as_object_mut().unwrap();
    let count = plan["observations"].as_array().map_or(0, Vec::len);
    plan.retain(|key, _| matches!(key.as_str(), "request" | "external_key"));
    plan.insert(
        "observations".into(),
        serde_json::json!({"count": count, "omitted": true}),
    );
    Ok(())
}
pub fn status(root: &Path, args: &StatusArgs, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let store = Store::open_readonly(&crate::source::runtime_dir(&root, false)?.join("state.db"))?;
    output(
        awr_runtime::creation_status(&store, &root, &args.key)?,
        json_output,
    )
}
pub fn recover(root: &Path, args: &RecoverArgs, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let mut store =
        Store::open_existing(&crate::source::runtime_dir(&root, false)?.join("state.db"))?;
    output(
        awr_runtime::recover_creation(&mut store, &root, &args.key, args.expected_revision)?,
        json_output,
    )
}

fn parse_spec_ref(text: &str) -> std::result::Result<SpecificationReference, String> {
    serde_json::from_str(text)
        .map_err(|_| "--spec-ref requires a specification reference JSON object".into())
}

impl CreateArgs {
    fn fields(&self) -> Result<serde_json::Map<String, serde_json::Value>> {
        let mut fields = serde_json::Map::new();
        for (key, values) in [
            ("acceptance", &self.acceptance),
            ("goal", &self.goal),
            ("depends_on", &self.depends_on),
            ("paths", &self.path),
            ("tags", &self.tag),
        ] {
            if !values.is_empty() {
                fields.insert(key.into(), serde_json::json!(values));
            }
        }
        for (key, value) in [
            ("kind", &self.kind),
            ("priority", &self.priority),
            ("summary", &self.summary),
            ("next_action", &self.next_action),
            ("owner", &self.owner),
            ("milestone", &self.milestone),
        ] {
            if let Some(value) = value {
                fields.insert(key.into(), serde_json::json!(value));
            }
        }
        if !self.spec_ref.is_empty() {
            fields.insert("spec_refs".into(), serde_json::to_value(&self.spec_ref)?);
        }
        Ok(fields)
    }

    fn recovery_error(&self, root: &Path, input: &CreateWorkInput, error: Error) -> Result<Error> {
        let report = error.report();
        if !matches!(
            report.code,
            "RevisionConflict" | "SourceStale" | "SourceConflict"
        ) {
            return Ok(error);
        }
        let valid = report
            .details
            .as_ref()
            .is_some_and(|d| d["preview_still_valid"] == true);
        let acceptance = if valid {
            self.expected_preview.as_deref().zip(
                report
                    .details
                    .as_ref()
                    .and_then(|d| d["current_revision"].as_u64()),
            )
        } else {
            None
        };
        Ok(error.with_details(
            serde_json::json!({"next_action":self.command(root, input, acceptance)?}),
        ))
    }

    fn command(
        &self,
        root: &Path,
        input: &CreateWorkInput,
        acceptance: Option<(&str, Revision)>,
    ) -> Result<String> {
        let mut args = vec!["awr".to_owned(), "work".into(), "create".into()];
        if let Some(path) = &self.input {
            args.extend(["--input".into(), path.to_string_lossy().into_owned()]);
        } else {
            args.extend([
                "--title".into(),
                self.title.clone().unwrap(),
                "--request-key".into(),
                self.request_key.clone().unwrap(),
            ]);
            if let Some(source) = self.source {
                args.extend(["--source".into(), source.to_string()]);
            }
            for (name, values) in [
                ("--acceptance", &self.acceptance),
                ("--goal", &self.goal),
                ("--depends-on", &self.depends_on),
                ("--path", &self.path),
                ("--tag", &self.tag),
            ] {
                for v in values {
                    args.extend([name.into(), v.clone()]);
                }
            }
            for (name, value) in [
                ("--kind", &self.kind),
                ("--priority", &self.priority),
                ("--summary", &self.summary),
                ("--next-action", &self.next_action),
                ("--owner", &self.owner),
                ("--milestone", &self.milestone),
            ] {
                if let Some(v) = value {
                    args.extend([name.into(), v.clone()]);
                }
            }
            for spec in &self.spec_ref {
                args.extend([
                    "--spec-ref".into(),
                    serde_json::to_string(spec).expect("specification reference"),
                ]);
            }
        }
        if self.activate {
            args.push("--activate".into());
        }
        if let Some((preview, revision)) = acceptance {
            args.extend([
                "--accept".into(),
                "--expected-preview".into(),
                preview.into(),
                "--expected-revision".into(),
                revision.to_string(),
            ]);
        }
        let mut command = cli_next_action(&args.iter().map(String::as_str).collect::<Vec<_>>());
        if command.len() > 300 || command.chars().any(char::is_control) {
            let path = awr_runtime::save_cli_recovery_input(root, input)?;
            let mut compact = vec![
                "awr".to_owned(),
                "work".into(),
                "create".into(),
                "--input".into(),
                path,
            ];
            if self.activate {
                compact.push("--activate".into());
            }
            if let Some((preview, revision)) = acceptance {
                compact.extend([
                    "--accept".into(),
                    "--expected-preview".into(),
                    preview.into(),
                    "--expected-revision".into(),
                    revision.to_string(),
                ]);
            }
            command = cli_next_action(&compact.iter().map(String::as_str).collect::<Vec<_>>());
        }
        Ok(command)
    }
}
