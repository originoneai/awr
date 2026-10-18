use crate::{query::short, session::RuntimeProject};
use awr_core::*;
use awr_runtime::{ARTIFACT_IMPORT_CAP, ArtifactFile, REGISTERED_CONTENT_READ_CAP, Runtime};
use clap::Subcommand;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Subcommand)]
pub enum EvidenceCommand {
    /// Register a JSON EvidenceDraft. Command and verification fields are assertions, never executed.
    ///
    /// The `--input` JSON object fields:
    /// - `work_item_key` — external key of the work item to bind.
    /// - `external_key` — this evidence's own key. **Required**.
    /// - `evidence_type`, `summary`, `locator` (path of the report file) —
    ///   **required**; `level` (e.g. `locally_verified`).
    /// - `sha256`, `source_sha` — full lowercase hex of the report/ledger.
    /// - `command`, `scope` (non-empty string array) + `verified_at`
    ///   (epoch milliseconds). `command` is an assertion, never executed.
    /// - `branch_id` — internal id; explicit `null` = main branch,
    ///   omitted = current branch.
    /// Receipt returns `evidence` (stored record incl. `id`), `event_id` and
    /// `validation_basis=caller_supplied_bindings`.
    Add {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        expected_revision: Revision,
    },
    /// Show metadata by external key or internal ID. Report content is opt-in.
    Show {
        id: String,
        #[arg(long)]
        source_sha: Option<String>,
        #[arg(long)]
        content: bool,
        #[arg(long, default_value_t = 65536)]
        max_bytes: u64,
    },
}
#[derive(Debug, Subcommand)]
pub enum DecisionCommand {
    /// Default: summary and source reference. --full includes the decision and rationale.
    Show {
        id: String,
        #[arg(long)]
        full: bool,
        #[arg(long, default_value_t = 65536)]
        max_bytes: u64,
    },
}
#[derive(Debug, Subcommand)]
pub enum ArtifactCommand {
    /// Copy a file to managed storage with its SHA256 and originating event.
    Add {
        path: PathBuf,
        #[arg(long = "type")]
        artifact_type: String,
        #[arg(long)]
        mime: String,
        #[arg(long)]
        source_event: Id,
        #[arg(long, default_value_t = ARTIFACT_IMPORT_CAP)]
        max_bytes: u64,
        #[arg(long)]
        expected_revision: Revision,
    },
    /// Read metadata only; never opens the artifact body.
    Show { id: Id },
    /// Read explicitly within a byte limit. Plain output is raw bytes; JSON requires UTF-8.
    Cat {
        id: Id,
        #[arg(long, default_value_t = 65536)]
        max_bytes: u64,
    },
}

fn evidence_brief(record: &EvidenceRecord) -> Value {
    let item = &record.item;
    json!({"id":item.id,"external_key":item.external_key,"type":item.evidence_type,"level":item.level,"summary":short(&item.summary),"locator":item.locator,"sha256":item.sha256,"source_sha":item.source_sha,"command":item.command.as_deref().map(short),"scope":item.scope.iter().take(20).collect::<Vec<_>>(),"scope_total":item.scope.len(),"work_item_id":item.work_item_id,"branch_id":item.branch_id,"verified_at":item.verified_at,"source_ref":item.source_ref,"freshness":record.source.as_ref().map(|s|s.freshness),"revision":item.revision})
}
/// Report how a stored locator is meant to be resolved.
fn locator_scheme(locator: &str) -> &'static str {
    if locator.starts_with("project://") {
        "project"
    } else if locator.starts_with("git://") {
        "git"
    } else if locator.starts_with("file://") {
        "file"
    } else {
        "path"
    }
}
fn print(value: &Value, text: &str, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{text}");
    }
    Ok(())
}
pub(crate) fn check_limit(size: u64, limit: u64) -> Result<()> {
    if limit == 0 || limit > REGISTERED_CONTENT_READ_CAP {
        return Err(Error::InvalidInput(
            "read limit must be 1..16777216 bytes".into(),
        ));
    }
    if size > limit {
        return Err(Error::InvalidInput(format!(
            "content has {size} bytes; read limit is {limit}"
        )));
    }
    Ok(())
}

pub fn evidence(root: &Path, command: &EvidenceCommand, json_output: bool) -> Result<()> {
    let mut db = match command {
        EvidenceCommand::Add {
            expected_revision, ..
        } => RuntimeProject::for_write(root, *expected_revision)?,
        EvidenceCommand::Show { .. } => RuntimeProject::open(root, true)?,
    };
    let project = db.project.id;
    match command {
        EvidenceCommand::Add {
            input,
            expected_revision,
        } => {
            let path = if input.is_absolute() {
                input.clone()
            } else {
                db.project.root.join(input)
            };
            if !path.metadata()?.is_file() {
                return Err(Error::InvalidInput(
                    "evidence input must be a regular JSON file".into(),
                ));
            }
            let file = File::open(path)?;
            check_limit(file.metadata()?.len(), 1024 * 1024)?;
            let mut bytes = Vec::new();
            file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
            check_limit(bytes.len() as u64, 1024 * 1024)?;
            awr_core::ensure_public_bytes(&bytes)?;
            let input: Value = serde_json::from_slice(&bytes)
                .map_err(|_| Error::InvalidInput("invalid evidence input JSON".into()))?;
            awr_core::ensure_public_value(&input)?;
            // EvidenceDraft is a domain type, so reject transport-only typos here.
            if let Some(fields) = input.as_object() {
                let allowed = [
                    "external_key",
                    "work_item_key",
                    "evidence_type",
                    "level",
                    "summary",
                    "locator",
                    "sha256",
                    "source_sha",
                    "command",
                    "scope",
                    "branch_id",
                    "verified_at",
                ];
                if fields.keys().any(|key| !allowed.contains(&key.as_str())) {
                    return Err(Error::InvalidInput("unknown evidence input field".into()));
                }
            }
            let declared_branch = input.get("branch_id").is_some();
            if input
                .get("verified_at")
                .is_some_and(|value| value.is_string())
            {
                return Err(Error::InvalidInput(
                    "verified_at must be Unix epoch milliseconds (a JSON integer, now_millis()), not a timestamp string".into(),
                ));
            }
            let mut draft: EvidenceDraft = serde_json::from_value(input).map_err(|error| {
                Error::InvalidInput(format!(
                    "evidence input does not match its schema: {}",
                    awr_core::redact_schema_error_detail(&error.to_string())
                ))
            })?;
            if !declared_branch {
                draft.branch_id = db.project.current_branch_id;
            }
            let (item, event) = Runtime::attach(&mut db.store, project)?
                .record_evidence(*expected_revision, draft)?;
            let mut value = db.metadata(event.project_revision);
            value["evidence"] = serde_json::to_value(&item)?;
            value["event_id"] = json!(event.id);
            value["validation_basis"] = json!("caller_supplied_bindings");
            print(
                &value,
                &format!(
                    "Evidence: {} ({})\nReport: {}\nRevision: {}\nVerification bindings recorded as supplied; no command executed.",
                    item.external_key, item.id, item.locator, event.project_revision
                ),
                json_output,
            )
        }
        EvidenceCommand::Show {
            id,
            source_sha,
            content,
            max_bytes,
        } => {
            if source_sha.as_deref().is_some_and(|s| !is_source_sha(s)) {
                return Err(Error::InvalidInput(
                    "--source-sha requires a full source SHA".into(),
                ));
            }
            let record = db.store.evidence(project, id)?;
            let assessment =
                record
                    .clone()
                    .assess(source_sha.as_deref(), None, db.project.current_branch_id);
            let mut value = db.metadata(db.project.project_revision);
            value["evidence"] = evidence_brief(&record);
            value["locator_scheme"] = json!(locator_scheme(&record.item.locator));
            value["currency"] = json!(assessment.currency);
            value["missing_bindings"] = json!(assessment.missing_bindings);
            value["reasons"] = json!(assessment.reasons);
            let body = if *content {
                let (_, bytes) = Runtime::attach(&mut db.store, project)?
                    .read_evidence_report(id, *max_bytes)?;
                let body = String::from_utf8(bytes).map_err(|_| {
                    Error::Unsupported(
                        "evidence report is not UTF-8 text; use the locator for binary content"
                            .into(),
                    )
                })?;
                value["content"] = json!(body);
                value["content_hash_verified"] = json!(record.item.sha256.is_some());
                Some(body)
            } else {
                None
            };
            db.check_revision()?;
            let mut text = format!(
                "Evidence: {} ({})\nSummary: {}\nReport: {}\nLocator scheme: {}\nCurrency: {}\nMissing bindings: {}",
                record.item.external_key,
                record.item.id,
                short(&record.item.summary),
                record.item.locator,
                locator_scheme(&record.item.locator),
                value["currency"],
                assessment.missing_bindings.join(", ")
            );
            if let Some(body) = body {
                text.push_str("\n\n");
                text.push_str(&body);
            }
            print(&value, &text, json_output)
        }
    }
}

pub fn decision(root: &Path, command: &DecisionCommand, json_output: bool) -> Result<()> {
    let db = RuntimeProject::open(root, true)?;
    match command {
        DecisionCommand::Show {
            id,
            full,
            max_bytes,
        } => {
            let decision = db.store.decision(db.project.id, id)?;
            let item = &decision.item;
            let mut value = db.metadata(db.project.project_revision);
            value["decision"] = if *full {
                let full = serde_json::to_value(item)?;
                check_limit(serde_json::to_vec(&full)?.len() as u64, *max_bytes)?;
                full
            } else {
                json!({"id":item.meta.id,"external_key":item.meta.external_key,"title":short(&item.title),"status":item.status,"adoption":item.adoption,"superseded_by":item.superseded_by,"summary":short(&item.decision),"affected_keys":item.affected_keys.iter().take(20).collect::<Vec<_>>(),"affected_keys_total":item.affected_keys.len(),"paths":item.paths.iter().take(20).collect::<Vec<_>>(),"paths_total":item.paths.len(),"revision":item.meta.revision})
            };
            value["source_ref"] = json!(item.meta.source_ref);
            value["freshness"] = json!(decision.source.freshness);
            db.check_revision()?;
            print(
                &value,
                &format!(
                    "Decision: {} ({})\nStatus: {}\n{}\nSource: {}",
                    item.meta.external_key,
                    item.meta.id,
                    item.raw_status,
                    if *full {
                        format!("{}\n\nRationale: {}", item.decision, item.rationale)
                    } else {
                        short(&item.decision)
                    },
                    item.meta.source_ref.locator
                ),
                json_output,
            )
        }
    }
}

pub fn artifact(root: &Path, command: &ArtifactCommand, json_output: bool) -> Result<()> {
    let mut db = RuntimeProject::open(root, false)?;
    let project = db.project.id;
    match command {
        ArtifactCommand::Add {
            path,
            artifact_type,
            mime,
            source_event,
            max_bytes,
            expected_revision,
        } => {
            let (artifact, event) = Runtime::attach(&mut db.store, project)?.import_artifact(
                *expected_revision,
                ArtifactFile {
                    path: path.clone(),
                    artifact_type: artifact_type.clone(),
                    mime: mime.clone(),
                    source_event_id: *source_event,
                    max_bytes: *max_bytes,
                },
            )?;
            let mut value = db.metadata(event.project_revision);
            value["artifact"] = json!(artifact);
            value["event_id"] = json!(event.id);
            print(
                &value,
                &format!(
                    "Artifact: {}\nBytes: {}\nSHA256: {}\nRevision: {}",
                    artifact.id, artifact.size, artifact.sha256, event.project_revision
                ),
                json_output,
            )
        }
        ArtifactCommand::Show { id } => {
            let artifact = db.store.artifact(project, *id)?;
            db.check_revision()?;
            let mut value = db.metadata(db.project.project_revision);
            value["artifact"] = json!(artifact);
            print(
                &value,
                &format!(
                    "Artifact: {}\nType: {}\nBytes: {}\nMIME: {}\nSHA256: {}\nLocator: {}\nSource event: {}",
                    artifact.id,
                    artifact.artifact_type,
                    artifact.size,
                    artifact.mime,
                    artifact.sha256,
                    artifact.locator,
                    artifact
                        .source_event_id
                        .map(|id| id.to_string())
                        .unwrap_or_default()
                ),
                json_output,
            )
        }
        ArtifactCommand::Cat { id, max_bytes } => {
            let (artifact, bytes) =
                Runtime::attach(&mut db.store, project)?.read_artifact(*id, *max_bytes)?;
            db.check_revision()?;
            if json_output {
                let body=String::from_utf8(bytes).map_err(|_|Error::Unsupported("JSON artifact content requires UTF-8; use artifact cat without --json for raw bytes".into()))?;
                let mut value = db.metadata(db.project.project_revision);
                value["artifact"] = json!(artifact);
                value["content"] = json!(body);
                value["content_hash_verified"] = json!(true);
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                std::io::stdout().lock().write_all(&bytes)?;
            }
            Ok(())
        }
    }
}
