use crate::{markdown_records::*, *};
use awr_core::*;
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, ops::Range, path::Path};

fn unsupported(s: &str) -> Error {
    Error::MutationUnsupported(s.into())
}
fn locate(text: &str, spec: &SourceSpec, target: &str) -> Result<Row> {
    let mut matches = rows(text, spec)?
        .into_iter()
        .filter(|r| r.values.get("id") == Some(&json!(target)))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(unsupported(
            "Markdown writes require one explicit stable ID; add an ID before editing",
        ));
    }
    Ok(matches.pop().unwrap())
}
pub fn read_markdown_mutation_record(
    root: &Path,
    source: &Source,
    patch: &MutationPatch,
) -> Result<Value> {
    verify_mutation_source(root, source, patch)?;
    let (_, spec, snapshot) = inspect_mutation_source(root, source, patch)?;
    Ok(Value::Object(
        locate(snapshot.text()?, &spec, &patch.target.meta.external_key)?.values,
    ))
}
fn trim_range(text: &str, range: Range<usize>) -> Range<usize> {
    let s = &text[range.clone()];
    let start = range.start + s.len() - s.trim_start().len();
    start..(range.start + s.trim_end().len()).max(start)
}
fn visible_range(text: &str, range: Range<usize>) -> Result<Range<usize>> {
    let s = &text[range.clone()];
    let end = range.start + s.find("<!--").unwrap_or(s.len());
    let trailing = &text[end..range.end];
    if trailing.contains("<!--") && !trailing.trim_start().starts_with("<!-- awr:") {
        return Err(unsupported(
            "editing this field would discard an unrelated HTML comment",
        ));
    }
    Ok(trim_range(text, range.start..end))
}
pub(crate) fn edit_markdown_fields(
    text: &str,
    spec: &SourceSpec,
    target: &str,
    changes: &Map<String, Value>,
) -> Result<String> {
    let row = locate(text, spec, target)?;
    let mut expected: Vec<_> = rows(text, spec)?.into_iter().map(|r| r.values).collect();
    let idx = expected
        .iter()
        .position(|r| r.get("id") == Some(&json!(target)))
        .unwrap();
    let mut edits = vec![];
    let mut extra = String::new();
    let raw = &text[row.range.clone()];
    let (columns, insertion, title, checkbox) = if let Some(headers) = &row.table {
        let cells = table_cells(text, row.range.clone())?;
        if cells.len() != headers.len() {
            return Err(unsupported("unequal table cells require manual editing"));
        }
        let title = headers.iter().position(|k| k == "title").unwrap();
        (
            headers
                .iter()
                .cloned()
                .zip(cells.clone())
                .collect::<BTreeMap<_, _>>(),
            trim_range(text, cells[title].clone()).end,
            visible_range(text, cells[title].clone())?,
            None,
        )
    } else {
        if ![
            "- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "* [X] ", "+ [ ] ", "+ [x] ",
            "+ [X] ",
        ]
        .iter()
        .any(|p| raw.starts_with(p))
        {
            return Err(unsupported(
                "checklist writes require one top-level single-line task",
            ));
        }
        let title = visible_range(text, row.range.start + 6..row.range.end)?;
        // Multiline list items remain readable but cannot be rewritten at a guessed boundary.
        let parsed = Parser::new(&text[title.clone()])
            .filter_map(|e| match e {
                pulldown_cmark::Event::Text(v) | pulldown_cmark::Event::Code(v) => {
                    Some(v.to_string())
                }
                _ => None,
            })
            .collect::<String>();
        if row.values["title"] != parsed.trim() {
            return Err(unsupported(
                "multiline or complex checklist title requires manual editing",
            ));
        }
        (
            BTreeMap::new(),
            row.range.start + raw.trim_end().len(),
            title,
            Some(row.range.start + 3..row.range.start + 4),
        )
    };
    for (field, value) in changes {
        if row.values.get(field) == Some(value) {
            continue;
        }
        expected[idx].insert(field.clone(), value.clone());
        if let Some((_, range)) = row.metadata.iter().find(|(k, _)| k == field) {
            edits.push((range.clone(), metadata_value(value)?));
        } else if field == "title" || columns.contains_key(field) {
            let range = if field == "title" {
                title.clone()
            } else {
                visible_range(text, columns[field].clone())?
            };
            let original = &text[range.clone()];
            let output =
                if original.starts_with('`') && original.ends_with('`') && original.len() > 1 {
                    let v = value
                        .as_str()
                        .ok_or_else(|| unsupported("code-styled cells require text"))?;
                    if v.contains(['`', '\n', '\r']) {
                        return Err(unsupported("replacement cannot preserve this code span"));
                    }
                    format!("`{}`", v.replace('|', "\\|"))
                } else {
                    escaped(value)?
                };
            edits.push((range, output));
        } else {
            extra += &format!(" <!-- awr:{field}={} -->", metadata_value(value)?);
        }
        if field == "status" {
            if let Some(marker) = &checkbox {
                let mapping = LedgerMapping::from_spec(spec)?;
                let done =
                    crate::markdown_ledger::normalized(value.as_str().unwrap_or(""), &mapping)
                        == WorkStatus::Completed;
                edits.push((marker.clone(), if done { "x" } else { " " }.into()));
            }
        }
    }
    if !extra.is_empty() {
        edits.push((insertion..insertion, extra));
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
    let mut output = text.to_owned();
    let mut end = text.len();
    for (r, v) in edits {
        if r.end > end {
            return Err(unsupported("overlapping Markdown edit spans"));
        }
        end = r.start;
        output.replace_range(r, &v);
    }
    let actual: Vec<_> = rows(&output, spec)?.into_iter().map(|r| r.values).collect();
    if actual != expected {
        return Err(unsupported(
            "Markdown edit would change other declared fields or rows",
        ));
    }
    Ok(output)
}
use pulldown_cmark::Parser;
pub(crate) fn append_markdown_work(
    text: &str,
    spec: &SourceSpec,
    external_key: &str,
    title: &str,
) -> Result<String> {
    let original = rows(text, spec)?;
    if original
        .iter()
        .any(|r| key(r).is_ok_and(|k| k == external_key))
    {
        return Err(Error::SourceConflict(
            "new Markdown ID already exists".into(),
        ));
    }
    let table_count = Parser::new_ext(text, pulldown_cmark::Options::ENABLE_TABLES)
        .filter(|e| {
            matches!(
                e,
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::Table(_))
            )
        })
        .count();
    let last = original.last().unwrap();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let record = if table_count == 1 && original.iter().all(|r| r.table.is_some()) {
        let headers = last.table.as_ref().unwrap();
        if !headers.iter().any(|h| h == "id") || !headers.iter().any(|h| h == "status") {
            return Err(unsupported(
                "table creation needs explicit id, title and status columns",
            ));
        }
        table_cells(text, last.range.clone())?;
        let mapping = LedgerMapping::from_spec(spec)?;
        let draft = mapping.write_value("status", &json!("draft"), &json!({}))?;
        let cells = headers
            .iter()
            .map(|h| {
                escaped(&match h.as_str() {
                    "id" => json!(external_key),
                    "title" => json!(title),
                    "status" => draft.clone(),
                    _ => json!(""),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        format!("| {} |{newline}", cells.join(" | "))
    } else if table_count == 0
        && original.iter().all(|r| r.table.is_none())
        && original.windows(2).all(|r| r[1].line <= r[0].line + 2)
    {
        let bullet = text[last.range.clone()]
            .chars()
            .next()
            .filter(|c| matches!(c, '-' | '*' | '+'))
            .ok_or_else(|| unsupported("creation requires a top-level checklist"))?;
        format!(
            "{bullet} [ ] {} <!-- awr:id={} --> <!-- awr:status=\"draft\" -->{newline}",
            escaped(&json!(title))?,
            metadata_value(&json!(external_key))?
        )
    } else {
        return Err(unsupported(
            "creation requires one unambiguous table or contiguous checklist; select a separate source for another group",
        ));
    };
    let at = if text[last.range.end..].starts_with('\n') {
        last.range.end + 1
    } else {
        last.range.end
    };
    let mut output = text.to_owned();
    output.insert_str(
        at,
        &format!(
            "{}{record}",
            if at > 0 && !text[..at].ends_with('\n') {
                newline
            } else {
                ""
            }
        ),
    );
    let after = rows(&output, spec)?;
    if after.len() != original.len() + 1
        || original
            .iter()
            .zip(&after)
            .any(|(a, b)| a.values != b.values)
    {
        return Err(unsupported(
            "creation would change previous Markdown records",
        ));
    }
    Ok(output)
}
pub fn prepare_markdown_mutation(
    root: &Path,
    source: &Source,
    proposal: &MutationProposal,
    ids: BTreeMap<(EntityKind, String), Id>,
) -> Result<PreparedYamlMutation> {
    let patch = proposal.bound_patch()?;
    if patch.target.kind != EntityKind::WorkItem {
        return Err(unsupported(
            "Markdown ledger writer only edits work records",
        ));
    }
    if patch.target.meta.source_ref.pointer.as_deref()
        != Some(pointer(&patch.target.meta.external_key).as_str())
    {
        return Err(unsupported(
            "Markdown record has no exact stable ID pointer",
        ));
    }
    let (locator, spec, before) = inspect_mutation_source(root, source, &patch)?;
    let Locator::File(path) = locator else {
        return Err(unsupported("Git sources remain read-only"));
    };
    if crate::is_runtime_owned_path(&root.canonicalize()?, &path) {
        return Err(unsupported(
            "runtime-owned files cannot be work sources; relocate authored documents outside .awr (except registered .awr/intake sources) and reindex",
        ));
    }
    if before.fingerprint != proposal.base_fingerprint {
        return Err(Error::SourceConflict(
            "Markdown source changed before edit".into(),
        ));
    }
    let mut changes = patch.changes.as_object().unwrap().clone();
    let mapping = LedgerMapping::from_spec(&spec)?;
    if let Some(field) = changes
        .keys()
        .find(|f| !crate::yaml_mutation::mutation_field_writable(&patch, f))
    {
        return Err(unsupported(&format!(
            "field {field} requires its domain action"
        )));
    }
    let record =
        Value::Object(locate(before.text()?, &spec, &patch.target.meta.external_key)?.values);
    if let Some(binding) = patch
        .work_action
        .as_ref()
        .and_then(|a| a.completion.as_ref())
    {
        if patch.changes != completion_source_changes(&record, binding)? {
            return Err(Error::RuleViolation(
                "completion must preserve current verification fields".into(),
            ));
        }
    }
    if let Some(status) = changes.get_mut("status") {
        *status = mapping.write_value("status", status, &record)?;
    }
    let output = edit_markdown_fields(
        before.text()?,
        &spec,
        &patch.target.meta.external_key,
        &changes,
    )?;
    crate::limits::check_source_size(output.as_bytes(), MARKDOWN_READ_CAP)?;
    let after = SourceSnapshot {
        content_review: None,
        locator: before.locator.clone(),
        fingerprint: fingerprint(output.as_bytes()),
        bytes: output.into_bytes(),
    };
    let (_, hash) = parse_mutation_projection(source, &spec, &after, ids, &patch.target)?;
    let plan = MutationWritePlan {
        id: Id::new(),
        before_fingerprint: before.fingerprint.clone(),
        after_fingerprint: after.fingerprint.clone(),
        before_size: before.bytes.len() as u64,
        after_size: after.bytes.len() as u64,
        target_after_hash: hash,
    };
    plan.validate()?;
    Ok(PreparedYamlMutation {
        path,
        before,
        after,
        plan,
    })
}
