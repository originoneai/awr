use crate::{
    LedgerMapping, Locator, ParseContext, SourceAdapter, SourceSnapshot, YamlLedgerAdapter,
    fingerprint, inspect_registered_source,
};
use awr_core::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct PreparedWorkCreation {
    pub path: PathBuf,
    pub before: SourceSnapshot,
    pub after: SourceSnapshot,
    pub record: Value,
}

pub fn prepare_work_creation(
    root: &Path,
    source: &Source,
    external_key: &str,
    title: &str,
) -> Result<PreparedWorkCreation> {
    prepare_work_creation_with_fields(root, source, external_key, title, &Default::default())
}

pub fn prepare_work_creation_with_fields(
    root: &Path,
    source: &Source,
    external_key: &str,
    title: &str,
    input: &serde_json::Map<String, Value>,
) -> Result<PreparedWorkCreation> {
    if serde_json::to_vec(input)?.len() > 64 * 1024
        || input.keys().any(|k| {
            ![
                "summary",
                "goal",
                "acceptance",
                "spec_refs",
                "next_action",
                "kind",
                "paths",
                "tags",
                "priority",
                "depends_on",
                "owner",
                "milestone",
            ]
            .contains(&k.as_str())
        })
    {
        return Err(Error::InvalidInput("creation fields must be bounded source facts: summary, goal, acceptance, spec_refs, next_action, kind, paths, tags, priority, depends_on, owner, milestone; status remains draft".into()));
    }
    ensure_public_value(&Value::Object(input.clone()))?;
    if !["yaml-ledger-v1", "markdown-ledger-v1"].contains(&source.adapter.as_str())
        || source.domain != "ledger"
    {
        return Err(Error::MutationUnsupported(
            "new work requires a registered YAML ledger".into(),
        ));
    }
    if source.freshness != Freshness::Fresh {
        return Err(Error::SourceStale(
            "refresh the ledger before creating work".into(),
        ));
    }
    let (locator, spec, before) = inspect_registered_source(root, source)?;
    let Locator::File(path) = locator else {
        return Err(Error::MutationUnsupported(
            "Git sources remain read-only".into(),
        ));
    };
    if crate::is_runtime_owned_path(&root.canonicalize()?, &path) {
        return Err(Error::RuleViolation(
            "runtime-owned files cannot be task sources; relocate authored documents outside .awr (except .awr/intake) and reindex".into(),
        ));
    }
    if before.fingerprint != source.fingerprint {
        return Err(Error::SourceConflict(
            "ledger changed before preparing new work".into(),
        ));
    }
    let mapping = LedgerMapping::from_spec(&spec)?;
    let mut fields = vec![
        (mapping.source_field("id").to_owned(), json!(external_key)),
        (mapping.source_field("title").to_owned(), json!(title)),
        (
            mapping.source_field("status").to_owned(),
            mapping.write_value("status", &json!("draft"), &json!({}))?,
        ),
    ];
    for (field, value) in input {
        fields.push((
            mapping.source_field(field).to_owned(),
            mapping.write_value(field, value, &json!({}))?,
        ));
    }
    let record = Value::Object(fields.iter().cloned().collect());
    if source.adapter == "markdown-ledger-v1" {
        let mut output = crate::markdown_mutation::append_markdown_work(
            before.text()?,
            &spec,
            external_key,
            title,
        )?;
        if !input.is_empty() {
            output = crate::markdown_mutation::edit_markdown_fields(
                &output,
                &spec,
                external_key,
                input,
            )?;
        }
        crate::limits::check_source_size(output.as_bytes(), crate::MARKDOWN_READ_CAP)?;
        let after = SourceSnapshot {
            content_review: None,
            locator: before.locator.clone(),
            fingerprint: fingerprint(output.as_bytes()),
            bytes: output.into_bytes(),
        };
        let batch = crate::MarkdownLedgerAdapter.parse(
            &after,
            &ParseContext {
                source,
                existing_ids: BTreeMap::new(),
            },
            &spec,
        )?;
        if !batch
            .work_items
            .iter()
            .any(|w| w.meta.external_key == external_key && w.status == WorkStatus::Draft)
        {
            return Err(Error::MutationUnsupported(
                "new Markdown work must remain draft".into(),
            ));
        }
        return Ok(PreparedWorkCreation {
            path,
            before,
            after,
            record,
        });
    }
    let mut expected: Value = serde_yaml_ng::from_str(before.text()?)
        .map_err(|_| Error::InvalidInput("invalid YAML ledger".into()))?;
    match expected.get_mut("work_items") {
        Some(Value::Array(items)) => items.push(record.clone()),
        Some(Value::Object(items)) => {
            if items.insert(external_key.into(), record.clone()).is_some() {
                return Err(Error::SourceConflict("new task key already exists".into()));
            }
        }
        _ => {
            return Err(Error::MutationUnsupported(
                "work_items requires an explicit list or keyed map".into(),
            ));
        }
    }
    let output = crate::yaml_edit::append_work(before.text()?, external_key, &fields)?;
    crate::limits::check_source_size(output.as_bytes(), crate::YAML_READ_CAP)?;
    let actual: Value = serde_yaml_ng::from_str(&output).map_err(|_| {
        Error::MutationUnsupported("new record cannot preserve this YAML shape".into())
    })?;
    if actual != expected {
        return Err(Error::MutationUnsupported(
            "creation changed facts outside the new record".into(),
        ));
    }
    let after = SourceSnapshot {
        content_review: None,
        locator: before.locator.clone(),
        fingerprint: fingerprint(output.as_bytes()),
        bytes: output.into_bytes(),
    };
    // Parsing catches duplicate list keys, mapping conflicts and malformed domain fields.
    let batch = YamlLedgerAdapter.parse(
        &after,
        &ParseContext {
            source,
            existing_ids: BTreeMap::new(),
        },
        &spec,
    )?;
    if batch
        .work_items
        .iter()
        .filter(|w| w.meta.external_key == external_key)
        .count()
        != 1
    {
        return Err(Error::SourceConflict(
            "new task must have exactly one source identity".into(),
        ));
    }
    if batch
        .work_items
        .iter()
        .any(|w| w.meta.external_key == external_key && w.status != WorkStatus::Draft)
    {
        return Err(Error::MutationUnsupported("the source maps draft to an executable state; configure one unambiguous draft spelling before creation".into()));
    }
    Ok(PreparedWorkCreation {
        path,
        before,
        after,
        record,
    })
}
