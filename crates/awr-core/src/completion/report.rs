//! Explain rejected reports without changing their serde contract or echoing submitted values.
use super::{CompletionCheck, CompletionReport};
use crate::{DiagnosticLocation, Error, SourceDiagnostic};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

pub(super) fn diagnostic(
    locator: &str,
    pointer: &str,
    rule: &str,
    message: &str,
    repair: &str,
) -> Box<SourceDiagnostic> {
    Box::new(SourceDiagnostic {
        details: None,
        message: message.into(),
        location: DiagnosticLocation {
            locator: Some(locator.into()),
            pointer: Some(pointer.into()),
            ..Default::default()
        },
        rule: rule.into(),
        repair: repair.into(),
    })
}

impl CompletionReport {
    /// Locate a validation failure in the original report, including aliases and struct sequences.
    /// Inspect bytes only on failure; this does not change parsing or validation decisions.
    pub fn locate_error(bytes: &[u8], mut error: Error) -> Error {
        if let Error::EvidenceDiagnostic(detail) = &mut error
            && let Some(pointer) = detail.location.pointer.as_deref()
        {
            detail.location.pointer = serde_json::from_slice::<Value>(bytes)
                .ok()
                .and_then(|value| source_pointer(&value, pointer));
        }
        error
    }

    /// Parse the original bytes. Diagnostic inspection only runs after serde rejects them.
    pub fn parse(bytes: &[u8], locator: &str) -> std::result::Result<Self, Box<SourceDiagnostic>> {
        serde_json::from_slice(bytes).map_err(|error| {
            // Value merges repeated keys; keep serde's location for duplicate-field rejections.
            if error.is_data()
                && !error.to_string().starts_with("duplicate field ")
                && let Ok(value) = serde_json::from_slice::<Value>(bytes)
                && let Some(detail) = shape_issue(&value, locator)
            {
                return detail;
            }
            // Unknown or duplicate fields and malformed JSON may not have a reliable pointer.
            // Serde columns count bytes; the public diagnostic uses Unicode character columns.
            let column = std::str::from_utf8(bytes).ok().and_then(|text| {
                let line = text.lines().nth(error.line().checked_sub(1)?)?;
                line.get(..error.column().checked_sub(1)?)
                    .map(|prefix| prefix.chars().count() + 1)
            });
            Box::new(SourceDiagnostic {
                details: None,
                message: "invalid completion report schema".into(),
                location: DiagnosticLocation {
                    locator: Some(locator.into()), pointer: None,
                    line: (error.line() > 0).then_some(error.line()), column,
                },
                rule: "completion.report.v1".into(),
                repair: "Check the JSON syntax and version 1 report structure, including duplicate fields; see docs/dev/reference/workflow.md for the completion report template.".into(),
            })
        })
    }
}

fn source_pointer(mut value: &Value, logical: &str) -> Option<String> {
    let mut pointer = String::new();
    for (depth, field) in logical.strip_prefix('/')?.split('/').enumerate() {
        let member = if value.is_array() && matches!(depth, 0 | 2) {
            // Field order is the existing serde sequence representation of these two structs.
            let fields: &[&str] = if depth == 0 {
                &[
                    "version",
                    "work_item",
                    "source_sha",
                    "command",
                    "scope",
                    "verified_at",
                    "checks",
                ]
            } else {
                &["name", "passed", "details", "criteria"]
            };
            fields.iter().position(|name| *name == field)?.to_string()
        } else if depth == 2
            && field == "details"
            && value.get("details").is_none()
            && value.get("result").is_some()
        {
            "result".into()
        } else {
            field.into()
        };
        value = if let Some(array) = value.as_array() {
            array.get(member.parse::<usize>().ok()?)?
        } else {
            value.get(&member)?
        };
        pointer.push('/');
        pointer.push_str(&member);
    }
    Some(pointer)
}

// Inspect only the report's known fields, using their actual Rust types. This is advice for
// an already rejected report, not an alternate validator. Extra producer metadata is ignored.
fn field<T: DeserializeOwned>(
    value: &Value,
    locator: &str,
    parent: &str,
    name: &str,
    expected: &str,
) -> Option<Box<SourceDiagnostic>> {
    let member = value.get(name);
    if member.is_some_and(|v| T::deserialize(v).is_ok()) {
        return None;
    }
    let (message, repair) = if member.is_none() {
        (
            format!("completion report field {name} is required"),
            format!("Provide {name} as {expected}; see the version 1 completion report template."),
        )
    } else {
        (
            format!("completion report field {name} has an invalid type or range"),
            format!("Use {expected} for {name}."),
        )
    };
    Some(diagnostic(
        locator,
        &format!("{parent}/{name}"),
        "completion.report.v1",
        &message,
        &repair,
    ))
}

fn shape_issue(value: &Value, locator: &str) -> Option<Box<SourceDiagnostic>> {
    // Struct sequence representations are still handled solely by serde.
    if !value.is_object() {
        return None;
    }
    let issue = field::<u32>(value, locator, "", "version", "an unsigned 32-bit integer")
        .or_else(|| field::<String>(value, locator, "", "work_item", "a string"))
        .or_else(|| field::<String>(value, locator, "", "source_sha", "a string"))
        .or_else(|| field::<String>(value, locator, "", "command", "a string"))
        .or_else(|| field::<Vec<String>>(value, locator, "", "scope", "an array of strings"))
        .or_else(|| {
            field::<i64>(
                value,
                locator,
                "",
                "verified_at",
                "a signed 64-bit Unix epoch milliseconds integer",
            )
        })
        .or_else(|| {
            if value.get("checks").is_some_and(Value::is_array) {
                None
            } else {
                field::<Vec<Value>>(value, locator, "", "checks", "an array of checks")
            }
        });
    if issue.is_some() {
        return issue;
    }
    for (index, check) in value["checks"].as_array()?.iter().enumerate() {
        if CompletionCheck::deserialize(check).is_ok() {
            continue;
        }
        let path = format!("/checks/{index}");
        if !check.is_object() {
            // Arrays can encode structs; do not incorrectly reject that existing representation.
            if check.is_array() {
                return None;
            }
            return Some(diagnostic(
                locator,
                &path,
                "completion.report.v1",
                "completion report check has an invalid type",
                "Use a check object with name, passed, details and acceptance criteria.",
            ));
        }
        let details = if check.get("details").is_none() && check.get("result").is_some() {
            "result"
        } else {
            "details"
        };
        return field::<String>(check, locator, &path, "name", "a string")
            .or_else(|| field::<bool>(check, locator, &path, "passed", "a boolean"))
            .or_else(|| field::<String>(check, locator, &path, details, "a string"))
            .or_else(|| {
                check.get("criteria").and_then(|_| {
                    field::<Vec<String>>(check, locator, &path, "criteria", "an array of strings")
                })
            });
    }
    None
}
