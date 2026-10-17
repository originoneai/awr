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
    path::Path,
};

#[derive(Debug, Args)]
pub struct Identity {
    /// Local hook dialect for the continuity record (`generic`, `codex` or `kimi`). This is not an authenticated MCP client principal.
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
        work: String,
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
    if !["codex", "kimi", "generic"].contains(&client)
        || external.trim().is_empty()
        || external.len() > 512
    {
        return Err(Error::InvalidInput(
            "client must be generic, or a documented native dialect (codex or kimi). Other hosts: use --client generic and prefix the conversation ID, e.g. --external-session cursor:<native-id>".into(),
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

fn native_hook(root: &Path, client: &str, work: &str) -> Result<Value> {
    let mut bytes = vec![];
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::InvalidInput("client event exceeds 1 MiB".into()));
    }
    let input: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::InvalidInput("invalid client event JSON".into()))?;
    let field = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or("");
    let external = field("session_id");
    validate_identity(client, external)?;
    let event = field("hook_event_name");
    if ![
        "SessionStart",
        "PreCompact",
        "PostCompact",
        "Stop",
        "SessionEnd",
        "Interrupt",
    ]
    .contains(&event)
    {
        return Err(Error::Unsupported(
            "unsupported client lifecycle event".into(),
        ));
    }
    if !Path::new(field("cwd")).canonicalize()?.starts_with(root) {
        return Err(Error::RuleViolation(
            "client event belongs to a different project".into(),
        ));
    }
    // An opt-in normalized host envelope, not fields assumed to exist in native hooks.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CompactionInput {
        observation: CompactionObservation,
        #[serde(default)]
        policy: CompactionPolicy,
    }
    let compaction: Option<CompactionInput> = input
        .get("awr_compaction")
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|_| Error::InvalidInput("invalid awr_compaction host envelope".into()))
        })
        .transpose()?;
    if let Some(c) = &compaction {
        if event != "PostCompact" {
            return Err(Error::InvalidInput(
                "awr_compaction is valid only after PostCompact".into(),
            ));
        }
        c.observation.validate(now_millis()?)?;
        c.policy.validate()?;
    }
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
    let model = if field("model").is_empty() {
        "not-reported"
    } else {
        field("model")
    };
    let mut binding = new_binding(root, &mut db, client, external, work, None, None, model)?;
    let compaction = compaction
        .map(|c| {
            let expected_revision = db.store.project(project)?.project_revision;
            awr_runtime::observe_compaction(
                &mut db.store,
                root,
                &awr_runtime::ObserveCompactionRequest {
                    session: binding.session_id,
                    expected_revision,
                    observation: c.observation,
                    policy: c.policy,
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
    let native_key = if !field("turn_id").is_empty() {
        Some(field("turn_id"))
    } else {
        None
    };
    let basis = json!([
        client,
        external,
        event,
        native_key,
        field("trigger"),
        input.get("stop_hook_active"),
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
        ClientCommand::Hook { client, work } => native_hook(&root, client, work)?,
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
        value.as_object_mut().unwrap().remove("awr");
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
