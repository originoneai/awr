use crate::{
    query::short,
    records::check_limit,
    session::{RuntimeProject, event_brief},
};
use awr_core::*;
use awr_source::Manifest;
use awr_store::{BranchFilter, EventCursor, EventQuery, Store};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ObjectKind {
    Goal,
    Plan,
    Rule,
    #[value(alias = "work_item", alias = "work-item")]
    Work,
    Checkpoint,
}

#[derive(Debug, Subcommand)]
pub enum ObjectCommand {
    /// Traverse all indexed objects with bounded pages and revision-bound cursors.
    List(crate::catalog::ListArgs),
    /// Read one projected object by external key/ID, or an immutable checkpoint by ID.
    Show {
        #[arg(value_enum)]
        kind: ObjectKind,
        reference: String,
        #[arg(long)]
        full: bool,
        /// Use the last recorded projection when source refresh is unavailable.
        #[arg(long)]
        cached: bool,
        #[arg(long)]
        entity_revision: Option<Revision>,
        #[arg(long, default_value_t = 65536)]
        max_bytes: u64,
    },
}

#[derive(Debug, Args)]
pub struct HistoryWindow {
    #[arg(long, default_value_t = 0, conflicts_with = "cursor")]
    after_revision: Revision,
    #[arg(long)]
    through_revision: Option<Revision>,
    /// JSON next_cursor from the preceding page; repeat the same scope and upper bound.
    #[arg(long)]
    cursor: Option<String>,
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Debug, Subcommand)]
pub enum EventCommand {
    /// Append a caller event through the same validated domain operation as MCP.
    Append(crate::event_append::AppendArgs),
    /// Read immutable event metadata. --full explicitly includes its payload.
    Show {
        id: Id,
        #[arg(long)]
        full: bool,
        /// Maximum payload bytes for a full read.
        #[arg(long, default_value_t = 65536)]
        max_bytes: u64,
    },
    /// Page through bounded event summaries; bodies require event show --full.
    History {
        #[arg(long, conflicts_with = "source")]
        work: Option<String>,
        #[arg(long, conflicts_with = "source")]
        session: Option<Id>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        event_type: Option<String>,
        #[arg(long)]
        importance: Option<String>,
        #[arg(long, conflicts_with = "main")]
        branch: Option<Id>,
        #[arg(long)]
        main: bool,
        #[command(flatten)]
        window: HistoryWindow,
    },
}

fn print(value: &Value, text: &str, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{text}");
    }
    Ok(())
}

fn resolve_source(store: &Store, project: Id, reference: &str) -> Result<(Source, bool)> {
    let mut matches = BTreeMap::new();
    for source in store.sources(project)? {
        if source.domain == reference || source.locator == reference {
            matches.insert(source.id, (source, true));
        }
    }
    if let Ok(id) = reference.parse() {
        match store.retained_source(project, id) {
            Ok(source) => {
                matches.insert(id, source);
            }
            Err(Error::NotFound(_)) => (),
            Err(e) => return Err(e),
        }
    }
    if matches.len() > 1 {
        return Err(Error::InvalidInput(format!(
            "ambiguous source {reference}; specify one ID: {}",
            matches
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    matches
        .into_values()
        .next()
        .ok_or_else(|| Error::NotFound(format!("source {reference}")))
}

pub fn object(root: &Path, command: &ObjectCommand, json_output: bool) -> Result<()> {
    if let ObjectCommand::List(args) = command {
        return crate::catalog::list(root, args, json_output);
    }
    let ObjectCommand::Show {
        kind,
        reference,
        full,
        cached,
        entity_revision,
        max_bytes,
    } = command
    else {
        unreachable!()
    };
    let db = RuntimeProject::open(root, !cached && *kind != ObjectKind::Checkpoint)?;
    let mut checkpoint_save = Value::Null;
    let mut session_delta = Value::Null;
    let mut checkpoint_id = None;
    let (item, source, revision) = if *kind == ObjectKind::Checkpoint {
        let id = reference
            .parse::<Id>()
            .map_err(|e| Error::InvalidInput(format!("checkpoint ID: {e}")))?;
        let cp = db.store.checkpoint(db.project.id, id)?;
        checkpoint_id = Some(id);
        checkpoint_save = db.store.checkpoint_save_metadata(db.project.id, id)?;
        (serde_json::to_value(&cp)?, None, cp.revision)
    } else {
        let kind = match kind {
            ObjectKind::Goal => EntityKind::Goal,
            ObjectKind::Plan => EntityKind::Plan,
            ObjectKind::Rule => EntityKind::Rule,
            ObjectKind::Work => EntityKind::WorkItem,
            ObjectKind::Checkpoint => unreachable!(),
        };
        let projected = db.store.object_projection(db.project.id, kind, reference)?;
        let revision = projected.item["revision"]
            .as_u64()
            .ok_or_else(|| Error::Storage("projected object has no revision".into()))?;
        (projected.item, Some(projected.source), revision)
    };
    if let Some(expected) = entity_revision {
        if *expected != revision {
            return Err(Error::RevisionConflict {
                expected: *expected,
                actual: revision,
            });
        }
    }
    if *full {
        if let Some(id) = checkpoint_id {
            check_limit(
                serde_json::to_vec(&item)?.len() as u64
                    + checkpoint_save["delta_bytes"].as_u64().unwrap_or(0),
                *max_bytes,
            )?;
            session_delta =
                serde_json::to_value(db.store.checkpoint_saved_delta(db.project.id, id)?)?;
        }
    }
    let output = if *full {
        let size = serde_json::to_vec(&item)?.len()
            + if session_delta.is_null() {
                0
            } else {
                serde_json::to_vec(&session_delta)?.len()
            };
        check_limit(size as u64, *max_bytes)?;
        item
    } else if *kind == ObjectKind::Checkpoint {
        json!({"id":item["id"],"session_id":item["session_id"],"revision":revision,"project_revision":item["project_revision"],
            "context_hash":item["context_hash"],"digest":short(item["digest"].as_str().unwrap_or("")),"created_at":item["created_at"]})
    } else {
        json!({"id":item["id"],"external_key":item["external_key"],"title":item["title"],
            "revision":revision,"status":item["status"],"raw_status":item["raw_status"],
            "summary":short(item["summary"].as_str().or(item["text"].as_str()).unwrap_or("")),"severity":item["severity"],
            "scope":item["scope"],"source_ref":item["source_ref"]})
    };
    let mut value = db.metadata(db.project.project_revision);
    value["object"] = output;
    value["source"] = serde_json::to_value(source)?;
    value["content_included"] = json!(full);
    if *kind == ObjectKind::Checkpoint {
        value["checkpoint_save"] = checkpoint_save;
        if *full {
            value["session_delta"] = session_delta;
        }
    }
    db.check_revision()?;
    let text = if *kind == ObjectKind::Checkpoint && *full {
        serde_json::to_string_pretty(&value)?
    } else {
        serde_json::to_string_pretty(&value["object"])?
    };
    print(&value, &text, json_output)
}

fn history(
    db: &RuntimeProject,
    mut query: EventQuery,
    window: &HistoryWindow,
    json_output: bool,
) -> Result<()> {
    query.after_revision = window.after_revision;
    query.through_revision = window.through_revision;
    query.cursor = window
        .cursor
        .as_deref()
        .map(serde_json::from_str::<EventCursor>)
        .transpose()
        .map_err(|_| Error::InvalidInput("invalid event cursor JSON".into()))?;
    query.limit = window.limit;
    let page = db.store.query_events(db.project.id, &query)?;
    let mut value = db.metadata(page.project_revision);
    value["query"] = serde_json::to_value(&query)?;
    value["events"] = json!(page.events.iter().map(event_brief).collect::<Vec<_>>());
    value["next_cursor"] = serde_json::to_value(&page.next_cursor)?;
    value["payloads_included"] = json!(false);
    db.check_revision()?;
    let mut lines = page
        .events
        .iter()
        .map(|e| {
            format!(
                "r{} {} {} {} {}",
                e.project_revision,
                e.id,
                e.importance,
                e.event_type,
                short(&e.summary)
            )
        })
        .collect::<Vec<_>>();
    if let Some(cursor) = &page.next_cursor {
        lines.push(format!("next_cursor: {}", serde_json::to_string(cursor)?));
    }
    print(&value, &lines.join("\n"), json_output)
}

pub fn event(root: &Path, command: &EventCommand, json_output: bool) -> Result<()> {
    if let EventCommand::Append(args) = command {
        return crate::event_append::run(root, args, json_output);
    }
    let db = RuntimeProject::open(root, false)?;
    match command {
        EventCommand::Append(_) => unreachable!("append is handled before read-only dispatch"),
        EventCommand::Show {
            id,
            full,
            max_bytes,
        } => {
            let mut value = db.metadata(db.project.project_revision);
            value["event"] = if *full {
                check_limit(
                    db.store.event_payload_bytes(db.project.id, *id)?,
                    *max_bytes,
                )?;
                serde_json::to_value(db.store.event(db.project.id, *id)?)?
            } else {
                db.store.event_metadata(db.project.id, *id)?
            };
            value["payload_included"] = json!(full);
            db.check_revision()?;
            print(
                &value,
                &serde_json::to_string_pretty(&value["event"])?,
                json_output,
            )
        }
        EventCommand::History {
            work,
            session,
            source,
            event_type,
            importance,
            branch,
            main,
            window,
        } => history(
            &db,
            EventQuery {
                work_item_id: work
                    .as_deref()
                    .map(|w| db.store.work_identity(db.project.id, w))
                    .transpose()?,
                session_id: *session,
                source_id: source
                    .as_deref()
                    .map(|s| resolve_source(&db.store, db.project.id, s).map(|v| v.0.id))
                    .transpose()?,
                event_type: event_type.clone(),
                importance: importance.clone(),
                branch: branch.map(BranchFilter::Branch).unwrap_or(if *main {
                    BranchFilter::Main
                } else {
                    BranchFilter::Any
                }),
                ..Default::default()
            },
            window,
            json_output,
        ),
    }
}

pub fn source_history(
    root: &Path,
    reference: &str,
    window: &HistoryWindow,
    json_output: bool,
) -> Result<()> {
    let db = RuntimeProject::open(root, false)?;
    let source = resolve_source(&db.store, db.project.id, reference)?.0;
    history(
        &db,
        EventQuery {
            source_id: Some(source.id),
            ..Default::default()
        },
        window,
        json_output,
    )
}

#[derive(Debug, Args)]
pub struct SourceRead {
    pub reference: String,
    #[arg(long)]
    content: bool,
    /// Cap applies to the whole source read, even when a line range is selected.
    #[arg(long, default_value_t = 65536)]
    max_bytes: u64,
    #[arg(long)]
    source_revision: Option<Revision>,
    #[arg(long)]
    fingerprint: Option<String>,
    #[arg(long, requires_all = ["end_line", "content"])]
    start_line: Option<usize>,
    #[arg(long, requires_all = ["start_line", "content"])]
    end_line: Option<usize>,
}

pub fn source_show(root: &Path, request: &SourceRead, json_output: bool) -> Result<()> {
    let db = RuntimeProject::open(root, false)?;
    let (source, active) = resolve_source(&db.store, db.project.id, &request.reference)?;
    if let Some(expected) = request.source_revision {
        if source.revision != expected {
            return Err(Error::RevisionConflict {
                expected,
                actual: source.revision,
            });
        }
    }
    if request
        .fingerprint
        .as_ref()
        .is_some_and(|f| f != &source.fingerprint)
    {
        return Err(Error::SourceConflict(
            "source fingerprint differs from the supplied reference".into(),
        ));
    }
    let mut value = db.metadata(db.project.project_revision);
    value["source"] = serde_json::to_value(&source)?;
    value["active"] = json!(active);
    value["content_included"] = json!(request.content);
    value["freshness_basis"] = json!("last_recorded_source_state");
    if request.content {
        check_limit(0, request.max_bytes)?;
        if !active {
            return Err(Error::SourceUnavailable("retired source retains metadata and history; content requires a current source registration".into()));
        }
        let manifest = Manifest::load(root)?;
        // A retained DB locator is not permission to reopen a removed source.
        // Only rediscover the matching registered domain/adapter, never the project tree.
        let mut authorized = None;
        for spec in manifest
            .sources
            .iter()
            .filter(|spec| spec.domain == source.domain && spec.adapter == source.adapter)
        {
            for locator in
                awr_source::source_adapter(&spec.adapter)?.discover(root, &manifest, spec)?
            {
                let identity = if spec.adapter == "markdown-directory-v1" {
                    awr_source::MarkdownDirectoryAdapter
                        .source_identity(root, &manifest, spec, &locator)?
                } else {
                    locator.identity()?
                };
                if identity == source.locator {
                    authorized = Some(locator);
                    break;
                }
            }
            if authorized.is_some() {
                break;
            }
        }
        let locator = authorized.ok_or_else(|| {
            Error::SourceUnavailable("source is no longer registered for content reads".into())
        })?;
        let cap = request
            .max_bytes
            .min(awr_source::source_read_cap(&source.adapter)?);
        let snapshot = locator.read(root, cap)?;
        if snapshot.fingerprint != source.fingerprint {
            return Err(Error::SourceConflict(
                "source bytes changed since indexing; reindex and obtain a new reference".into(),
            ));
        }
        let content = match (request.start_line, request.end_line) {
            (Some(start), Some(end)) => snapshot.section_text(start, end)?,
            (None, None) => snapshot.text()?,
            _ => {
                return Err(Error::InvalidInput(
                    "both start and end lines are required".into(),
                ));
            }
        };
        value["content"] = json!(content);
        value["content_locator"] = json!(snapshot.locator);
        value["content_fingerprint_verified"] = json!(true);
        value["lines"] = json!(request.start_line.zip(request.end_line));
    }
    db.check_revision()?;
    let text = format!(
        "{} {} r{} {}\n{}",
        source.id,
        source.domain,
        source.revision,
        source.locator,
        value["content"]
            .as_str()
            .unwrap_or("Content omitted; use --content to read it.")
    );
    print(&value, &text, json_output)
}
