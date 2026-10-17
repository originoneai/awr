use crate::query::{QueryProject, WorkCommand, short};
use awr_core::*;
use awr_runtime::{BranchFilter, EventCursor, EventQuery, Runtime};
use awr_store::Store;
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, Args)]
pub struct Selection {
    /// Explicit session ID. Required when more than one active candidate matches.
    #[arg(long)]
    session: Option<Id>,
    #[arg(long)]
    /// Caller-declared agent; checkpoint records this separately from the session label.
    agent: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Outcome {
    Ended,
    Interrupted,
    Incomplete,
}
impl From<Outcome> for SessionOutcome {
    fn from(value: Outcome) -> Self {
        match value {
            Outcome::Ended => Self::Ended,
            Outcome::Interrupted => Self::Interrupted,
            Outcome::Incomplete => Self::Incomplete,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// Observe native compaction and assess continuity without opening a session.
    #[command(subcommand)]
    Compaction(crate::compaction::CompactionCommand),
    /// Refresh current facts and continue work in a new agent/provider/model session.
    Resume(crate::resume::ResumeArgs),
    /// Refresh sources and start a session at the supplied project revision.
    ///
    /// JSON output (`--json`): one object with `ok`, `project_revision`,
    /// `session`, `claim` and `event`.
    /// - `session.id` is the AWR session id to reuse in later `--session`
    ///   options (e.g. `jq -er '.session.id'`); it is not the native chat id.
    /// - `claim` is the acquired claim or `null` without `--claim`; it carries
    ///   `id`, `work_item_id`, `agent_id`, `session_id`, `status`.
    /// - The claim is runtime ownership only; it does not rewrite source
    ///   work status.
    Start {
        #[arg(long)]
        work: Option<String>,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: String,
        #[arg(long)]
        claim: bool,
        #[arg(long, requires = "claim")]
        ttl_ms: Option<u64>,
        /// Caller-supplied MCP namespace for `--conversation`: use `stdio` for local `awr-mcp`, or the registry client ID for shared HTTP. This does not authenticate the caller and is distinct from `client bind --client` hook dialects.
        #[arg(long, requires = "conversation")]
        client: Option<String>,
        /// Stable host conversation ID for the MCP binding. It is distinct from
        /// local `client bind` continuity until that command uses `--session`.
        #[arg(long, requires = "client")]
        conversation: Option<String>,
        #[arg(long)]
        expected_revision: Revision,
    },
    List {
        #[arg(long)]
        active: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show the session, its checkpoint, inherited handoff and claims.
    Show { id: Id },
    /// Save observed session/source changes with caller-supplied progress and last-used context hash.
    Checkpoint {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        work: Option<String>,
        /// 64 hexadecimal characters without a sha256: prefix. Copy work_context.context_hash
        /// from the last consumed work prepare or context compile result, not a source fingerprint.
        #[arg(long)]
        context_hash: String,
        /// Nonblank progress summary for the successor; this is text, not a cryptographic digest.
        #[arg(long)]
        digest: String,
        /// Nonblank next action for continuing the work.
        #[arg(long)]
        next_action: String,
        /// Unfinished item to retain; repeat for multiple nonblank entries.
        #[arg(long)]
        open_loop: Vec<String>,
        #[arg(long)]
        /// Additional caller-reported references; kept separate from observed changes.
        changed_entity: Vec<String>,
        /// Expected project revision from session show, not session.revision.
        #[arg(
            long = "expected-project-revision",
            visible_alias = "expected-revision"
        )]
        expected_revision: Revision,
    },
    /// Close the session and release all of its active claims, even if sources are unavailable.
    End {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        work: Option<String>,
        #[arg(long, value_enum, default_value_t=Outcome::Ended)]
        outcome: Outcome,
        #[arg(long)]
        expected_revision: Revision,
    },
}

#[derive(Debug, Args)]
pub struct ClaimArgs {
    work: String,
    #[command(flatten)]
    selection: Selection,
    #[arg(long)]
    ttl_ms: Option<u64>,
    #[arg(long)]
    expected_revision: Revision,
}
#[derive(Debug, Args)]
pub struct ReleaseArgs {
    work: String,
    #[command(flatten)]
    selection: Selection,
    #[arg(long)]
    claim: Id,
    #[arg(long)]
    expected_revision: Revision,
}
#[derive(Debug, Args)]
pub struct HandoffArgs {
    work: String,
    #[command(flatten)]
    selection: Selection,
    #[arg(long)]
    to_session: Option<Id>,
    #[arg(long, requires = "to_session")]
    ttl_ms: Option<u64>,
    #[arg(long)]
    expected_revision: Revision,
}
#[derive(Debug, Args)]
pub struct HistoryArgs {
    work: String,
    #[arg(long)]
    session: Option<Id>,
    #[arg(long)]
    all_branches: bool,
    #[arg(long)]
    event_type: Option<String>,
    #[arg(long, default_value_t = 0)]
    after_revision: Revision,
    /// JSON next_cursor from the previous history response.
    #[arg(long)]
    cursor: Option<String>,
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

pub(crate) struct RuntimeProject {
    pub store: Store,
    pub project: Project,
    refreshed: bool,
}
impl RuntimeProject {
    /// Check the caller's revision both before and after source refresh, as MCP writes do.
    pub fn for_write(root: &Path, expected: Revision) -> Result<Self> {
        let root = root.canonicalize()?;
        let database = crate::source::runtime_dir(&root, false)?.join("state.db");
        let mut store = Store::open_existing(&database)?;
        let mut project = store.project_by_root(&root)?;
        if project.project_revision != expected {
            return Err(Error::RevisionConflict {
                expected,
                actual: project.project_revision,
            });
        }
        let refresh = awr_source::index_project(
            &mut store,
            &root,
            &awr_source::Manifest::load(&root)?,
            false,
        )?;
        if !refresh.ok || refresh.pending != 0 {
            return Err(Error::SourceStale(
                "source refresh is incomplete; inspect awr source reindex".into(),
            ));
        }
        project = store.project(project.id)?;
        if project.project_revision != expected {
            return Err(Error::RevisionConflict {
                expected,
                actual: project.project_revision,
            });
        }
        Ok(Self {
            store,
            project,
            refreshed: true,
        })
    }

    pub fn open(root: &Path, refresh: bool) -> Result<Self> {
        if refresh {
            let query = QueryProject::open(root)?;
            query.finish()?;
            return Ok(Self {
                store: query.store,
                project: query.project,
                refreshed: true,
            });
        }
        let root = root.canonicalize()?;
        let database = crate::source::runtime_dir(&root, false)?.join("state.db");
        if !database.is_file() {
            return Err(Error::NotFound(
                "AWR database; initialize this project first".into(),
            ));
        }
        let store = Store::open(&database)?;
        let project = store.project_by_root(&root)?;
        Ok(Self {
            store,
            project,
            refreshed: false,
        })
    }
    pub fn metadata(&self, revision: Revision) -> Value {
        json!({"ok":true,"project_revision":revision,"source_refresh_performed":self.refreshed,"freshness_basis":if self.refreshed {"source_refresh"} else {"runtime_database"}})
    }
    pub fn check_revision(&self) -> Result<()> {
        let actual = self.store.project(self.project.id)?.project_revision;
        if actual != self.project.project_revision {
            return Err(Error::RevisionConflict {
                expected: self.project.project_revision,
                actual,
            });
        }
        Ok(())
    }
    fn selected(&self, selection: &Selection, work: Option<&str>) -> Result<Session> {
        let work = work
            .map(|key| self.store.work_identity(self.project.id, key))
            .transpose()?;
        self.store.select_active_session(
            self.project.id,
            selection.session,
            work,
            selection.agent.as_deref(),
            self.project.current_branch_id,
        )
    }
}

pub(crate) fn event_brief(event: &Event) -> Value {
    json!({"id":event.id,"type":event.event_type,"summary":short(&event.summary),"importance":event.importance,"session_id":event.session_id,"branch_id":event.branch_id,"project_revision":event.project_revision,"created_at":event.created_at,
        "work_item_id":event.work_item_id,"source_id":event.payload.get("source_id").and_then(|v|v.as_str()).and_then(|s|s.parse::<Id>().ok()),
        "checkpoint_id":event.payload.get("checkpoint_id").and_then(|v|v.as_str()).and_then(|s|s.parse::<Id>().ok()),"artifact_id":event.payload.get("artifact_id").and_then(|v|v.as_str()).and_then(|s|s.parse::<Id>().ok())})
}
fn print(value: &Value, text: &str, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{text}");
    }
    Ok(())
}

pub fn run(root: &Path, command: &SessionCommand, json_output: bool) -> Result<()> {
    if let SessionCommand::Compaction(command) = command {
        return crate::compaction::run(root, command, json_output);
    }
    if let SessionCommand::Resume(args) = command {
        return crate::resume::run(root, args, json_output);
    }
    let refresh = matches!(
        command,
        SessionCommand::Start { .. } | SessionCommand::Checkpoint { .. }
    );
    let mut db = RuntimeProject::open(root, refresh)?;
    let project = db.project.id;
    match command {
        SessionCommand::Resume(_) | SessionCommand::Compaction(_) => unreachable!(),
        SessionCommand::Start {
            work,
            agent,
            provider,
            model,
            claim,
            ttl_ms,
            client,
            conversation,
            expected_revision,
        } => {
            let binding = match (client, conversation) {
                (Some(client), Some(conversation)) => {
                    let binding = McpSessionBinding {
                        client: client.clone(),
                        conversation: conversation.clone(),
                    };
                    binding.validate()?;
                    Some(binding)
                }
                _ => None,
            };
            let draft = SessionDraft {
                work_item_key: work.clone(),
                agent_id: agent.clone(),
                provider: provider.clone(),
                model: model.clone(),
                branch_id: db.project.current_branch_id,
                claim: *claim,
                claim_ttl_ms: *ttl_ms,
            };
            // With a binding, reuse an existing conversation session exactly
            // like the MCP channel does, so both surfaces resolve the same
            // session instead of splitting host bindings.
            if let Some(binding) = &binding {
                if let Some(session) = db.store.mcp_bound_session(project, binding)? {
                    let work_matches = match (&session.work_item_id, work.as_deref()) {
                        (_, None) => true,
                        // Propagate lookup failures instead of misreporting a
                        // store error as an identity conflict; a missing work
                        // key is a NotFound, which is accurate on its own.
                        (Some(id), Some(key)) => db.store.work_identity(project, key)? == *id,
                        (None, Some(_)) => false,
                    };
                    if !work_matches
                        || session.agent_id != *agent
                        || session.provider != *provider
                        || session.model != *model
                        || session.branch_id != db.project.current_branch_id
                    {
                        return Err(Error::SourceConflict(
                            "conversation already belongs to different work or identity".into(),
                        ));
                    }
                    db.check_revision()?;
                    // A reused binding must not report a usable session that
                    // is already closed; the repair is to resume it.
                    if session.status != "active" {
                        return Err(Error::InvalidTransition(format!(
                            "conversation binding resolves session {} that is {}; resume the session or start a new conversation",
                            session.id, session.status
                        )));
                    }
                    // The same command with a new binding rejects a stale
                    // --expected-revision inside the transaction; reuse must
                    // enforce it too instead of silently accepting it.
                    if *expected_revision != db.project.project_revision {
                        return Err(Error::RevisionConflict {
                            expected: *expected_revision,
                            actual: db.project.project_revision,
                        });
                    }
                    // A caller scripting `session start --claim` must not
                    // silently lose the claim on the reuse path: acquire it
                    // on the resolved session, exactly like `work claim`.
                    // The store rejects a closed session or a conflicting
                    // active claim, so reuse never reports success for an
                    // unmet requested condition or steals a claim.
                    let (claim, revision) = if *claim {
                        let (claim, event) = Runtime::attach(&mut db.store, project)?
                            .acquire_claim(*expected_revision, session.id, *ttl_ms)?;
                        (Some(claim), event.project_revision)
                    } else {
                        (None, db.project.project_revision)
                    };
                    let mut value = db.metadata(revision);
                    value["session"] = json!(session);
                    value["binding"] = json!(binding);
                    value["binding_reused"] = json!(true);
                    value["claim"] = json!(claim);
                    // The caller sees the session's actual claim state even
                    // when it did not request one.
                    value["active_claims"] = json!(
                        db.store
                            .session_claims(project, session.id)?
                            .iter()
                            .filter(|c| c.status == "active")
                            .collect::<Vec<_>>()
                    );
                    let claim_note = claim
                        .as_ref()
                        .map(|c| c.id.to_string())
                        .unwrap_or_else(|| "none".into());
                    return print(
                        &value,
                        &format!(
                            "Session: {}\nAgent: {}\nClaim: {}\nBinding: reused\nRevision: {}",
                            session.id, agent, claim_note, revision
                        ),
                        json_output,
                    );
                }
            }
            let mut runtime = Runtime::attach(&mut db.store, project)?;
            let (started, event) = match binding {
                Some(binding) => runtime.start_bound_session(*expected_revision, draft, binding)?,
                None => runtime.start_session(*expected_revision, draft)?,
            };
            let mut value = db.metadata(event.project_revision);
            value["session"] = json!(started.session);
            value["claim"] = json!(started.claim);
            value["event"] = event_brief(&event);
            print(
                &value,
                &format!(
                    "Session: {}\nAgent: {}\nClaim: {}\nRevision: {}",
                    started.session.id,
                    agent,
                    started
                        .claim
                        .as_ref()
                        .map(|c| c.id.to_string())
                        .unwrap_or_else(|| "none".into()),
                    event.project_revision
                ),
                json_output,
            )
        }
        SessionCommand::List { active, limit } => {
            let sessions = db.store.sessions(project, *active, *limit)?;
            db.check_revision()?;
            let mut value = db.metadata(db.project.project_revision);
            value["sessions"] = json!(sessions);
            value["limit"] = json!(limit);
            value["may_have_more"] = json!(sessions.len() == *limit);
            let rows = sessions
                .iter()
                .map(|s| {
                    format!(
                        "{} {} agent={} work={} checkpoint={}",
                        s.id,
                        s.status,
                        s.agent_id,
                        s.work_item_id.map(|id| id.to_string()).unwrap_or_default(),
                        s.last_checkpoint_id
                            .map(|id| id.to_string())
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            print(
                &value,
                &format!(
                    "Sessions: {} (limit {})\nRevision: {}\n{}",
                    sessions.len(),
                    limit,
                    db.project.project_revision,
                    rows
                ),
                json_output,
            )
        }
        SessionCommand::Show { id } => {
            let session = db.store.session(project, *id)?;
            let checkpoint = db.store.latest_checkpoint(project, *id)?;
            let inherited = db.store.incoming_handoff(project, *id)?;
            let claims = db.store.session_claims(project, *id)?;
            let successor = db.store.resumed_successor(project, *id)?;
            let saves = db.store.checkpoint_attempts(project, *id, 20)?;
            let checkpoint_save = checkpoint
                .as_ref()
                .map(|c| db.store.checkpoint_save_metadata(project, c.id))
                .transpose()?;
            let inherited_save = inherited
                .as_ref()
                .map(|c| db.store.checkpoint_save_metadata(project, c.id))
                .transpose()?;
            db.check_revision()?;
            let mut value = db.metadata(db.project.project_revision);
            value["session"] = json!(session);
            value["checkpoint"] = json!(checkpoint);
            value["inherited_checkpoint"] = json!(inherited);
            value["claims"] = json!(claims);
            value["resumed_successor"] = json!(successor);
            value["checkpoint_saves"] = json!(saves);
            value["checkpoint_save"] = json!(checkpoint_save);
            value["inherited_checkpoint_save"] = json!(inherited_save);
            value["context_requires_refresh"] = json!(true);
            value["checkpoint_write_revision"] = json!({"scope":"project","value":db.project.project_revision,"cli_option":"--expected-project-revision"});
            let next = checkpoint
                .as_ref()
                .or(inherited.as_ref())
                .map(|c| c.next_action.as_str())
                .unwrap_or("No checkpoint yet");
            let loops = checkpoint
                .as_ref()
                .or(inherited.as_ref())
                .map(|c| c.open_loops.join("; "))
                .unwrap_or_default();
            print(
                &value,
                &format!(
                    "Session: {} ({})\nSession agent label: {} (not caller proof)\nProject revision (checkpoint --expected-project-revision): {}\nNext: {}\nOpen loops: {}\nInherited checkpoint: {}\nIncomplete checkpoint attempts: {}\nSession revision: {}\nCheckpoint caller: {} (unverified)\n{}\nRefresh context before continuing work.",
                    session.id,
                    session.status,
                    session.agent_id,
                    db.project.project_revision,
                    next,
                    loops,
                    inherited
                        .as_ref()
                        .map(|c| c.id.to_string())
                        .unwrap_or_else(|| "none".into()),
                    saves.incomplete_count,
                    session.revision,
                    checkpoint_save
                        .as_ref()
                        .or(inherited_save.as_ref())
                        .and_then(|v| v["actor"]["agent_id"].as_str())
                        .unwrap_or("not declared"),
                    CHECKPOINT_LIMITATION,
                ),
                json_output,
            )
        }
        SessionCommand::Checkpoint {
            selection,
            work,
            context_hash,
            digest,
            next_action,
            open_loop,
            changed_entity,
            expected_revision,
        } => {
            // With an explicit session, validate the declaration in the write transaction
            // so a mismatch returns handoff guidance instead of an empty selector result.
            let session = if selection.session.is_some() {
                db.selected(
                    &Selection {
                        session: selection.session,
                        agent: None,
                    },
                    work.as_deref(),
                )?
            } else {
                db.selected(selection, work.as_deref())?
            };
            let draft = CheckpointDraft {
                context_hash: context_hash.clone(),
                digest: digest.clone(),
                next_action: next_action.clone(),
                open_loops: open_loop.clone(),
                changed_entities: changed_entity.clone(),
            };
            let (checkpoint, event) = Runtime::attach(&mut db.store, project)?
                .checkpoint_as(
                    *expected_revision,
                    session.id,
                    draft,
                    selection.agent.as_deref(),
                )
                .map_err(Error::for_checkpoint)?;
            let mut value = db.metadata(event.project_revision);
            value["checkpoint"] = json!(checkpoint);
            value["event"] = event_brief(&event);
            value["checkpoint_save"] = db.store.checkpoint_save_metadata(project, checkpoint.id)?;
            value["actor"] = value["checkpoint_save"]["actor"].clone();
            value["warning"] = json!(CHECKPOINT_LIMITATION);
            value["context_hash_verified"] = json!(false);
            value["context_hash_basis"] = json!("caller_supplied_last_used_context");
            value["save_status"] = json!("completed");
            print(
                &value,
                &format!(
                    "Checkpoint: {}\nSession: {}\nNext: {}\nOpen loops: {}\nProject revision: {}\nCaller: {} (unverified)\n{}",
                    checkpoint.id,
                    session.id,
                    checkpoint.next_action,
                    checkpoint.open_loops.join("; "),
                    event.project_revision,
                    selection.agent.as_deref().unwrap_or(
                        "not declared; use --agent, or session resume when changing agents"
                    ),
                    CHECKPOINT_LIMITATION
                ),
                json_output,
            )
        }
        SessionCommand::End {
            selection,
            work,
            outcome,
            expected_revision,
        } => {
            let session = db.selected(selection, work.as_deref())?;
            let (session, event) = Runtime::attach(&mut db.store, project)?.end_session(
                *expected_revision,
                session.id,
                (*outcome).into(),
            )?;
            let mut value = db.metadata(event.project_revision);
            value["session"] = json!(session);
            value["closed_claim_ids"] = event.payload["closed_claim_ids"].clone();
            value["event"] = event_brief(&event);
            print(
                &value,
                &format!(
                    "Session: {} ({})\nClosed claims: {}\nRevision: {}",
                    session.id, session.status, value["closed_claim_ids"], event.project_revision
                ),
                json_output,
            )
        }
    }
}

pub fn work(root: &Path, command: &WorkCommand, json_output: bool) -> Result<()> {
    let mut db = RuntimeProject::open(root, matches!(command, WorkCommand::Claim(_)))?;
    let project = db.project.id;
    match command {
        WorkCommand::Claim(args) => {
            let session = db.selected(&args.selection, Some(&args.work))?;
            let (claim, event) = Runtime::attach(&mut db.store, project)?.acquire_claim(
                args.expected_revision,
                session.id,
                args.ttl_ms,
            )?;
            let mut value = db.metadata(event.project_revision);
            value["claim"] = json!(claim);
            value["event"] = event_brief(&event);
            print(
                &value,
                &format!(
                    "Claim: {}\nSession: {}\nRevision: {}",
                    claim.id, session.id, event.project_revision
                ),
                json_output,
            )
        }
        WorkCommand::Release(args) => {
            let session = db.selected(&args.selection, Some(&args.work))?;
            let (claim, event) = Runtime::attach(&mut db.store, project)?.release_claim(
                args.expected_revision,
                session.id,
                args.claim,
            )?;
            let mut value = db.metadata(event.project_revision);
            value["claim"] = json!(claim);
            value["event"] = event_brief(&event);
            print(
                &value,
                &format!(
                    "Claim: {} ({})\nRevision: {}",
                    claim.id, claim.status, event.project_revision
                ),
                json_output,
            )
        }
        WorkCommand::Handoff(args) => {
            let session = db.selected(&args.selection, Some(&args.work))?;
            let (handoff, event) = Runtime::attach(&mut db.store, project)?.handoff(
                args.expected_revision,
                session.id,
                args.to_session,
                args.ttl_ms,
            )?;
            let mut value = db.metadata(event.project_revision);
            value["handoff"] = json!(handoff);
            value["context_requires_refresh"] = json!(true);
            value["event"] = event_brief(&event);
            print(
                &value,
                &format!(
                    "Handoff: {} -> {}\nCheckpoint: {}\nNext: {}\nOpen loops: {}\nTransferred claim: {}\nRevision: {}",
                    handoff.from_session.id,
                    handoff
                        .to_session
                        .as_ref()
                        .map(|s| s.id.to_string())
                        .unwrap_or_else(|| "unassigned; claim released".into()),
                    handoff.checkpoint.id,
                    handoff.checkpoint.next_action,
                    handoff.checkpoint.open_loops.join("; "),
                    handoff
                        .transferred_claim
                        .as_ref()
                        .map(|c| c.id.to_string())
                        .unwrap_or_else(|| "none".into()),
                    event.project_revision
                ),
                json_output,
            )
        }
        WorkCommand::History(args) => {
            let work = db.store.work_identity(project, &args.work)?;
            let cursor = args
                .cursor
                .as_ref()
                .map(|s| serde_json::from_str::<EventCursor>(s))
                .transpose()?;
            let query = EventQuery {
                work_item_id: Some(work),
                session_id: args.session,
                branch: if args.all_branches {
                    BranchFilter::Any
                } else {
                    BranchFilter::exact(db.project.current_branch_id)
                },
                event_type: args.event_type.clone(),
                after_revision: args.after_revision,
                cursor,
                limit: args.limit,
                ..Default::default()
            };
            let page = db.store.query_events(project, &query)?;
            let mut value = db.metadata(page.project_revision);
            value["work"] = json!(args.work);
            value["events"] = json!(page.events.iter().map(event_brief).collect::<Vec<_>>());
            value["next_cursor"] = json!(page.next_cursor);
            let rows = page
                .events
                .iter()
                .map(|e| {
                    format!(
                        "{} r{} {} {}",
                        e.id,
                        e.project_revision,
                        e.event_type,
                        short(&e.summary)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            print(
                &value,
                &format!(
                    "Work: {}\nRevision: {}\n{}\nNext cursor: {}",
                    args.work, page.project_revision, rows, value["next_cursor"]
                ),
                json_output,
            )
        }
        WorkCommand::Edit(_)
        | WorkCommand::Show { .. }
        | WorkCommand::Graph { .. }
        | WorkCommand::Assess(_)
        | WorkCommand::Manage(_)
        | WorkCommand::Prepare(_)
        | WorkCommand::PrepareCompletion(_)
        | WorkCommand::Create(_)
        | WorkCommand::CreateStatus(_)
        | WorkCommand::CreateRecover(_)
        | WorkCommand::Progress(_)
        | WorkCommand::Block(_)
        | WorkCommand::Unblock(_)
        | WorkCommand::Cancel(_)
        | WorkCommand::Complete(_)
        | WorkCommand::Reopen(_) => Err(Error::Unsupported(
            "use the work command router for source-backed operations".into(),
        )),
    }
}
