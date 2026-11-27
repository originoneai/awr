//! `github-pr-snapshot-v1`: projects a locally materialized GitHub pull
//! request snapshot as work items. Fetching stays outside AWR: a host or
//! script runs `gh pr list --json ...` (or an equivalent API call) and
//! registers the response bytes as a source. Every row stays byte-bound to
//! that snapshot and the adapter never contacts GitHub. The snapshot is
//! read-only; work actions cannot rewrite it. Canonical `status` follows the
//! built-in mapping below while the raw GitHub `state` is kept verbatim in
//! `raw_status`, and review signal is surfaced through `blocker` and
//! `next_action` so readers do not need the snapshot at hand.

use crate::{
    Locator, Manifest, ParseContext, SourceAdapter, SourceSnapshot, SourceSpec, fingerprint, limits,
};
use awr_core::{
    DiagnosticLocation, EntityKind, Error, ProjectionBatch, Result, SourceDiagnostic, WorkItem,
    WorkStatus,
};
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

pub struct GithubPrSnapshotAdapter;

fn invalid(pointer: &str, rule: &str, message: &str, repair: &str) -> Error {
    Error::InvalidSource(Box::new(SourceDiagnostic {
        details: None,
        message: message.into(),
        location: DiagnosticLocation {
            pointer: Some(pointer.into()),
            ..Default::default()
        },
        rule: rule.into(),
        repair: repair.into(),
    }))
}

fn string(value: &Value, pointer: &str) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(invalid(
            pointer,
            "github.string",
            "field must be a string",
            "Use a quoted string; null omits the field.",
        )),
    }
}

/// Names out of a gh list whose entries are plain strings or objects carrying
/// one key (`labels[].name`, `files[].path`).
fn nested_names(value: &Value, field: &str, pointer: &str) -> Result<Vec<String>> {
    match value {
        Value::Null => Ok(vec![]),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let pointer = format!("{pointer}/{index}");
                match item {
                    Value::String(name) if !name.trim().is_empty() => Ok(name.clone()),
                    Value::Object(_) => string(&item[field], &format!("{pointer}/{field}"))?
                        .filter(|name| !name.trim().is_empty())
                        .ok_or_else(|| {
                            invalid(
                                &format!("{pointer}/{field}"),
                                "github.nested_name",
                                "entry needs a nonempty name field",
                                "Keep objects with a name/path field, or plain nonempty strings.",
                            )
                        }),
                    _ => Err(invalid(
                        &pointer,
                        "github.nested_name",
                        "entry must be a nonempty string or an object",
                        "Keep objects with a name/path field, or plain nonempty strings.",
                    )),
                }
            })
            .collect(),
        _ => Err(invalid(
            pointer,
            "github.string_list",
            "field must be a list",
            "Use a JSON list, for example gh's labels or files arrays.",
        )),
    }
}

/// Identity `{owner}/{repo}#{number}` parsed from the pull request html URL,
/// so two repositories can share one snapshot (and one number) without
/// colliding. Works for GitHub Enterprise hosts with a path prefix because
/// only the trailing segments are interpreted.
fn external_key(url: &str, number: u64, pointer: &str) -> Result<String> {
    let bad = || {
        invalid(
            pointer,
            "github.url",
            "url is not a pull request html URL or its number differs from the number field",
            "Use the default url field from gh pr list --json url (https://host/owner/repo/pull/123).",
        )
    };
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| path)
        .ok_or_else(bad)?;
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let [.., owner, repo, kind, tail] = segments.as_slice() else {
        return Err(bad());
    };
    if kind != &"pull" || tail.parse::<u64>() != Ok(number) {
        return Err(bad());
    }
    Ok(format!("{owner}/{repo}#{number}"))
}

struct Mapped {
    status: WorkStatus,
    blocker: Option<String>,
    next_action: String,
    warning: Option<String>,
}

/// Canonical status from the GitHub state tuple. Unknown spellings stay
/// `unknown` with a diagnostic; no transition is ever guessed.
fn map_status(state: &str, is_draft: bool, review_decision: Option<&str>, pointer: &str) -> Mapped {
    let mapped = |status, blocker: Option<&str>, next_action: &str| Mapped {
        status,
        blocker: blocker.map(str::to_owned),
        next_action: next_action.into(),
        warning: None,
    };
    let unknown = |field: &str, value: &str| Mapped {
        status: WorkStatus::Unknown,
        blocker: None,
        next_action: String::new(),
        warning: Some(format!(
            "{pointer}/{field}: unknown value {value:?}; no transition inferred"
        )),
    };
    match state {
        "MERGED" => mapped(WorkStatus::Completed, None, ""),
        "CLOSED" => mapped(WorkStatus::Cancelled, None, ""),
        "OPEN" if is_draft => mapped(
            WorkStatus::Draft,
            None,
            "Mark the pull request ready for review.",
        ),
        "OPEN" => match review_decision.filter(|d| !d.is_empty()) {
            None | Some("REVIEW_REQUIRED") => mapped(
                WorkStatus::Blocked,
                Some("Waiting for review."),
                "Wait for review.",
            ),
            Some("CHANGES_REQUESTED") => {
                mapped(WorkStatus::InProgress, None, "Address the review feedback.")
            }
            Some("APPROVED") => mapped(
                WorkStatus::Blocked,
                Some("Approved; waiting for a maintainer to merge."),
                "Wait for merge.",
            ),
            Some(other) => unknown("reviewDecision", other),
        },
        other => unknown("state", other),
    }
}

/// Section fingerprint of the source facts. `updatedAt` is excluded: it moves
/// on any GitHub event without changing a mapped field, and hashing it would
/// break evidence bindings on every fetch. An empty `reviewDecision` is
/// normalized to null so equivalent snapshots hash equally.
fn section_fingerprint(entry: &Value) -> Result<String> {
    let mut facts = entry.clone();
    if let Some(object) = facts.as_object_mut() {
        object.remove("updatedAt");
        if object.get("reviewDecision").and_then(Value::as_str) == Some("") {
            object.insert("reviewDecision".into(), Value::Null);
        }
    }
    Ok(fingerprint(&serde_json::to_vec(&facts)?))
}

impl SourceAdapter for GithubPrSnapshotAdapter {
    fn name(&self) -> &'static str {
        "github-pr-snapshot-v1"
    }
    fn discover(
        &self,
        root: &Path,
        manifest: &Manifest,
        spec: &SourceSpec,
    ) -> Result<Vec<Locator>> {
        Ok(vec![Locator::from_spec(root, manifest, spec)?])
    }
    fn parse(
        &self,
        snapshot: &SourceSnapshot,
        context: &ParseContext<'_>,
        spec: &SourceSpec,
    ) -> Result<ProjectionBatch> {
        limits::check_source_size_only(&snapshot.bytes, limits::JSON_READ_CAP)?;
        if spec.domain != "ledger" {
            return Err(Error::InvalidInput(
                "github-pr-snapshot-v1 requires the ledger domain".into(),
            ));
        }
        if !spec.options.is_empty() {
            return Err(Error::InvalidInput(
                "github-pr-snapshot-v1 accepts no adapter options; the field and status mapping is built in".into(),
            ));
        }
        let document: Value = serde_json::from_str(snapshot.text()?).map_err(|error| {
            Error::InvalidSource(Box::new(SourceDiagnostic {
                details: None,
                message: "invalid JSON pull request snapshot".into(),
                location: DiagnosticLocation {
                    line: Some(error.line()),
                    column: Some(error.column()),
                    ..Default::default()
                },
                rule: "json.syntax".into(),
                repair: "Check brackets, commas and quotes at this location; the snapshot must be the JSON array emitted by gh.".into(),
            }))
        })?;
        // Decode from public-source-checked values; raw credentials never
        // reach rows, same contract as the YAML ledger decoder.
        snapshot.ensure_value(&document)?;
        let document = if awr_core::secrets_mode() == awr_core::SecretsMode::Warn {
            awr_core::redact_sensitive_value(document)
        } else {
            document
        };
        let entries = document.as_array().ok_or_else(|| {
            invalid(
                "",
                "github.root",
                "snapshot root must be a JSON array of pull requests",
                "Emit a top-level array, for example gh pr list --json ...; a search response wrapped in an items object is not accepted.",
            )
        })?;
        let mut batch = ProjectionBatch::default();
        let mut keys = BTreeSet::new();
        for (index, entry) in entries.iter().enumerate() {
            let pointer = format!("/{index}");
            if !entry.is_object() {
                return Err(invalid(
                    &pointer,
                    "github.record",
                    "entry must be an object",
                    "Each pull request is a JSON object such as {\"number\": 1, ...}.",
                ));
            }
            let number = entry["number"].as_u64().filter(|n| *n > 0).ok_or_else(|| {
                invalid(
                    &format!("{pointer}/number"),
                    "github.number",
                    "number must be a positive integer",
                    "Include number, for example gh pr list --json number,title,state,url.",
                )
            })?;
            let raw_state = string(&entry["state"], &format!("{pointer}/state"))?
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    invalid(
                        &format!("{pointer}/state"),
                        "github.state",
                        "state must be a nonempty string",
                        "Include state; GitHub emits OPEN, CLOSED or MERGED.",
                    )
                })?;
            let url = string(&entry["url"], &format!("{pointer}/url"))?
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    invalid(
                        &format!("{pointer}/url"),
                        "github.url",
                        "url must be a nonempty string",
                        "Include url; the identity owner/repo#number is parsed from it.",
                    )
                })?;
            let key = external_key(&url, number, &format!("{pointer}/url"))?;
            if !keys.insert(key.clone()) {
                return Err(Error::SourceConflict(format!(
                    "duplicate pull request key {key}"
                )));
            }
            let is_draft = match &entry["isDraft"] {
                Value::Null => false,
                Value::Bool(draft) => *draft,
                _ => {
                    return Err(invalid(
                        &format!("{pointer}/isDraft"),
                        "github.boolean",
                        "isDraft must be boolean",
                        "Use true or false without quotes.",
                    ));
                }
            };
            let review_decision = string(
                &entry["reviewDecision"],
                &format!("{pointer}/reviewDecision"),
            )?;
            let mapped = map_status(&raw_state, is_draft, review_decision.as_deref(), &pointer);
            if let Some(warning) = mapped.warning {
                batch.warnings.push(warning);
            }
            let mut meta = context.meta(
                EntityKind::WorkItem,
                &key,
                snapshot,
                Some(pointer.clone()),
                None,
            )?;
            meta.source_ref.section_fingerprint = Some(section_fingerprint(entry)?);
            batch.work_items.push(WorkItem {
                archived: false,
                ordinary_completion: None,
                meta,
                title: string(&entry["title"], &format!("{pointer}/title"))?
                    .unwrap_or_else(|| key.clone()),
                kind: Some("github-pr".into()),
                owner: match &entry["author"] {
                    Value::Null => None,
                    author => string(&author["login"], &format!("{pointer}/author/login"))?,
                },
                required: false,
                raw_status: raw_state,
                status: mapped.status,
                priority: None,
                milestone: match &entry["milestone"] {
                    Value::Null => None,
                    Value::String(_) => {
                        string(&entry["milestone"], &format!("{pointer}/milestone"))?
                    }
                    milestone => {
                        string(&milestone["title"], &format!("{pointer}/milestone/title"))?
                    }
                },
                score: None,
                evidence_level: None,
                summary: String::new(),
                next_action: mapped.next_action,
                blocker: mapped.blocker,
                acceptance: vec![],
                tags: nested_names(&entry["labels"], "name", &format!("{pointer}/labels"))?,
                paths: nested_names(&entry["files"], "path", &format!("{pointer}/files"))?,
                spec_refs: vec![],
                definition: None,
            });
        }
        Ok(batch)
    }
}
