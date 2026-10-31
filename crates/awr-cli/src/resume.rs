use awr_core::{Error, Id, Result, ResumeClaim, Revision};
use awr_runtime::{ResumeRequest, resume_session};
use clap::Args;
use std::path::Path;

#[derive(Debug, Args)]
pub struct ResumeArgs {
    #[arg(long, visible_alias = "session")]
    from_session: Option<Id>,
    #[arg(long)]
    work: Option<String>,
    #[arg(long)]
    agent: String,
    #[arg(long)]
    provider: String,
    #[arg(long)]
    model: String,
    /// Explicitly acquire a new claim, even when the predecessor has none.
    #[arg(long, conflicts_with = "no_claim")]
    claim: bool,
    /// Release predecessor claims without inheriting one; default preserves a still-live claim.
    #[arg(long)]
    no_claim: bool,
    /// Without it the new claim lives for the project's default lifetime, when `.awr/policy.toml`
    /// (or AWR_DEFAULT_CLAIM_TTL) declares one, and otherwise has no expiry.
    #[arg(long, requires = "claim")]
    ttl_ms: Option<u64>,
    /// The new claim has no expiry, whatever the project's default lifetime is.
    #[arg(long, requires = "claim", conflicts_with = "ttl_ms")]
    no_expiry: bool,
    #[arg(long)]
    expected_revision: Revision,
    #[arg(long, default_value_t = 5000)]
    budget: usize,
    #[arg(long)]
    path: Option<Vec<String>>,
    #[arg(long)]
    tag: Option<Vec<String>>,
    #[arg(long)]
    goal: Vec<String>,
    #[arg(long)]
    source_sha: Option<String>,
}

pub fn run(root: &Path, args: &ResumeArgs, json_output: bool) -> Result<()> {
    let mut db = crate::session::RuntimeProject::open(root, false)?;
    let report = resume_session(
        &mut db.store,
        root,
        &ResumeRequest {
            from_session_id: args.from_session,
            work_item_key: args.work.clone(),
            agent_id: args.agent.clone(),
            provider: args.provider.clone(),
            model: args.model.clone(),
            claim: if args.claim {
                ResumeClaim::Acquire
            } else if args.no_claim {
                ResumeClaim::None
            } else {
                ResumeClaim::Inherit
            },
            claim_ttl_ms: if args.claim {
                crate::session::claim_ttl(root, args.ttl_ms, args.no_expiry)?
            } else {
                None
            },
            expected_revision: args.expected_revision,
            token_budget: args.budget,
            paths: args.path.clone(),
            tags: args.tag.clone(),
            goal_keys: args.goal.clone(),
            source_sha: args.source_sha.clone(),
        },
    )?;
    if json_output {
        let mut value = serde_json::to_value(&report)?;
        if let Some(context) = &report.context {
            value["context"] = crate::context::l1_value(context)?;
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "Resume: {}\nFrom: {}\nCheckpoint: {}\nContext ready: {}",
            report
                .resumed
                .as_ref()
                .map(|r| r.session.id.to_string())
                .unwrap_or_else(|| "not created; preflight incomplete".into()),
            report.from_session_id,
            report
                .checkpoint_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none; source/session-start recovery".into()),
            report.context_ready
        );
        if let Some(resumed) = &report.resumed {
            println!(
                "Agent: {} | Provider: {} | Model: {}\nClaim: {}",
                resumed.session.agent_id,
                resumed.session.provider,
                resumed.session.model,
                resumed
                    .claim
                    .as_ref()
                    .map(|c| c.id.to_string())
                    .unwrap_or_else(|| "none".into())
            );
        }
        for gap in &report.recovery_gaps {
            println!("Recovery gap: {gap}");
        }
        print!(
            "{}",
            awr_runtime::render_execution_observations(&report.executions)?
        );
        if let Some(context) = &report.context {
            print!("{}", context.rendered_context());
        }
        if let Some(error) = &report.context_error {
            println!("Context error: {}: {}", error.code, error.message);
        }
    }
    if !report.context_ready {
        let reason = if let Some(resumed) = &report.resumed {
            format!(
                "successor session {} was created; use context compile --session {} before execution instead of repeating resume",
                resumed.session.id, resumed.session.id
            )
        } else {
            "resume preflight is incomplete; no successor session was created".into()
        };
        return Err(Error::ContextIncomplete(reason));
    }
    Ok(())
}
