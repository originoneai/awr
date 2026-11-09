use crate::client_dialect::{self, HookDecision, HookDialect, HookEvent, LifecycleEvent};
use crate::query::QueryProject;
use awr_context::{BootstrapRequest, bootstrap};
use awr_core::*;
use awr_runtime::{ResumeRequest, Runtime, resume_session};
use awr_store::Store;
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Args)]
pub struct Identity {
    /// Local hook dialect for the continuity record (`generic`, `codex`, `kimi` or `claude`). This is not an authenticated MCP client principal.
    #[arg(long)]
    client: String,
    /// Native host conversation/session ID stored as `ClientBinding.external_session`. Use `--session` on `client bind` to attach it to an existing AWR session.
    #[arg(long)]
    external_session: String,
}
#[derive(Debug, Subcommand)]
pub enum ClientCommand {
    /// Bind local native-client continuity to an AWR session without acquiring a claim. `--session` is the explicit bridge to an existing active session; without it, local continuity may create or recover a session. This does not create an MCP conversation binding.
    Bind {
        #[command(flatten)]
        identity: Identity,
        #[arg(long)]
        work: String,
        /// Attach to this existing active AWR session, including an MCP-bound session.
        #[arg(long)]
        session: Option<Id>,
        #[arg(long)]
        from_session: Option<Id>,
        #[arg(long, default_value = "not-reported")]
        model: String,
    },
    /// Persist the next action and open loops consumed by automatic checkpoints.
    Progress {
        #[command(flatten)]
        identity: Identity,
        #[arg(long)]
        next_action: String,
        #[arg(long)]
        digest: Option<String>,
        #[arg(long)]
        open_loop: Vec<String>,
        #[arg(long)]
        clear_open_loops: bool,
    },
    Show {
        #[command(flatten)]
        identity: Identity,
    },
    /// Receive a documented lifecycle event as JSON on stdin; no transcript bodies are read.
    Hook {
        /// Local hook dialect for the event envelope. Required: use `generic` unless a documented native dialect applies; this is distinct from the MCP client principal.
        #[arg(long)]
        client: String,
        #[arg(long)]
        work: Option<String>,
    },
    /// Preview or install an L2 project-local lifecycle adapter. Currently Codex only; other hosts stay on the L0 generic receiver.
    Install {
        /// Host dialect. Required. Only `codex` currently writes hook files.
        #[arg(long)]
        client: String,
        #[arg(long)]
        work: String,
        #[arg(long)]
        accept: bool,
    },
}

fn validate_identity(client: &str, external: &str) -> Result<()> {
    if !["codex", "kimi", "generic", "claude"].contains(&client)
        || external.trim().is_empty()
        || external.len() > 512
    {
        return Err(Error::InvalidInput(
            "client must be generic, or a documented native dialect (codex, kimi or claude). Other hosts: use --client generic and prefix the conversation ID, e.g. --external-session cursor:<native-id>".into(),
        ));
    }
    Ok(())
}
pub(crate) fn lock(root: &Path, area: &str, key: &str) -> Result<fs::File> {
    let runtime = crate::source::runtime_dir(root, false)?;
    let dir = runtime.join(area);
    if let Err(error) = fs::create_dir(&dir) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(Error::Storage(format!(
                "creating runtime lock directory: {error}"
            )));
        }
    }
    let directory = awr_source::open_dir_exact(&dir)?;
    let name = format!("{:x}.lock", Sha256::digest(key.as_bytes()));
    // Exclusive creation avoids a create/open race in capability path resolution when
    // several processes encounter the same lock for the first time. Never unlink locks.
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    let file = match directory.open_with(&name, &options) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut existing = cap_std::fs::OpenOptions::new();
            existing.read(true).write(true);
            directory.open_with(&name, &existing)
        }
        Err(error) => Err(error),
    }
    .map_err(|e| Error::Storage(format!("opening runtime lock: {e}")))?
    .into_std();
    file.lock()
        .map_err(|e| Error::Storage(format!("acquiring runtime lock: {e}")))?;
    Ok(file)
}
fn save(
    store: &mut Store,
    project: Id,
    binding: ClientBinding,
    checkpointed: bool,
) -> Result<ClientBinding> {
    for _ in 0..8 {
        let revision = store.project(project)?.project_revision;
        match store.save_client_binding(project, revision, binding.clone(), checkpointed) {
            Ok((binding, _)) => return Ok(binding),
            Err(Error::RevisionConflict { .. }) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(Error::Storage(
        "project stayed busy while saving client continuity; retry this delivery".into(),
    ))
}
fn new_binding(
    root: &Path,
    db: &mut QueryProject,
    client: &str,
    external: &str,
    work: &str,
    session: Option<Id>,
    from: Option<Id>,
    model: &str,
) -> Result<ClientBinding> {
    validate_identity(client, external)?;
    if session.is_some() && from.is_some() {
        return Err(Error::InvalidInput(
            "choose a bound session or a predecessor, not both".into(),
        ));
    }
    let project = db.project.id;
    if let Some(binding) = db.store.client_binding(project, client, external)? {
        let bound = db.store.session(project, binding.session_id)?;
        if session.is_some_and(|id| id != binding.session_id)
            || db
                .store
                .work_item_by_id(project, bound.work_item_id.unwrap())?
                .item
                .meta
                .external_key
                != work
        {
            return Err(Error::SourceConflict(
                "client conversation already belongs to different work/session".into(),
            ));
        }
        if bound.status == "active" {
            return Ok(binding);
        }
        return Err(Error::InvalidTransition("this client binding ended; use a new client conversation and bind --from-session to continue it".into()));
    }
    db.finish()?;
    let agent = format!("{client}:{external}");
    let bound = if let Some(id) = session {
        let s = db.store.session(project, id)?;
        if s.status != "active"
            || s.work_item_id.is_none()
            || db
                .store
                .work_item_by_id(project, s.work_item_id.unwrap())?
                .item
                .meta
                .external_key
                != work
        {
            return Err(Error::InvalidInput(
                "binding requires an active session on the selected work".into(),
            ));
        }
        s
    } else if let Some(id) = from {
        let revision = db.store.project(project)?.project_revision;
        let report = resume_session(
            &mut db.store,
            root,
            &ResumeRequest {
                from_session_id: Some(id),
                work_item_key: Some(work.into()),
                agent_id: agent.clone(),
                provider: client.into(),
                model: model.into(),
                claim: ResumeClaim::None,
                claim_ttl_ms: None,
                expected_revision: revision,
                token_budget: 5000,
                paths: None,
                tags: None,
                goal_keys: vec![],
                source_sha: None,
            },
        )?;
        report
            .resumed
            .ok_or_else(|| {
                Error::ContextIncomplete("predecessor context could not be recovered".into())
            })?
            .session
    } else {
        // Recover the gap between session creation and binding persistence after an interrupted hook.
        let matches: Vec<_> = db
            .store
            .sessions(project, true, 1000)?
            .into_iter()
            .filter(|s| s.agent_id == agent && s.branch_id == db.project.current_branch_id)
            .collect();
        if matches.len() > 1 {
            return Err(Error::SourceConflict(
                "multiple orphan client sessions; bind an explicit session".into(),
            ));
        }
        if let Some(s) = matches.into_iter().next() {
            if s.work_item_id.is_none()
                || db
                    .store
                    .work_item_by_id(project, s.work_item_id.unwrap())?
                    .item
                    .meta
                    .external_key
                    != work
            {
                return Err(Error::SourceConflict(
                    "unbound client session belongs to different work".into(),
                ));
            }
            s
        } else {
            let revision = db.store.project(project)?.project_revision;
            db.store
                .start_session(
                    project,
                    revision,
                    SessionDraft {
                        work_item_key: Some(work.into()),
                        agent_id: agent.clone(),
                        provider: client.into(),
                        model: model.into(),
                        branch_id: db.project.current_branch_id,
                        claim: false,
                        claim_ttl_ms: None,
                    },
                )?
                .0
                .session
        }
    };
    let pack = bootstrap(
        &mut db.store,
        root,
        &BootstrapRequest {
            work_item_key: Some(work.into()),
            session_id: Some(bound.id),
            agent_id: Some(bound.agent_id.clone()),
            token_budget: 5000,
        },
    )?;
    let source_work = db
        .store
        .work_item_by_id(project, bound.work_item_id.unwrap())?;
    let checkpoint = pack.context.checkpoint;
    let next = checkpoint
        .as_ref()
        .map(|c| c.next_action.clone())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| source_work.item.next_action.clone());
    let binding = ClientBinding {
        client: client.into(),
        external_session: external.into(),
        session_id: bound.id,
        revision: 0,
        digest: checkpoint
            .as_ref()
            .map(|c| c.digest.clone())
            .unwrap_or_else(|| {
                "Client attached; unrecorded conversation history is unavailable.".into()
            }),
        next_action: if next.is_empty() {
            "Review current work and record its next action.".into()
        } else {
            next
        },
        open_loops: checkpoint
            .as_ref()
            .map(|c| c.open_loops.clone())
            .unwrap_or_default(),
        context_hash: Some(pack.context_hash),
        context_revision: Some(pack.context.project_revision),
        progress_revision: 0,
        last_delivery_key: None,
        last_hook_event: None,
        checkpoint_id: None,
        observed_at: now_millis()?,
    };
    save(&mut db.store, project, binding, false)
}

fn native_hook(root: &Path, client: &str, work: Option<&str>) -> Result<Value> {
    let mut bytes = vec![];
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let dialect = client_dialect::dialect(client)?;
    // Envelope validation and existing lifecycle failures remain caller-visible.
    let mut event = dialect.parse(&bytes)?;
    event.cwd = event.cwd.canonicalize()?;
    if !event.cwd.starts_with(root) {
        return Err(Error::RuleViolation(
            "client event belongs to a different project".into(),
        ));
    }
    let (decision, metadata) = process_hook(root, client, work, &event, dialect.as_ref())?;
    let mut rendered = dialect.render(&decision, &event);
    debug_assert_eq!(rendered.exit_code, 0);
    if !metadata.is_null() {
        let target = rendered
            .stdout
            .as_object_mut()
            .unwrap()
            .entry("awr")
            .or_insert_with(|| json!({}));
        target
            .as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
    }
    Ok(rendered.stdout)
}

fn gate_failure(error: Error) -> (HookDecision, Value) {
    eprintln!(
        "AWR hook continued after a gate error ({}); run awr doctor.",
        error.code()
    );
    (
        HookDecision::allow(),
        json!({"internal_error":error.code()}),
    )
}

fn process_hook(
    root: &Path,
    client: &str,
    work: Option<&str>,
    event: &HookEvent,
    dialect: &dyn HookDialect,
) -> Result<(HookDecision, Value)> {
    if event.event == LifecycleEvent::PreToolUse {
        return Ok(write_gate(root, client, event, dialect).unwrap_or_else(gate_failure));
    }
    if event.event == LifecycleEvent::Stop {
        match completion_gate(root, client, event, dialect, &|_, _, _, _, _| Ok(false)) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => (),
            Err(error) => return Ok(gate_failure(error)),
        }
    }
    let cached = Store::open_readonly(&crate::source::runtime_dir(root, false)?.join("state.db"))?;
    let project = cached.project_by_root(root)?;
    let binding = cached.client_binding(project.id, client, &event.native_session)?;
    let work = match work {
        Some(work) => Some(work.to_owned()),
        None => binding
            .map(|binding| -> Result<String> {
                let session = cached.session(project.id, binding.session_id)?;
                Ok(cached
                    .work_item_by_id(
                        project.id,
                        session.work_item_id.ok_or_else(|| {
                            Error::InvalidInput("client session has no work".into())
                        })?,
                    )?
                    .item
                    .meta
                    .external_key)
            })
            .transpose()?,
    };
    drop(cached);
    let Some(work) = work else {
        if matches!(
            event.event,
            LifecycleEvent::SessionStart | LifecycleEvent::PostCompact
        ) {
            return Ok((
                HookDecision::Continue {
                    context: Some(unbound_context(root, client, event)?),
                },
                json!({"bound":false,"checkpoint_saved":false}),
            ));
        }
        return Ok((
            HookDecision::allow(),
            json!({"bound":false,"checkpoint_saved":false}),
        ));
    };
    let value = lifecycle_hook(root, client, &work, event)?;
    let context = value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .map(str::to_owned);
    Ok((HookDecision::Continue { context }, value["awr"].clone()))
}

fn lifecycle_hook(root: &Path, client: &str, work: &str, hook: &HookEvent) -> Result<Value> {
    let external = hook.native_session.as_str();
    let event = hook.event.name();
    let _lock = lock(root, "clients", &format!("{client}:{external}"))?;
    // Other machines publish while this one is idle, so a session that is
    // starting is the cheapest place to notice it. This runs before the project
    // is opened, because opening it refreshes the sources: a peer's bytes have
    // to be in place first, both so the context describes the current tree and
    // so a peer holding a parseable copy can heal one that was broken here. It
    // never changes the exit code - an unreachable store is a line to read, not
    // a session that refuses to open.
    let exchange = if event == "SessionStart" {
        crate::workspace::session_start(root)
    } else {
        None
    };
    let mut db = QueryProject::open(root)?;
    let project = db.project.id;
    let model = hook.model.as_deref().unwrap_or("not-reported");
    let mut binding = new_binding(root, &mut db, client, external, work, None, None, model)?;
    let compaction = hook
        .compaction
        .as_ref()
        .map(|c| {
            let expected_revision = db.store.project(project)?.project_revision;
            awr_runtime::observe_compaction(
                &mut db.store,
                root,
                &awr_runtime::ObserveCompactionRequest {
                    session: binding.session_id,
                    expected_revision,
                    observation: c.observation.clone(),
                    policy: c.policy.clone(),
                },
            )
        })
        .transpose()?;
    if matches!(event, "SessionStart" | "PostCompact") {
        let session = db.store.session(project, binding.session_id)?;
        let pack = bootstrap(
            &mut db.store,
            root,
            &BootstrapRequest {
                work_item_key: Some(work.into()),
                session_id: Some(binding.session_id),
                agent_id: Some(session.agent_id),
                token_budget: 5000,
            },
        )?;
        binding.context_hash = Some(pack.context_hash.clone());
        binding.context_revision = Some(pack.context.project_revision);
        binding.last_hook_event = Some(event.into());
        binding = save(&mut db.store, project, binding, false)?;
        let executions = awr_runtime::inspect_work_executions(
            &db.store,
            root,
            project,
            session.work_item_id.unwrap(),
            session.branch_id,
        )?;
        let continuity = format!(
            "{}\n\nSaved next action: {}\nOpen loops: {}\nTo update continuity, use awr client progress --client {} --external-session {} --next-action <action>. Unrecorded conversation history has not been reconstructed.",
            pack.rendered_context,
            binding.next_action,
            serde_json::to_string(&binding.open_loops)?,
            client,
            external
        );
        let continuity = format!(
            "{continuity}\n{}",
            awr_runtime::render_execution_observations(&executions)?
        );
        let continuity = if let Some(c) = &compaction {
            format!(
                "{continuity}\nAction guidance: {}",
                serde_json::to_string(&c["guidance"])?
            )
        } else {
            continuity
        };
        let continuity = match exchange {
            Some(notice) => format!("{continuity}\n{notice}"),
            None => continuity,
        };
        let total_tokens = awr_context::token_count(&continuity);
        if total_tokens > 10000 {
            return Err(Error::BudgetExceeded {
                required: total_tokens,
                budget: 10000,
            });
        }
        let mut result = json!({"continue":true,"hookSpecificOutput":{"hookEventName":event,"additionalContext":continuity},"awr":{"binding":binding,"context_ready":pack.context.complete,"checkpoint_saved":false,"executions":executions,"additional_context_tokens":total_tokens}});
        if let Some(mut c) = compaction {
            // The instruction is already in additionalContext; do not duplicate it in metadata.
            c.as_object_mut().unwrap().remove("guidance");
            result["awr"]["compaction"] = c;
        }
        return Ok(result);
    }
    // Stable native turn IDs deduplicate repeats. Events without IDs also bind their observed
    // source fingerprints, persisted progress and non-checkpoint work revision.
    let sources = db.store.sources(project)?;
    let source_state: Vec<_> = sources
        .iter()
        .map(|s| (&s.id, &s.fingerprint, &s.freshness))
        .collect();
    let native_key = hook.turn_id.as_deref();
    let basis = json!([
        client,
        external,
        event,
        native_key,
        hook.trigger.as_deref().unwrap_or(""),
        hook.reported_stop_hook_active,
        binding.progress_revision,
        source_state,
        db.store.client_work_revision(project, binding.session_id)?
    ]);
    let key = format!("{:x}", Sha256::digest(serde_json::to_vec(&basis)?));
    if let Some(prior) = db.store.client_delivery(project, client, external, &key)? {
        return Ok(
            json!({"continue":true,"awr":{"duplicate":true,"checkpoint_saved":true,"binding":prior}}),
        );
    }
    let checkpoint = if let Some(saved) =
        db.store
            .client_delivery_checkpoint(project, binding.session_id, &key)?
    {
        saved
    } else {
        let digest = format!(
            "[client-delivery:{key}] {}\nObserved lifecycle event: {event}. Transcript bodies were not imported.",
            binding.digest
        );
        let draft = CheckpointDraft {
            context_hash: binding.context_hash.clone().ok_or_else(|| {
                Error::ContextIncomplete("client has not loaded an AWR context".into())
            })?,
            digest,
            next_action: binding.next_action.clone(),
            open_loops: binding.open_loops.clone(),
            changed_entities: vec![],
        };
        let revision = db.store.project(project)?.project_revision;
        Runtime::attach(&mut db.store, project)?
            .checkpoint(revision, binding.session_id, draft)?
            .0
    };
    binding.checkpoint_id = Some(checkpoint.id);
    binding.last_delivery_key = Some(key);
    binding.last_hook_event = Some(event.into());
    binding = save(&mut db.store, project, binding, true)?;
    // A shutdown notification is advisory. Save its evidence; explicit handoff/end still owns
    // releasing claims and ending work sessions, so another active executor is never interrupted.
    Ok(json!({"continue":true,"awr":{"checkpoint_saved":true,"duplicate":false,"binding":binding}}))
}

fn unbound_context(root: &Path, client: &str, event: &HookEvent) -> Result<String> {
    let output = std::process::Command::new(std::env::current_exe()?)
        .arg("--project")
        .arg(root)
        .args(["status", "--view", "action"])
        .env("AWR_LOCAL_WEB", "0")
        .output()?;
    if !output.status.success() {
        return Err(Error::SourceUnavailable(
            "cannot load the project action view".into(),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut end = text.len().min(1536);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(format!(
        "{}\nNext: Bind this conversation to selected work: awr client bind --client {} --external-session {} --work <KEY>. To reuse an active AWR session, add --session <SESSION>.",
        &text[..end],
        client,
        shell_quote(&event.native_session)
    ))
}

fn gates_enabled() -> bool {
    std::env::var("AWR_HOOK_GATES").as_deref() != Ok("off")
}

fn claimed_work(
    store: &Store,
    project: &Project,
    client: &str,
    external: &str,
) -> Result<Option<(Session, WorkItem)>> {
    let Some(binding) = store.client_binding(project.id, client, external)? else {
        return Ok(None);
    };
    let session = store.session(project.id, binding.session_id)?;
    if session.status != "active" || session.branch_id != project.current_branch_id {
        return Ok(None);
    }
    let Some(work_id) = session.work_item_id else {
        return Ok(None);
    };
    let now = now_millis()?;
    let claimed = store
        .session_claims(project.id, session.id)?
        .iter()
        .any(|claim| {
            claim.status == "active"
                && claim.work_item_id == work_id
                && claim.released_at.is_none()
                && claim.expires_at.is_none_or(|expiry| expiry > now)
        });
    if !claimed {
        return Ok(None);
    }
    let work = store.work_item_by_id(project.id, work_id)?.item;
    if matches!(work.status, WorkStatus::Completed | WorkStatus::Cancelled) {
        return Ok(None);
    }
    Ok(Some((session, work)))
}

/// Resolve existing symlinks before handling `..`, including a not-yet-created leaf.
fn target_path(cwd: &Path, path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => (),
            Component::ParentDir => {
                resolved.pop();
            }
            other => {
                resolved.push(other);
                match fs::symlink_metadata(&resolved) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        resolved = resolved.canonicalize()?;
                    }
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    // Canonicalize the existing ancestor as well, so native absolute paths and
    // project roots use the same platform representation (including Windows prefixes).
    let mut ancestor = resolved.as_path();
    let mut suffix = Vec::new();
    while !ancestor.try_exists()? {
        suffix.push(
            ancestor
                .file_name()
                .ok_or_else(|| Error::InvalidInput("write target has no existing ancestor".into()))?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| Error::InvalidInput("write target has no existing ancestor".into()))?;
    }
    let mut resolved = ancestor.canonicalize()?;
    for name in suffix.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn hash_key(value: &Value) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

struct GateCandidate {
    decision: HookDecision,
    gate: &'static str,
    delivery_key: String,
    turn_key: Option<String>,
    path: Option<String>,
    source_sha: Option<String>,
    session: Option<Id>,
}

fn record_gate(
    root: &Path,
    client: &str,
    event: &HookEvent,
    dialect: &dyn HookDialect,
    candidate: GateCandidate,
) -> Result<(HookDecision, Value)> {
    // Unsupported decisions are surfaced by the dialect without recording a gate that never ran.
    if !dialect.supports(&candidate.decision) {
        return Ok((candidate.decision, json!({})));
    }
    let database = crate::source::runtime_dir(root, false)?.join("state.db");
    let cached = Store::open_readonly(&database)?;
    let project = cached.project_by_root(root)?;
    if cached.client_gate_seen(
        project.id,
        client,
        &event.native_session,
        &candidate.delivery_key,
        candidate.turn_key.as_deref(),
    )? {
        return Ok((HookDecision::allow(), json!({"gate_duplicate":true})));
    }
    drop(cached);
    // No lock file and no writable connection are opened on a pass-through decision.
    let _lock = lock(
        root,
        "clients",
        &format!("gate:{client}:{}", event.native_session),
    )?;
    let mut store = Store::open(&database)?;
    if store.client_gate_seen(
        project.id,
        client,
        &event.native_session,
        &candidate.delivery_key,
        candidate.turn_key.as_deref(),
    )? {
        return Ok((HookDecision::allow(), json!({"gate_duplicate":true})));
    }
    let mut receipt = EventDraft::new("client.gate", "Returned an opt-in hook decision");
    receipt.session_id = candidate.session;
    receipt.branch_id = project.current_branch_id;
    receipt.payload = json!({"client":client,"external_session":event.native_session,"gate":candidate.gate,"decision":candidate.decision.name(),"reason":candidate.decision.reason(),"path":candidate.path,"delivery_key":candidate.delivery_key,"turn_key":candidate.turn_key,"source_sha":candidate.source_sha});
    for _ in 0..8 {
        let revision = store.project(project.id)?.project_revision;
        match store.record_client_gate(project.id, revision, receipt.clone()) {
            Ok(saved) => return Ok((candidate.decision, json!({"gate_event_id":saved.id}))),
            Err(Error::RevisionConflict { .. }) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(Error::Storage(
        "project stayed busy while recording a hook decision".into(),
    ))
}

fn write_gate(
    root: &Path,
    client: &str,
    event: &HookEvent,
    dialect: &dyn HookDialect,
) -> Result<(HookDecision, Value)> {
    let allow = || Ok((HookDecision::allow(), json!({})));
    if !gates_enabled() {
        return allow();
    }
    let hooks = awr_source::GatePolicy::load(root)?.hooks;
    if hooks.write_gate == awr_source::WriteGate::Off {
        return allow();
    }
    let Some(tool) = &event.tool else {
        return allow();
    };
    if !dialect.write_tools().contains(&tool.name.as_str()) {
        return allow();
    }
    let mut paths = Vec::new();
    for path in &tool.paths {
        let absolute = target_path(&event.cwd, path)?;
        if let Ok(relative) = absolute.strip_prefix(root) {
            if !hooks.is_exempt(relative)? {
                paths.push(relative.to_owned());
            }
        }
    }
    if paths.is_empty() {
        return allow();
    }
    let store = Store::open_readonly(&crate::source::runtime_dir(root, false)?.join("state.db"))?;
    let project = store.project_by_root(root)?;
    let claimed = claimed_work(&store, &project, client, &event.native_session)?;
    let (decision, path) = if let Some((_, work)) = &claimed {
        if !hooks.write_gate_paths {
            return allow();
        }
        let mut outside = None;
        for path in paths {
            if !awr_source::hook_work_path_matches(&work.paths, &path)? {
                outside = Some(path);
                break;
            }
        }
        let Some(path) = outside else {
            return allow();
        };
        (
            HookDecision::Ask {
                reason: format!(
                    "Target {} is outside the paths declared by {}. Review awr work show {} before writing.",
                    path.display(),
                    work.meta.external_key,
                    work.meta.external_key
                ),
            },
            path,
        )
    } else {
        let reason = format!(
            "No claimed work for this session. See what to continue: awr status. Then: awr session start --work <KEY> --claim --agent <AGENT> --provider <PROVIDER> --model <MODEL> --expected-revision <REV>. Bind it with awr client bind --client {client} --external-session {} --work <KEY> --session <SESSION>.",
            shell_quote(&event.native_session)
        );
        let decision = match hooks.write_gate {
            awr_source::WriteGate::Deny => HookDecision::Deny { reason },
            _ => HookDecision::Ask { reason },
        };
        (decision, paths.remove(0))
    };
    // A host without turn IDs gets one interruption until saved continuity advances.
    let binding = store.client_binding(project.id, client, &event.native_session)?;
    let epoch = match &event.turn_id {
        Some(turn) => json!(["native", turn]),
        None => json!([
            "saved",
            binding
                .as_ref()
                .map(|b| (b.progress_revision, b.checkpoint_id))
        ]),
    };
    let turn_key = hash_key(&json!([
        client,
        event.native_session,
        project.current_branch_id,
        "write",
        epoch
    ]))?;
    let path = path.to_string_lossy().replace('\\', "/");
    let delivery_key = hash_key(&json!([turn_key, path, decision.reason()]))?;
    let session = claimed.as_ref().map(|(s, _)| s.id);
    drop(store);
    record_gate(
        root,
        client,
        event,
        dialect,
        GateCandidate {
            decision,
            gate: "write",
            delivery_key,
            turn_key: Some(turn_key),
            path: Some(path),
            source_sha: None,
            session,
        },
    )
}

// The completion gate will be supplied by the checks consumer. Until then production
// always reports false; tests inject a result without a user-visible bypass switch.
type CompletionCheck = dyn Fn(&Store, Id, &Session, &WorkItem, &str) -> Result<bool>;
fn completion_gate(
    root: &Path,
    client: &str,
    event: &HookEvent,
    dialect: &dyn HookDialect,
    check: &CompletionCheck,
) -> Result<Option<(HookDecision, Value)>> {
    if !gates_enabled() || event.stop_hook_active {
        return Ok(None);
    }
    if !awr_source::GatePolicy::load(root)?
        .hooks
        .completion_reminder
    {
        return Ok(None);
    }
    let store = Store::open_readonly(&crate::source::runtime_dir(root, false)?.join("state.db"))?;
    let project = store.project_by_root(root)?;
    let Some((session, work)) = claimed_work(&store, &project, client, &event.native_session)?
    else {
        return Ok(None);
    };
    if work.status != WorkStatus::InProgress {
        return Ok(None);
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || !is_source_sha(&sha) {
        return Ok(None);
    }
    if !check(&store, project.id, &session, &work, &sha)? {
        return Ok(None);
    }
    let delivery_key = hash_key(&json!([
        client,
        event.native_session,
        "completion",
        session.id,
        work.meta.id,
        sha
    ]))?;
    let turn_key = event
        .turn_id
        .as_ref()
        .map(|turn| hash_key(&json!([client, event.native_session, "completion", turn])))
        .transpose()?;
    // A repeated Stop must continue into the ordinary checkpoint path.
    if store.client_gate_seen(
        project.id,
        client,
        &event.native_session,
        &delivery_key,
        turn_key.as_deref(),
    )? {
        return Ok(None);
    }
    let reason = format!(
        "All checks for {} pass at {}. Complete it: awr work complete {} --session {} --reason <REASON> --input <COMPLETION_INPUT> --expected-revision <REV> --end-session. If it should stay open, tell the user why.",
        work.meta.external_key, sha, work.meta.external_key, session.id
    );
    drop(store);
    let result = record_gate(
        root,
        client,
        event,
        dialect,
        GateCandidate {
            decision: HookDecision::BlockStop { reason },
            gate: "completion",
            delivery_key,
            turn_key,
            path: None,
            source_sha: Some(sha),
            session: Some(session.id),
        },
    )?;
    if matches!(result.0, HookDecision::Continue { .. }) {
        return Ok(None);
    }
    Ok(Some(result))
}

fn shell_quote(s: &str) -> String {
    if cfg!(windows) {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}
fn install(root: &Path, client: &str, work: &str, accept: bool) -> Result<Value> {
    if client != "codex" {
        return Err(Error::Unsupported("automatic L2 installation currently supports Codex only; other hosts use --client generic with a host-prefixed conversation ID and the lifecycle receiver, with manual checkpoints".into()));
    }
    let db = QueryProject::open(root)?;
    db.finish()?;
    db.store.work_item(db.project.id, work)?;
    let exe = std::env::current_exe()?.canonicalize()?;
    let command = format!(
        "{}{} --project {} client hook --client codex --work {}",
        if cfg!(windows) { "& " } else { "" },
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&root.to_string_lossy()),
        shell_quote(work)
    );
    let dir = root.join(".codex");
    let path = dir.join("hooks.json");
    if dir.exists() {
        awr_source::open_dir_exact(&dir)?;
    }
    let original = if path.exists() {
        Some(awr_source::read_source_capped(&path, 1024 * 1024)?)
    } else {
        None
    };
    let mut config: Value = original
        .as_ref()
        .map(|bytes| {
            serde_json::from_slice(bytes).map_err(|_| {
                Error::InvalidInput(
                    "existing hooks.json is invalid; preserve and fix it before installation"
                        .into(),
                )
            })
        })
        .transpose()?
        .unwrap_or_else(|| json!({"hooks":{}}));
    let object = config
        .as_object_mut()
        .ok_or_else(|| Error::InvalidInput("hooks configuration must be an object".into()))?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::InvalidInput("hooks field must be an object".into()))?;
    for event in [
        "SessionStart",
        "PreCompact",
        "PostCompact",
        "Stop",
        "SessionEnd",
        "Interrupt",
    ] {
        let entries = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| Error::InvalidInput("hook event handlers must be arrays".into()))?;
        if !entries.iter().any(|v| {
            v["hooks"]
                .as_array()
                .is_some_and(|v| v.iter().any(|h| h["command"] == command))
        }) {
            entries.push(json!({"hooks":[{"type":"command","command":command,"timeout":if ["SessionEnd","Interrupt"].contains(&event){3}else{15},"statusMessage":"AWR work continuity"}]}));
        }
    }
    if accept {
        if !dir.exists() {
            fs::create_dir(&dir)?;
        }
        let _lock = lock(root, "clients", "hook-install")?;
        let actual = if path.exists() {
            Some(awr_source::read_source_capped(&path, 1024 * 1024)?)
        } else {
            None
        };
        if actual != original {
            return Err(Error::SourceConflict(
                "hook configuration changed during installation".into(),
            ));
        }
        if actual
            .as_ref()
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .as_ref()
            != Some(&config)
        {
            let temp = dir.join(format!("awr-hooks-{}.json", Id::new()));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&serde_json::to_vec_pretty(&config)?)?;
            file.sync_all()?;
            fs::rename(temp, &path)?;
        }
        let config_path = dir.join("config.toml");
        if !config_path.exists() {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(config_path)?
                .write_all(b"# Project-local Codex configuration; review AWR hooks in /hooks.\n")?;
        }
    }
    Ok(
        json!({"installed":accept,"path":path,"configuration":config,"activation_verified":false,"next_action":"Open a fresh trusted client session and review the exact AWR hook definitions in /hooks. A checkpoint receipt proves delivery; configuration alone does not."}),
    )
}

pub fn run(root: &Path, command: &ClientCommand, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let mut value = match command {
        ClientCommand::Hook { client, work } => native_hook(&root, client, work.as_deref())?,
        ClientCommand::Install {
            client,
            work,
            accept,
        } => install(&root, client, work, *accept)?,
        ClientCommand::Show { identity } => {
            let db = crate::session::RuntimeProject::open(&root, false)?;
            json!({"binding":db.store.client_binding(db.project.id,&identity.client,&identity.external_session)?})
        }
        ClientCommand::Bind {
            identity,
            work,
            session,
            from_session,
            model,
        } => {
            validate_identity(&identity.client, &identity.external_session)?;
            let _lock = lock(
                &root,
                "clients",
                &format!("{}:{}", identity.client, identity.external_session),
            )?;
            let mut db = QueryProject::open(&root)?;
            let mut binding = new_binding(
                &root,
                &mut db,
                &identity.client,
                &identity.external_session,
                work,
                *session,
                *from_session,
                model,
            )?;
            let native = db.store.session(db.project.id, binding.session_id)?;
            let pack = bootstrap(
                &mut db.store,
                &root,
                &BootstrapRequest {
                    work_item_key: Some(work.clone()),
                    session_id: Some(binding.session_id),
                    agent_id: Some(native.agent_id),
                    token_budget: 5000,
                },
            )?;
            binding.context_hash = Some(pack.context_hash.clone());
            binding.context_revision = Some(pack.context.project_revision);
            let project = db.project.id;
            let executions = awr_runtime::inspect_work_executions(
                &db.store,
                &root,
                project,
                native.work_item_id.unwrap(),
                native.branch_id,
            )?;
            json!({"binding":save(&mut db.store,project,binding,false)?,"context":pack,"executions":executions})
        }
        ClientCommand::Progress {
            identity,
            next_action,
            digest,
            open_loop,
            clear_open_loops,
        } => {
            validate_identity(&identity.client, &identity.external_session)?;
            let _lock = lock(
                &root,
                "clients",
                &format!("{}:{}", identity.client, identity.external_session),
            )?;
            let mut db = QueryProject::open(&root)?;
            let project = db.project.id;
            let mut binding = db
                .store
                .client_binding(project, &identity.client, &identity.external_session)?
                .ok_or_else(|| {
                    Error::NotFound("client binding; bind this conversation first".into())
                })?;
            binding.next_action = next_action.clone();
            if let Some(v) = digest {
                binding.digest = v.clone();
            }
            if *clear_open_loops || !open_loop.is_empty() {
                binding.open_loops = open_loop.clone();
            }
            binding.progress_revision = db.store.project(project)?.project_revision;
            json!({"binding":save(&mut db.store,project,binding,false)?})
        }
    };
    if matches!(command, ClientCommand::Hook { .. }) && !json_output {
        let unsupported = value["awr"]["unsupported_decision"].clone();
        value.as_object_mut().unwrap().remove("awr");
        if !unsupported.is_null() {
            value["awr"] = json!({"unsupported_decision":unsupported});
        }
        if matches!(command, ClientCommand::Hook { client, .. } if client == "claude")
            && value == json!({})
        {
            return Ok(());
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_dialect::{ClaudeDialect, HookDialect};
    use std::process::Command;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("awr-reminder-{}", Id::new()));
            fs::create_dir_all(path.join(".awr")).unwrap();
            let fixture = Self(path.canonicalize().unwrap());
            for (name, contents) in [
                (
                    ".awr/project.toml",
                    include_str!("../../../examples/basic/project.toml"),
                ),
                ("GOALS.md", include_str!("../../../examples/basic/GOALS.md")),
                ("PLAN.md", include_str!("../../../examples/basic/PLAN.md")),
                ("RULES.md", include_str!("../../../examples/basic/RULES.md")),
                (
                    "work-ledger.yaml",
                    include_str!("../../../examples/basic/work-ledger.yaml"),
                ),
            ] {
                fs::write(
                    fixture.0.join(name),
                    contents.replace("status: ready", "status: in_progress"),
                )
                .unwrap();
            }
            fs::write(
                fixture.0.join(".awr/gates.toml"),
                "version = 1\n[hooks]\ncompletion_reminder = true\n",
            )
            .unwrap();
            // The indexed fixture is synthetic; no repository or real session state is reused.
            let mut store = Store::open(&fixture.0.join(".awr/state.db")).unwrap();
            awr_source::index_project(
                &mut store,
                &fixture.0,
                &awr_source::Manifest::load(&fixture.0).unwrap(),
                false,
            )
            .unwrap();
            let project = store.project_by_root(&fixture.0).unwrap();
            let session = store
                .start_session(
                    project.id,
                    project.project_revision,
                    SessionDraft {
                        work_item_key: Some("EXAMPLE-001".into()),
                        agent_id: "fixture".into(),
                        provider: "fixture".into(),
                        model: "test".into(),
                        branch_id: None,
                        claim: true,
                        claim_ttl_ms: None,
                    },
                )
                .unwrap()
                .0
                .session;
            drop(store);
            let mut query = QueryProject::open(&fixture.0).unwrap();
            new_binding(
                &fixture.0,
                &mut query,
                "claude",
                "fixture-session",
                "EXAMPLE-001",
                Some(session.id),
                None,
                "test",
            )
            .unwrap();
            drop(query);
            fixture.git(&["init", "--quiet"]);
            fixture.commit();
            fixture
        }
        fn git(&self, args: &[&str]) {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fn commit(&self) {
            self.git(&[
                "-c",
                "user.name=Synthetic Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "Synthetic gate fixture",
            ]);
        }
        fn stop(&self, turn: &str, active: bool) -> HookEvent {
            ClaudeDialect.parse(&serde_json::to_vec(&json!({"session_id":"fixture-session","hook_event_name":"Stop","cwd":self.0,"turn_id":turn,"stop_hook_active":active})).unwrap()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn criterion_4_passing_completion_gate_blocks_once_per_commit_then_checkpoints() {
        let f = Fixture::new();
        let event = f.stop("turn-one", false);
        let check = |_: &Store, _: Id, _: &Session, _: &WorkItem, sha: &str| {
            assert!(is_source_sha(sha));
            Ok(true)
        };
        let (decision, receipt) = completion_gate(&f.0, "claude", &event, &ClaudeDialect, &check)
            .unwrap()
            .unwrap();
        assert_eq!(
            ClaudeDialect.render(&decision, &event).stdout["decision"],
            "block"
        );
        assert!(
            decision
                .reason()
                .unwrap()
                .contains("awr work complete EXAMPLE-001")
        );
        assert!(receipt["gate_event_id"].is_string());
        assert!(
            completion_gate(
                &f.0,
                "claude",
                &f.stop("turn-two", false),
                &ClaudeDialect,
                &check
            )
            .unwrap()
            .is_none()
        );
        let (second, metadata) =
            process_hook(&f.0, "claude", None, &event, &ClaudeDialect).unwrap();
        assert_eq!(second, HookDecision::allow());
        assert_eq!(metadata["checkpoint_saved"], true);
        f.commit();
        assert!(
            completion_gate(
                &f.0,
                "claude",
                &f.stop("turn-three", false),
                &ClaudeDialect,
                &check
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn criterion_4_active_stop_and_unsatisfied_checks_never_block() {
        let f = Fixture::new();
        assert!(
            completion_gate(
                &f.0,
                "claude",
                &f.stop("turn-one", true),
                &ClaudeDialect,
                &|_, _, _, _, _| panic!("recursive Stop must skip the gate")
            )
            .unwrap()
            .is_none()
        );
        assert!(
            completion_gate(
                &f.0,
                "claude",
                &f.stop("turn-two", false),
                &ClaudeDialect,
                &|_, _, _, _, _| Ok(false)
            )
            .unwrap()
            .is_none()
        );
        let store = Store::open_readonly(&f.0.join(".awr/state.db")).unwrap();
        let project = store.project_by_root(&f.0).unwrap();
        let gates = store
            .query_events(
                project.id,
                &awr_store::EventQuery {
                    event_type: Some("client.gate".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(gates.events.is_empty());
    }
}
