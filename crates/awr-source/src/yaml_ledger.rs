use crate::{Locator, Manifest, ParseContext, SourceAdapter, SourceSnapshot, SourceSpec};
use awr_core::{
    DiagnosticLocation, Edge, EntityKind, Error, Evidence, EvidenceLevel, Goal, Id, Plan,
    ProjectionBatch, Result, SourceDiagnostic, SourceRef, WorkItem, WorkStatus,
};
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

pub struct YamlLedgerAdapter;
/// Explicit protocol opt-in. Older builds reject this adapter rather than
/// silently dropping ownership fields from an otherwise valid YAML ledger.
pub struct YamlWorkstreamLedgerAdapter;

impl SourceAdapter for YamlWorkstreamLedgerAdapter {
    fn name(&self) -> &'static str {
        "yaml-workstream-ledger-v1"
    }
    fn discover(
        &self,
        root: &Path,
        manifest: &Manifest,
        spec: &SourceSpec,
    ) -> Result<Vec<Locator>> {
        YamlLedgerAdapter.discover(root, manifest, spec)
    }
    fn parse(
        &self,
        snapshot: &SourceSnapshot,
        context: &ParseContext<'_>,
        spec: &SourceSpec,
    ) -> Result<ProjectionBatch> {
        YamlLedgerAdapter.parse(snapshot, context, spec)
    }
}

struct Entry<'a> {
    key: String,
    pointer: String,
    value: &'a Value,
}

fn invalid(pointer: &str, rule: &str, message: &str, repair: &str) -> Error {
    Error::InvalidSource(Box::new(SourceDiagnostic {
        message: message.into(),
        location: DiagnosticLocation {
            pointer: Some(pointer.into()),
            ..Default::default()
        },
        rule: rule.into(),
        repair: repair.into(),
    }))
}

fn string(value: &Value, field: &str) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(invalid(
            field,
            "ledger.string",
            "field must be a string",
            "Use a quoted string or a YAML block scalar (|) for multiline text.",
        )),
    }
}
fn strings(value: &Value, field: &str) -> Result<Vec<String>> {
    match value {
        Value::Null => Ok(vec![]),
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, v)| {
                let pointer = format!("{field}/{index}");
                string(v, &pointer)?.ok_or_else(|| {
                    invalid(
                        &pointer,
                        "ledger.nonnull_string",
                        "list contains null",
                        "Supply a string or remove the empty list entry.",
                    )
                })
            })
            .collect(),
        _ => Err(invalid(
            field,
            "ledger.string_list",
            "field must be a string or string list",
            "Use a string or a YAML list of strings; quote paths containing YAML punctuation.",
        )),
    }
}
fn boolean(value: &Value, default: bool, field: &str) -> Result<bool> {
    if value.is_null() {
        Ok(default)
    } else {
        value.as_bool().ok_or_else(|| {
            invalid(
                field,
                "ledger.boolean",
                "field must be boolean",
                "Use true or false without quotes.",
            )
        })
    }
}
fn alias<'a>(value: &'a Value, first: &str, second: &str, pointer: &str) -> Result<&'a Value> {
    if !value[first].is_null() && !value[second].is_null() && value[first] != value[second] {
        return Err(invalid(
            &format!("{pointer}/{}", escape(first)),
            "ledger.alias_conflict",
            &format!("conflicting {first} and {second}"),
            "Keep one authoritative field or make both aliases agree.",
        ));
    }
    Ok(if value[first].is_null() {
        &value[second]
    } else {
        &value[first]
    })
}
fn alias_pointer(value: &Value, pointer: &str, first: &str, second: &str) -> String {
    format!(
        "{pointer}/{}",
        escape(if value[first].is_null() {
            second
        } else {
            first
        })
    )
}

fn escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
fn entries<'a>(document: &'a Value, field: &str) -> Result<Vec<Entry<'a>>> {
    let raw: Vec<(Option<&str>, String, &Value)> = match &document[field] {
        Value::Null => vec![],
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| (None, format!("/{}/{i}", escape(field)), v))
            .collect(),
        Value::Object(items) => items
            .iter()
            .map(|(key, v)| {
                (
                    Some(key.as_str()),
                    format!("/{}/{}", escape(field), escape(key)),
                    v,
                )
            })
            .collect(),
        _ => {
            return Err(invalid(
                &format!("/{field}"),
                "ledger.collection",
                "collection must be a list or keyed map",
                "Use a YAML list of records or a map keyed by each record ID.",
            ));
        }
    };
    let mut keys = BTreeSet::new();
    raw.into_iter()
        .map(|(fallback, pointer, value)| {
            if !value.is_object() {
                return Err(invalid(
                    &pointer,
                    "ledger.record",
                    "record must be a mapping",
                    "Supply named fields such as id, title and status.",
                ));
            }
            let key = string(
                alias(value, "external_key", "id", &pointer)?,
                &alias_pointer(value, &pointer, "external_key", "id"),
            )?
            .or(fallback.map(str::to_owned))
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                invalid(
                    &pointer,
                    "ledger.identity",
                    "record needs an id or external_key",
                    "Give this record a nonempty stable id; keyed maps may use their map key.",
                )
            })?;
            if let Some(fallback) = fallback {
                if fallback != key {
                    return Err(Error::SourceConflict(format!(
                        "{pointer}: map key and entity key differ"
                    )));
                }
            }
            if !keys.insert(key.clone()) {
                return Err(Error::SourceConflict(format!(
                    "duplicate {field} key {key}"
                )));
            }
            Ok(Entry {
                key,
                pointer,
                value,
            })
        })
        .collect()
}

impl SourceAdapter for YamlLedgerAdapter {
    fn name(&self) -> &'static str {
        "yaml-ledger-v1"
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
        crate::limits::check_source_size_only(&snapshot.bytes, crate::YAML_READ_CAP)?;
        if spec.domain != "ledger" {
            return Err(Error::InvalidInput(
                "yaml-ledger-v1 requires the ledger domain".into(),
            ));
        }
        let mapping = crate::LedgerMapping::from_spec(spec)?;
        let result = Self::parse_document(
            snapshot,
            context,
            &mapping,
            spec.adapter == "yaml-workstream-ledger-v1",
        );
        result.map_err(|mut error| {
            if let Error::InvalidSource(diagnostic) = &mut error {
                diagnostic.location.locator = Some(snapshot.locator.clone());
                if let Some(pointer) = &mut diagnostic.location.pointer {
                    // Mapping applies only to work record fields, not goal or plan fields.
                    let mut parts: Vec<_> = pointer.split('/').map(str::to_owned).collect();
                    if parts.len() >= 4 && parts[1] == "work_items" {
                        let canonical = parts[3].replace("~1", "/").replace("~0", "~");
                        parts[3] = escape(mapping.source_field(&canonical));
                        *pointer = parts.join("/");
                    }
                    if diagnostic.location.line.is_none()
                        && let Ok(text) = snapshot.text()
                        && let Some((line, column)) =
                            crate::yaml_edit::pointer_location(text, pointer)
                    {
                        diagnostic.location.line = Some(line);
                        diagnostic.location.column = Some(column);
                    }
                }
            }
            error
        })
    }
}

impl YamlLedgerAdapter {
    fn parse_document(
        snapshot: &SourceSnapshot,
        context: &ParseContext<'_>,
        mapping: &crate::LedgerMapping,
        scoped: bool,
    ) -> Result<ProjectionBatch> {
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(snapshot.text()?)
        .map_err(|error| Error::InvalidSource(Box::new(SourceDiagnostic {
            message: "invalid YAML ledger document".into(),
            location: DiagnosticLocation {
                line: error.location().map(|p| p.line()),
                column: error.location().map(|p| p.column()),
                ..Default::default()
            },
            rule: "yaml.syntax".into(),
            repair: "Check indentation, matching brackets and quotes at this location; quote text containing ': ' or use a block scalar (|).".into(),
        })))?;
        let document = serde_json::to_value(yaml)?;
        Self::parse_decoded(snapshot, context, mapping, scoped, document)
    }

    /// Decode adapter-produced fields using the original byte-verified source.
    /// Never manufacture a snapshot that claims transformed bytes are original.
    pub(crate) fn parse_decoded(
        snapshot: &SourceSnapshot,
        context: &ParseContext<'_>,
        mapping: &crate::LedgerMapping,
        scoped: bool,
        document: Value,
    ) -> Result<ProjectionBatch> {
        snapshot.ensure_value(&document)?;
        let document = mapping.document(document)?;
        if !scoped && document.get("workstreams").is_some() {
            return Err(Error::Unsupported(
                "workstream declarations require yaml-workstream-ledger-v1".into(),
            ));
        }
        if !document.is_object()
            || !["work_items", "milestones", "goals"]
                .iter()
                .any(|key| document.get(key).is_some())
        {
            return Err(invalid(
                "",
                "ledger.root",
                "YAML ledger needs work_items, milestones or goals",
                "Add a root work_items, milestones or goals collection.",
            ));
        }
        let mut batch = ProjectionBatch::default();
        for Entry {
            key,
            pointer,
            value,
        } in entries(&document, "goals")?
        {
            let title = string(&value["title"], &format!("{pointer}/title"))?
                .unwrap_or_else(|| key.clone());
            batch.goals.push(Goal {
                meta: context.meta(
                    EntityKind::Goal,
                    &key,
                    snapshot,
                    Some(pointer.clone()),
                    None,
                )?,
                title,
                status: string(&value["status"], &format!("{pointer}/status"))?
                    .unwrap_or_else(|| "unknown".into()),
                priority: string(&value["priority"], &format!("{pointer}/priority"))?,
                summary: string(&value["summary"], &format!("{pointer}/summary"))?
                    .unwrap_or_default(),
                success_criteria: strings(
                    alias(value, "success_criteria", "acceptance", &pointer)?,
                    &alias_pointer(value, &pointer, "success_criteria", "acceptance"),
                )?,
            });
        }
        for Entry {
            key,
            pointer,
            value,
        } in entries(&document, "milestones")?
        {
            let title = string(&value["title"], &format!("{pointer}/title"))?
                .or(string(&value["name"], &format!("{pointer}/name"))?)
                .unwrap_or_else(|| key.clone());
            batch.plans.push(Plan {
                meta: context.meta(
                    EntityKind::Plan,
                    &key,
                    snapshot,
                    Some(pointer.clone()),
                    None,
                )?,
                title,
                status: string(&value["status"], &format!("{pointer}/status"))?
                    .unwrap_or_else(|| "unknown".into()),
                kind: Some("milestone".into()),
                summary: string(&value["summary"], &format!("{pointer}/summary"))?
                    .unwrap_or_default(),
                scope: strings(&value["scope"], &format!("{pointer}/scope"))?,
                acceptance: strings(
                    alias(value, "acceptance", "success_criteria", &pointer)?,
                    &alias_pointer(value, &pointer, "acceptance", "success_criteria"),
                )?,
            });
        }
        for Entry {
            key,
            pointer,
            value,
        } in entries(&document, "work_items")?
        {
            let raw_status =
                string(&value["status"], &format!("{pointer}/status"))?.unwrap_or_default();
            let status = mapping.status(&raw_status);
            if status == WorkStatus::Unknown {
                batch.warnings.push(format!(
                    "{pointer}/status: unknown raw status {raw_status:?}; no transition inferred"
                ));
            }
            let meta = context.meta(
                EntityKind::WorkItem,
                &key,
                snapshot,
                Some(pointer.clone()),
                None,
            )?;
            let direct_level = &value["evidence_level"];
            let nested_level = &value["verification"]["evidence_level"];
            if !direct_level.is_null() && !nested_level.is_null() && direct_level != nested_level {
                return Err(invalid(
                    &format!("{pointer}/evidence_level"),
                    "ledger.alias_conflict",
                    "conflicting evidence levels",
                    "Keep evidence_level and verification.evidence_level consistent.",
                ));
            }
            let raw_level = if direct_level.is_null() {
                nested_level
            } else {
                direct_level
            };
            let evidence_level = match string(
                raw_level,
                &format!(
                    "{pointer}/{}",
                    if direct_level.is_null() {
                        "verification/evidence_level"
                    } else {
                        "evidence_level"
                    }
                ),
            )? {
                None => None,
                Some(level) if level == "none" => None,
                Some(level) => Some(
                    serde_json::from_value(Value::String(level.clone())).unwrap_or_else(|_| {
                        batch
                            .warnings
                            .push(format!("{pointer}: unknown evidence level {level:?}"));
                        EvidenceLevel::Unknown
                    }),
                ),
            };
            let work = WorkItem {
                archived: boolean(&value["archived"], false, &format!("{pointer}/archived"))?,
                ordinary_completion: value
                    .get("ordinary_completion")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .map(|v| serde_json::from_value(v).map_err(|_| invalid(&format!("{pointer}/ordinary_completion"), "ledger.ordinary_completion", "ordinary completion record has invalid fields", "Use the versioned ordinary completion record returned by AWR; do not invent a completion record.")))
                    .transpose()?,
                meta,
                title: string(&value["title"], &format!("{pointer}/title"))?
                    .unwrap_or_else(|| key.clone()),
                kind: string(&value["kind"], &format!("{pointer}/kind"))?,
                owner: string(&value["owner"], &format!("{pointer}/owner"))?,
                required: boolean(
                    alias(value, "required", "required_for_v1", &pointer)?,
                    false,
                    &alias_pointer(value, &pointer, "required", "required_for_v1"),
                )?,
                raw_status,
                status,
                priority: string(&value["priority"], &format!("{pointer}/priority"))?,
                milestone: string(&value["milestone"], &format!("{pointer}/milestone"))?,
                score: if value["score"].is_null() {
                    None
                } else {
                    Some(value["score"].as_i64().ok_or_else(|| {
                        invalid(
                            &format!("{pointer}/score"),
                            "ledger.integer",
                            "score must be an integer",
                            "Use an unquoted integer.",
                        )
                    })?)
                },
                evidence_level,
                summary: string(&value["summary"], &format!("{pointer}/summary"))?
                    .unwrap_or_default(),
                next_action: string(&value["next_action"], &format!("{pointer}/next_action"))?
                    .unwrap_or_default(),
                blocker: string(&value["blocker"], &format!("{pointer}/blocker"))?,
                acceptance: strings(&value["acceptance"], &format!("{pointer}/acceptance"))?,
                tags: strings(&value["tags"], &format!("{pointer}/tags"))?,
                paths: strings(alias(value, "paths", "deliverables", &pointer)?, &alias_pointer(value, &pointer, "paths", "deliverables"))?,
            };
            let deps = alias(value, "depends_on", "dependencies", &pointer)?;
            let dependencies = if deps.is_null() {
                vec![]
            } else if let Some(a) = deps.as_array() {
                a.iter().collect()
            } else {
                return Err(invalid(
                    &alias_pointer(value, &pointer, "depends_on", "dependencies"),
                    "ledger.dependencies",
                    "dependencies must be a list",
                    "Use a list of work IDs, for example [WORK-1].",
                ));
            };
            let mut targets = BTreeSet::new();
            for (index, dependency) in dependencies.into_iter().enumerate() {
                let dependency_pointer = format!(
                    "{pointer}/{}/{index}",
                    if value.get("depends_on").is_some() {
                        "depends_on"
                    } else {
                        "dependencies"
                    }
                );
                let (target, required) = if dependency.is_object() {
                    (
                        string(
                            alias(dependency, "id", "key", &dependency_pointer)?,
                            &alias_pointer(dependency, &dependency_pointer, "id", "key"),
                        )?
                        .ok_or_else(|| {
                            missing_dependency_identity(dependency, &dependency_pointer)
                        })?,
                        boolean(
                            &dependency["required"],
                            true,
                            &format!("{dependency_pointer}/required"),
                        )?,
                    )
                } else {
                    (
                        string(dependency, &dependency_pointer)?.ok_or_else(|| {
                            invalid(
                                &dependency_pointer,
                                "ledger.dependency_identity",
                                "dependency cannot be null",
                                "Supply a stable work ID or remove the empty dependency.",
                            )
                        })?,
                        true,
                    )
                };
                if !targets.insert(target.clone()) {
                    return Err(Error::SourceConflict(format!(
                        "{pointer}: duplicate dependency {target}"
                    )));
                }
                let mut reference = work.meta.source_ref.clone();
                let field = if value.get("depends_on").is_some() {
                    "depends_on"
                } else {
                    "dependencies"
                };
                reference.pointer = Some(format!(
                    "{pointer}/{}/{index}",
                    escape(mapping.source_field(field))
                ));
                batch.edges.push(edge(
                    context,
                    &key,
                    "depends_on",
                    EntityKind::WorkItem,
                    &target,
                    required,
                    reference,
                ));
            }
            if let Some(milestone) = &work.milestone {
                let mut reference = work.meta.source_ref.clone();
                reference.pointer = Some(format!(
                    "{pointer}/{}",
                    escape(mapping.source_field("milestone"))
                ));
                batch.edges.push(edge(
                    context,
                    &key,
                    "part_of",
                    EntityKind::Plan,
                    milestone,
                    true,
                    reference,
                ));
            }
            for goal in strings(
                alias(value, "goal", "goals", &pointer)?,
                &alias_pointer(value, &pointer, "goal", "goals"),
            )? {
                let mut reference = work.meta.source_ref.clone();
                reference.pointer = Some(format!(
                    "{pointer}/{}",
                    escape(mapping.source_field(if value.get("goal").is_some() {
                        "goal"
                    } else {
                        "goals"
                    }))
                ));
                batch.edges.push(edge(
                    context,
                    &key,
                    "supports",
                    EntityKind::Goal,
                    &goal,
                    true,
                    reference,
                ));
            }
            parse_evidence(context, snapshot, value, &work, &pointer, &mut batch)?;
            batch.work_items.push(work);
        }
        if scoped {
            batch.workstream_projection = Some(parse_workstreams(&document, context, &batch)?);
        }
        Ok(batch)
    }
}

fn parse_workstreams(
    document: &Value,
    context: &ParseContext<'_>,
    batch: &ProjectionBatch,
) -> Result<awr_core::WorkstreamProjection> {
    use awr_core::{
        Workstream, WorkstreamCatalog, WorkstreamProjection, WorkstreamState,
        WorkstreamWorkBinding, validate_workstream_ownership,
    };
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Definition {
        id: Id,
        external_key: String,
        title: String,
        state: WorkstreamState,
        authority_version: u64,
        goal_keys: Vec<String>,
        acceptance_contracts: Vec<String>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Definitions {
        version: u32,
        legacy_default: Option<Id>,
        definitions: Vec<Definition>,
    }
    let definitions: Definitions = serde_json::from_value(document["workstreams"].clone()).map_err(|_| invalid(
        "/workstreams", "workstreams.catalog", "workstream definitions do not match the versioned schema",
        "Supply version, definitions and optional legacy_default; each definition needs a stable ID, key, title, state, authority_version, goal_keys and acceptance_contracts."
    ))?;
    let project_id = context.source.project_id.to_string();
    let catalog = WorkstreamCatalog {
        version: definitions.version,
        project_id: project_id.clone(),
        legacy_default: definitions.legacy_default,
        workstreams: definitions
            .definitions
            .into_iter()
            .map(|d| Workstream {
                id: d.id,
                project_id: project_id.clone(),
                external_key: d.external_key,
                title: d.title,
                state: d.state,
                authority_version: d.authority_version,
                goal_keys: d.goal_keys,
                acceptance_contracts: d.acceptance_contracts,
            })
            .collect(),
    };
    catalog.validate()?;
    let scopes = catalog
        .workstreams
        .iter()
        .map(|s| (s.external_key.as_str(), s.id))
        .collect::<std::collections::BTreeMap<_, _>>();
    let works = batch
        .work_items
        .iter()
        .map(|w| (w.meta.external_key.as_str(), w.meta.id.to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut ownership = Vec::new();
    for entry in entries(document, "work_items")? {
        let scope = entry.value["workstream"]
            .as_str()
            .and_then(|key| scopes.get(key))
            .ok_or_else(|| {
                invalid(
                    &format!("{}/workstream", entry.pointer),
                    "workstreams.ownership",
                    "work needs exactly one declared workstream key",
                    "Set workstream to the external_key of a definition in this ledger.",
                )
            })?;
        ownership.push(WorkstreamWorkBinding {
            project_id: project_id.clone(),
            workstream_id: *scope,
            work_item_id: works
                .get(entry.key.as_str())
                .ok_or_else(|| Error::SourceConflict("missing parsed work identity".into()))?
                .clone(),
        });
    }
    validate_workstream_ownership(
        &catalog,
        &works.into_values().collect::<Vec<_>>(),
        &ownership,
    )?;
    Ok(WorkstreamProjection { catalog, ownership })
}

fn edge(
    context: &ParseContext<'_>,
    key: &str,
    relation: &str,
    to_kind: EntityKind,
    target: &str,
    required: bool,
    source_ref: SourceRef,
) -> Edge {
    Edge {
        id: Id::new(),
        project_id: context.source.project_id,
        from_kind: EntityKind::WorkItem,
        from_key: key.into(),
        relation: relation.into(),
        to_kind,
        to_key: target.into(),
        required,
        revision: 1,
        source_ref,
    }
}

/// A plain list item such as `- Report.java: section 2` or `- WORK-1: waits for the API` is read by YAML as a mapping with one
/// key, not as text. True for such a mapping, i.e. one whose only key is not a field its position knows.
fn is_unquoted_text(item: &Value, known: &[&str]) -> bool {
    item.as_object().is_some_and(|fields| {
        fields.len() == 1 && fields.keys().all(|key| !known.contains(&key.as_str()))
    })
}

/// The diagnostic for a dependency mapping without an id. `- WORK-1: a note` arrives here looking like a dependency that
/// forgot its id although the author wrote one.
fn missing_dependency_identity(item: &Value, pointer: &str) -> Error {
    if is_unquoted_text(item, &["id", "key", "required"]) {
        // The rejected text itself is never echoed.
        return invalid(
            pointer,
            "ledger.dependency_identity",
            "dependency entry is a mapping without id or key; YAML reads an item such as `- WORK-1: a note` as a mapping",
            "Write the work ID alone (- WORK-1), or id: WORK-1 with an optional required: false; keep notes in the work's summary or next_action, or quote the whole entry if it is text.",
        );
    }
    invalid(
        pointer,
        "ledger.dependency_identity",
        "dependency needs id or key",
        "Supply a stable work ID.",
    )
}

/// The diagnostic for an evidence entry without a usable locator. A plain list item such as `- Report.java: section 2` is read
/// by YAML as a mapping with one key, not as text, so it arrives here looking like a missing locator although the author wrote one.
fn missing_locator(item: &Value, pointer: &str) -> Error {
    if is_unquoted_text(item, &["locator", "path", "summary"]) {
        // The rejected text itself is never echoed.
        return invalid(
            pointer,
            "ledger.evidence_locator",
            "evidence entry is a mapping without locator or path; YAML reads an unquoted `text: more text` item as a mapping",
            "Quote the whole entry, for example - \"report.md: section 2\", or write locator: report.md with an optional summary. Quote any entry that contains `: ` or ` #`.",
        );
    }
    invalid(
        pointer,
        "ledger.evidence_locator",
        "evidence reference needs a locator",
        "Supply the actual report path or URI.",
    )
}

fn parse_evidence(
    context: &ParseContext<'_>,
    snapshot: &SourceSnapshot,
    value: &Value,
    work: &WorkItem,
    pointer: &str,
    batch: &mut ProjectionBatch,
) -> Result<()> {
    let items = match &value["evidence"] {
        Value::Null => return Ok(()),
        Value::Array(items) => items,
        _ => {
            return Err(invalid(
                &format!("{pointer}/evidence"),
                "ledger.evidence",
                "evidence must be a list",
                "Use a list of report locators or objects containing locator.",
            ));
        }
    };
    let mut locators = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let item_pointer = format!("{pointer}/evidence/{index}");
        let location = if item.is_object() {
            alias(item, "locator", "path", &item_pointer)?
        } else {
            item
        };
        let locator = string(location, &item_pointer)?
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| missing_locator(item, &item_pointer))?;
        if !locators.insert(locator.clone()) {
            return Err(Error::SourceConflict(format!(
                "{pointer}: duplicate evidence locator {locator}"
            )));
        }
        let key = format!("{}/evidence/{locator}", work.meta.external_key);
        let meta = context.meta(
            EntityKind::Evidence,
            &key,
            snapshot,
            Some(format!("{pointer}/evidence/{index}")),
            None,
        )?;
        batch.evidence.push(Evidence {
            id: meta.id,
            project_id: context.source.project_id,
            work_item_id: Some(work.meta.id),
            external_key: key,
            evidence_type: "source_reference".into(),
            level: EvidenceLevel::Unknown,
            summary: if item.is_object() {
                string(&item["summary"], &format!("{item_pointer}/summary"))?
                    .unwrap_or_else(|| "Evidence reference; not yet verified by AWR".into())
            } else {
                "Evidence reference; not yet verified by AWR".into()
            },
            locator,
            sha256: None,
            source_sha: None,
            command: None,
            scope: vec![work.meta.external_key.clone()],
            source_ref: Some(meta.source_ref),
            branch_id: None,
            revision: 1,
            verified_at: None,
        });
    }
    Ok(())
}
