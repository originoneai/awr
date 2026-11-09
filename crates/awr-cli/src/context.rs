use awr_context::{
    BootstrapRequest, ContextRequest, DeltaBaseline, DeltaContextRequest, DeltaRequest, bootstrap,
    compile_context, context_delta,
};
use awr_core::{Error, Id, Result};
use clap::Subcommand;
use std::path::Path;

#[derive(Debug, Subcommand)]
pub enum ContextCommand {
    /// Refresh sources and summarize changes since a checkpoint, revision or selected session.
    Delta {
        #[arg(long)]
        work: Option<String>,
        #[arg(long)]
        session: Option<Id>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, conflicts_with = "after_revision")]
        checkpoint: Option<Id>,
        #[arg(long, conflicts_with = "checkpoint")]
        after_revision: Option<u64>,
        #[arg(long, default_value_t = 12)]
        event_limit: usize,
        #[arg(long, default_value_t = 24)]
        entity_limit: usize,
    },
    /// Compile current work, hard facts, required context and explicit gaps within a token budget.
    Compile {
        #[arg(long)]
        work: Option<String>,
        #[arg(long)]
        session: Option<Id>,
        /// Compile without selecting an active runtime session.
        #[arg(long)]
        detached: bool,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        branch: Option<Id>,
        #[arg(long)]
        goal: Vec<String>,
        #[arg(long)]
        path: Option<Vec<String>>,
        #[arg(long)]
        tag: Option<Vec<String>>,
        #[arg(long)]
        source_sha: Option<String>,
        #[arg(long, default_value = "work")]
        intent: String,
        /// Default 5000; raise to required facts up to 10000 when omitted.
        #[arg(long)]
        budget: Option<usize>,
        #[arg(long, conflicts_with = "after_revision")]
        checkpoint: Option<Id>,
        #[arg(long, conflicts_with = "checkpoint")]
        after_revision: Option<u64>,
    },
    /// Restore current work, critical rules and last checkpoint; compile L1 before execution.
    Bootstrap {
        #[arg(long)]
        work: Option<String>,
        #[arg(long)]
        session: Option<Id>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value_t = 1000)]
        budget: usize,
    },
}

pub fn run(root: &Path, command: &ContextCommand, json_output: bool) -> Result<()> {
    let mut db = crate::session::RuntimeProject::open(root, false)?;
    match command {
        ContextCommand::Delta {
            work,
            session,
            agent,
            checkpoint,
            after_revision,
            event_limit,
            entity_limit,
        } => {
            let report = context_delta(
                &mut db.store,
                root,
                &DeltaContextRequest {
                    work_item_key: work.clone(),
                    agent_id: agent.clone(),
                    delta: DeltaRequest {
                        session_id: *session,
                        event_limit: *event_limit,
                        entity_limit_per_source: *entity_limit,
                        baseline: if let Some(id) = checkpoint {
                            DeltaBaseline::Checkpoint { id: *id }
                        } else if let Some(revision) = after_revision {
                            DeltaBaseline::Revision {
                                revision: *revision,
                            }
                        } else {
                            DeltaBaseline::Auto
                        },
                    },
                },
            )?;
            if json_output {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                // This bounded structured delta contains summaries and references, never event bodies.
                println!("{}", serde_json::to_string_pretty(&report)?);
                println!(
                    "Drill down with source history <source-id>, event show <event-id> --full, or object show <kind> <id> --full."
                );
            }
            if !report.source_refresh_ok {
                return Err(Error::SourceStale(
                    "delta contains source refresh issues; inspect source_issues".into(),
                ));
            }
            Ok(())
        }
        ContextCommand::Compile {
            work,
            session,
            detached,
            agent,
            branch,
            goal,
            path,
            tag,
            source_sha,
            intent,
            budget,
            checkpoint,
            after_revision,
        } => {
            let mut request = ContextRequest {
                work_item_key: work.clone(),
                session_id: *session,
                detached: *detached,
                agent_id: agent.clone(),
                branch_id: *branch,
                goal_keys: goal.clone(),
                paths: path.clone(),
                tags: tag.clone(),
                source_sha: source_sha.clone(),
                intent: intent.clone(),
                token_budget: budget.unwrap_or(5000),
                delta_baseline: if let Some(id) = checkpoint {
                    DeltaBaseline::Checkpoint { id: *id }
                } else if let Some(revision) = after_revision {
                    DeltaBaseline::Revision {
                        revision: *revision,
                    }
                } else {
                    DeltaBaseline::Auto
                },
            };
            let mut basis = if budget.is_some() {
                "explicit"
            } else {
                "default"
            };
            let result = match compile_context(&mut db.store, root, &request) {
                Err(Error::BudgetExceeded { required, .. })
                    if budget.is_none() && required <= 10000 =>
                {
                    request.token_budget = required;
                    basis = "raised_to_required";
                    compile_context(&mut db.store, root, &request)
                }
                result => result,
            };
            let report = result.map_err(|error| {
                if let Error::BudgetExceeded { required, .. } = &error {
                    let mut args = vec!["awr".to_owned(), "context".into(), "compile".into()];
                    for (name, value) in [("--work",work.clone()),("--session",session.map(|id|id.to_string())),
                        ("--agent",agent.clone()),("--branch",branch.map(|id|id.to_string())),
                        ("--source-sha",source_sha.clone()),("--checkpoint",checkpoint.map(|id|id.to_string())),
                        ("--after-revision",after_revision.map(|r|r.to_string()))] {
                        if let Some(v) = value { args.extend([name.into(),v]); }
                    }
                    if *detached { args.push("--detached".into()); }
                    for (name, values) in [("--goal",goal.as_slice()),("--path",path.as_deref().unwrap_or_default()),("--tag",tag.as_deref().unwrap_or_default())] {
                        for v in values { args.extend([name.into(),v.clone()]); }
                    }
                    args.extend(["--intent".into(),intent.clone(),"--budget".into(),required.to_string()]);
                    error.with_details(serde_json::json!({"next_action":awr_core::cli_next_action(&args.iter().map(String::as_str).collect::<Vec<_>>())}))
                } else { error }
            })?;
            print_l1_with_budget(
                &report,
                db.store.project(db.project.id)?.project_revision,
                json_output,
                Some(serde_json::json!({"applied":request.token_budget,"basis":basis})),
            )
        }

        ContextCommand::Bootstrap {
            work,
            session,
            agent,
            budget,
        } => {
            let pack = bootstrap(
                &mut db.store,
                root,
                &BootstrapRequest {
                    work_item_key: work.clone(),
                    session_id: *session,
                    agent_id: agent.clone(),
                    token_budget: *budget,
                },
            )?;
            if json_output {
                println!("{}", serde_json::to_string_pretty(&pack)?);
            } else {
                print!("{}", pack.rendered_context);
            }
            if !pack.context.complete {
                return Err(Error::ContextIncomplete("bootstrap contains explicit gaps; inspect the returned context before proceeding".into()));
            }
            Ok(())
        }
    }
}

/// Every CLI surface embedding a compiled L1 report uses the same transport metadata.
pub(crate) fn l1_value(report: &awr_context::WorkContextReport) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(report)?;
    value["ok"] = serde_json::json!(report.completeness.complete);
    value["project_revision"] = serde_json::json!(report.completeness.project_revision);
    value["freshness_basis"] = serde_json::json!("source_refresh");
    value["source_refresh_performed"] = serde_json::json!(true);
    value["read_only"] = serde_json::json!(false);
    if !report.completeness.complete {
        value["error"] = serde_json::json!(
            Error::ContextIncomplete(
                "L1 has required gaps; inspect completeness before execution".into()
            )
            .report()
        );
    }
    Ok(value)
}

pub(crate) fn print_l1(
    report: &awr_context::WorkContextReport,
    revision: awr_core::Revision,
    json_output: bool,
) -> Result<()> {
    print_l1_with_budget(report, revision, json_output, None)
}

fn print_l1_with_budget(
    report: &awr_context::WorkContextReport,
    revision: awr_core::Revision,
    json_output: bool,
    budget: Option<serde_json::Value>,
) -> Result<()> {
    if revision != report.completeness.project_revision {
        return Err(Error::RevisionConflict {
            expected: report.completeness.project_revision,
            actual: revision,
        });
    }
    let error = (!report.completeness.complete).then(|| {
        Error::ContextIncomplete(
            "L1 has required gaps; inspect completeness before execution".into(),
        )
    });
    if json_output {
        let mut value = l1_value(report)?;
        if let Some(budget) = budget {
            value["budget"] = budget;
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        print!("{}", report.rendered_context());
        if let Some(budget) = budget.filter(|v| v["basis"] == "raised_to_required") {
            eprintln!("Budget: {} (raised_to_required)", budget["applied"]);
        }
    }
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
