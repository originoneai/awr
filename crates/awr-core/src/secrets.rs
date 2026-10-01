//! Shared, versioned data-minimization policy. This detects recognizable credentials,
//! labelled secret values and environment dumps, not arbitrary unlabelled private data.
//! Rejection diagnostics never contain the matched key, value or surrounding text.
use crate::{Error, Result};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{borrow::Cow, cell::OnceCell, sync::LazyLock};

pub const SECRET_POLICY_VERSION: u32 = 7;
pub const SENSITIVE_CONTENT_WITHHELD: &str = "[sensitive content withheld]";
const REJECTION: &str =
    "sensitive content is not accepted; remove secret values or use explicit redacted placeholders";

static KNOWN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)(?:^|[^\p{L}\p{N}_])(?:sk-[a-z0-9_-]{16,}|gh[pousr]_[a-z0-9]{20,}|github_pat_[a-z0-9_]{20,}|",
        r"xox[baprs]-[a-z0-9-]{12,}|\b(?:AKIA|ASIA)[A-Z0-9]{16}\b|",
        r"\beyJ[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}|",
        r"-----BEGIN (?:[A-Z0-9]+ )?PRIVATE KEY-----|",
        r"[a-z][a-z0-9+.-]*://[^\s/@:]+:[^\s/@]+@)"
    ))
    .expect("fixed credential pattern")
});

static BEARER_AUTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^\p{L}\p{N}_])bearer[\s]+([a-z0-9+/_=-]+)")
        .expect("fixed Bearer pattern")
});

static BASIC_AUTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^\p{L}\p{N}_])basic[\s]+([a-z0-9+/_=-]+)")
        .expect("fixed Basic authentication pattern")
});

static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
    r"(?i)(?:^|[^\p{L}\p{N}_])(?:[a-z][a-z0-9_]*[_-])?",
    r"(?:api[\s_-]*key|access[\s_-]*token|refresh[\s_-]*token|id[\s_-]*token|",
    r"client[\s_-]*secret|password|passwd|pwd|secret|token|authorization|",
    r"private[\s_-]*prompt|密码|密碼|口令|令牌|密钥|密鑰|私有提示词|私有提示詞|私有[\s_-]*prompt)",
    r#"[\s\"'`]*[:=]"#
)).expect("fixed secret assignment pattern")
});

static ENV_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s;])(?:export[ \t]+)?[A-Z_][A-Z0-9_]{1,80}[ \t]*=[ \t]*")
        .expect("fixed environment assignment pattern")
});

// Only complete, value-free declarations qualify. A bare YAML field, object
// literal, initializer, literal type or arbitrary type expression does not.
static TYPE_DECLARATION: LazyLock<Regex> = LazyLock::new(|| {
    let primitive = r"(?:string|number|boolean|unknown|never|undefined|null)(?:\[\])?";
    let field = format!(
        r"(?:readonly[ \t]+)?[A-Za-z_$][\w$]*\??[ \t]*:[ \t]*{primitive}(?:[ \t]*\|[ \t]*{primitive})*[ \t]*"
    );
    Regex::new(&format!(
        r"(?m)^[ \t]*(?:export[ \t]+)?(?:declare[ \t]+)?(?:(?:interface|class)[ \t]+[A-Za-z_$][\w$]*|type[ \t]+[A-Za-z_$][\w$]*[ \t]*=)[ \t]*\{{\s*(?:{field}(?:[;,]|\r?\n)\s*)*(?:{field})?\s*\}}[ \t]*;?[ \t]*\r?$"
    )).expect("fixed value-free declaration pattern")
});

/// A diagnostic category is safe to disclose; matched text and key names are not.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveCategory {
    Credential,
    LabelledValue,
    EnvironmentDump,
    PrivatePrompt,
}
impl SensitiveCategory {
    fn rejection(self) -> String {
        format!("{REJECTION}; category={}", self.name())
    }
    fn name(self) -> &'static str {
        match self {
            Self::Credential => "credential",
            Self::LabelledValue => "labelled_value",
            Self::EnvironmentDump => "environment_dump",
            Self::PrivatePrompt => "private_prompt",
        }
    }
    // Placement guidance is category-specific: a real secret and a public schema
    // need opposite repairs. Diagnostics still never echo the matched key or value.
    pub(crate) fn repair(self) -> &'static str {
        match self {
            Self::Credential => {
                "Real credentials must not be stored or managed by AWR. Keep this file outside registered sources (remove it from the source manifest) and keep real values in env/secret management; inside AWR use only explicit placeholders such as ${VAR} or [redacted]."
            }
            Self::LabelledValue => {
                "If this is a real secret, it must not be stored or managed by AWR: keep the file outside registered sources and reference ${VAR} or [redacted] inside AWR. If this is a public schema, use a complete value-free interface/type declaration or a structured JSON/YAML schema; do not disable scanning. For unchanged suspected public file content, use awr intake review; content or policy changes require a new review."
            }
            Self::EnvironmentDump => {
                "Real environment dumps must not be stored or managed by AWR. Keep environment files outside registered sources; inside AWR reference variables as ${VAR} and describe command requirements in prose. For unchanged suspected public file content, use awr intake review; content or policy changes require a new review."
            }
            Self::PrivatePrompt => {
                "Private prompts must not be stored or managed by AWR. Keep them outside registered sources and reference their external storage location instead. For public examples misclassified as private, use awr intake review with explicit source-bound decisions."
            }
        }
    }
}

pub(crate) fn sensitive_rejection_details(message: &str) -> Option<Value> {
    sensitive_category_for_message(message).map(|category| {
        serde_json::json!({"policy_version": SECRET_POLICY_VERSION, "category": category,
            "next_action": category.repair()})
    })
}
pub(crate) fn sensitive_category_for_message(message: &str) -> Option<SensitiveCategory> {
    [
        SensitiveCategory::Credential,
        SensitiveCategory::LabelledValue,
        SensitiveCategory::EnvironmentDump,
        SensitiveCategory::PrivatePrompt,
    ]
    .into_iter()
    .find(|category| message == category.rejection())
}

// Environment dumps and explicit exports retain their boundary. An incidental
// assignment in a command/prose string is not an environment dump. Sensitive key
// labels and recognizable credentials are still checked independently everywhere.
fn environment_assignment(text: &str, matched: &str, start: usize) -> bool {
    if matched.trim_start().starts_with("export") {
        return true;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
    let prefix = text[line_start..start].trim();
    let line = text[line_start..].lines().next().unwrap_or("").trim();
    (prefix.is_empty() || prefix.chars().all(|c| matches!(c, '`' | '\'' | '"')))
        && !inline_command(line)
}
fn inline_command(line: &str) -> bool {
    let mut assignment = false;
    for word in line.split_whitespace() {
        if let Some((name, value)) = word.split_once('=') {
            if !env_name(name)
                || name != name.to_ascii_uppercase()
                || value.is_empty()
                || value.contains(['\'', '"', '`', ';'])
            {
                return false;
            }
            assignment = true;
        } else {
            return assignment
                && word.chars().any(char::is_alphabetic)
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c));
        }
    }
    false
}

// "completed native authorization: the user accepted ..." is a narrative event,
// unlike a line/header/JSON/YAML field named authorization. Only the former may
// carry prose. A single opaque word or an authentication scheme remains a value.
fn narrative_authorization(text: &str, matched: &str, start: usize, end: usize) -> bool {
    if matched.trim() != "authorization:" {
        return false;
    }
    let prefix = text[..start].rsplit('\n').next().unwrap_or("").trim();
    if !prefix.chars().last().is_some_and(char::is_alphabetic) {
        return false;
    }
    let rest = text[end..].trim_start().lines().next().unwrap_or("");
    let first = rest.split_whitespace().next().unwrap_or("");
    if first.to_ascii_lowercase().starts_with("bearer")
        || first.to_ascii_lowercase().starts_with("basic")
    {
        return false;
    }
    if first.is_ascii() {
        first.chars().all(char::is_alphabetic)
            && first.len() < 24
            && rest.split_whitespace().take(3).count() == 3
    } else {
        // A CJK sentence has explicit prose punctuation, unlike an opaque value.
        rest.chars().any(|c| matches!(c, '，' | '。' | '；' | '：'))
            && first
                .chars()
                .next()
                .is_some_and(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
    }
}

static PRIVATE_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?im)^[ \t]*(?:#{1,6}[ \t]+)?(?:private[ _-]*prompt|私有提示词|私有提示詞)[ \t]*[:=]?[ \t]*\r?$"
).expect("fixed private prompt heading pattern")
});

static ENV_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r#"(?i)(?:^|[^\p{L}\p{N}_./-])(?:env|environment|environment[ _-]*(?:variables|vars|dump)|环境变量|環境變數)[\s\"']*[:=]"#
).expect("fixed environment object pattern")
});

// A heading is context, not evidence of a dump. Keep flow containers and
// indented YAML mappings / assignment lists protected, but do not classify
// Markdown prose or version lists merely because they follow "Environment:".
fn environment_block_entry(
    text: &str,
    end: usize,
    strings: &OnceCell<Vec<(usize, usize)>>,
) -> Option<usize> {
    let rest = &text[end..];
    let value = rest.trim_start();
    if !assignment_has_value(text, end, strings) || definition_after_assignment(rest) {
        return None;
    }
    if value.starts_with(['{', '[']) {
        return Some(end + rest.len() - value.len());
    }
    if !rest[..rest.len() - value.len()].contains('\n') {
        return None;
    }
    let heading = text[..end].rsplit('\n').next().unwrap_or("");
    let heading_indent = heading.len() - heading.trim_start().len();
    let mut offset = end;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let start = offset + indent;
        offset += line.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let entry = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        let is_list = entry.len() != trimmed.len();
        let (key, value) = entry.split_once(['=', ':'])?;
        let separator = entry.as_bytes()[key.len()];
        // A version/prose list is not a dump merely because of its heading.
        // Uppercase variable-shaped colon entries remain review candidates,
        // including YAML sequences and mixed Markdown/YAML sections.
        let name = key.trim().trim_matches(['\'', '"', '`']);
        if !is_list && indent <= heading_indent {
            return None;
        }
        if !env_name(name) || (is_list && separator == b':' && name != name.to_ascii_uppercase()) {
            if is_list {
                continue;
            }
            return None;
        }
        if has_value(value) && !definition_after_assignment(value) {
            return Some(start + trimmed.len() - entry.len());
        }
        // A YAML value can start on a more deeply indented following line.
        // Do not mistake the next sibling field for this field's empty value.
        if value.trim().is_empty() {
            let mut next_offset = offset;
            for next in text[offset..].split_inclusive('\n') {
                let content = next.trim_start();
                let next_indent = next.len() - content.len();
                if !content.is_empty() && !content.starts_with('#') {
                    if next_indent > indent && has_value(content) {
                        return Some(next_offset + next_indent);
                    }
                    break;
                }
                next_offset += next.len();
            }
        }
    }
    None
}

fn folded_char(c: char) -> Option<char> {
    match c {
        '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0),
        '\u{3000}' => Some(' '),
        '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{206f}'
        | '\u{feff}' => None,
        _ => Some(c),
    }
}

/// Only detection is normalized; stored public content is never rewritten.
/// Decode JSON unicode escapes as well, including those embedded in Markdown.
fn normalized(text: &str) -> Cow<'_, str> {
    if !text.contains('\\') && !text.chars().any(|c| folded_char(c) != Some(c)) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(normalized_chars(text).map(|(_, c)| c).collect())
}

fn normalized_chars(text: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        loop {
            let (offset, mut c) = chars.next()?;
            if c == '\\' && chars.peek().is_some_and(|(_, c)| *c == 'u') {
                let mut candidate = chars.clone();
                candidate.next();
                let hex: String = candidate.by_ref().take(4).map(|(_, c)| c).collect();
                if hex.len() == 4
                    && hex.bytes().all(|b| b.is_ascii_hexdigit())
                    && let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(decoded) = char::from_u32(code)
                {
                    c = decoded;
                    chars = candidate;
                }
            }
            if let Some(c) = folded_char(c) {
                return Some((offset, c));
            }
        }
    })
}

fn placeholder(text: &str) -> bool {
    let text = text.trim();
    matches!(
        text.to_ascii_lowercase().as_str(),
        "" | "[redacted]" | "<redacted>" | "[withheld]" | "<withheld>" | "***"
    ) || (text.starts_with("${") && text.ends_with('}') && env_name(&text[2..text.len() - 1]))
        || (text.starts_with('$') && env_name(&text[1..]))
}
fn env_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !value.as_bytes()[0].is_ascii_digit()
}
const PUBLIC_LITERALS: &[&str] = &[
    "true", "True", "TRUE", "false", "False", "FALSE", "null", "Null", "NULL", "~", "{}", "[]",
];
fn public_literal_end(tail: &str) -> bool {
    // Quotes can begin a concatenated value; only verified source-string
    // envelopes may remove their closing quote before reaching this check.
    let tail = tail.trim_start_matches([')', ']', '}', '`']);
    tail.is_empty()
        || tail.starts_with(|c: char| {
            c.is_whitespace() || matches!(c, ',' | ';' | '、' | '。' | '，' | '；')
        })
        || tail
            .strip_prefix('.')
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}
fn has_value(rest: &str) -> bool {
    let rest = rest.trim_start();
    if rest.is_empty() {
        return false;
    }
    if PUBLIC_LITERALS
        .iter()
        .any(|literal| rest.strip_prefix(literal).is_some_and(public_literal_end))
    {
        return false;
    }
    // Markdown code delimiters preserve a literal; string quotes change its type.
    if rest
        .strip_prefix('`')
        .and_then(|tail| tail.split_once('`'))
        .is_some_and(|(value, tail)| {
            PUBLIC_LITERALS.contains(&value.trim()) && public_literal_end(tail)
        })
    {
        return false;
    }
    let value = if let Some(quote) = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '\'' | '"' | '`'))
    {
        let tail = &rest[quote.len_utf8()..];
        tail.split(quote).next().unwrap_or(tail)
    } else {
        rest.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'' | '`'))
            .next()
            .unwrap_or(rest)
    };
    // Nonempty strings and numbers remain values, even when a string spells a literal.
    !placeholder(value)
}

// Raw JSON/YAML strings can contain public notes ending in a scalar. Retain
// their verified enclosing quote without granting Shell concatenations the
// same exemption. Raw assignments are still scanned, including duplicate keys.
fn source_string_ranges(text: &str) -> Vec<(usize, usize)> {
    let value =
        serde_json::from_str::<Value>(text).or_else(|_| serde_yaml_ng::from_str::<Value>(text));
    if !value.is_ok_and(|value| value.is_object() || value.is_array()) {
        return Vec::new();
    }
    let mut chars = text.char_indices().peekable();
    let mut open = None;
    let mut ranges = Vec::new();
    while let Some((offset, c)) = chars.next() {
        match open {
            Some((_, '"')) if c == '\\' => {
                chars.next();
            }
            Some((_, '\'')) if c == '\'' && chars.peek().is_some_and(|(_, c)| *c == '\'') => {
                chars.next();
            }
            Some((start, quote)) if c == quote => {
                ranges.push((start, offset));
                open = None;
            }
            None if matches!(c, '\'' | '"') => open = Some((offset, c)),
            _ => {}
        }
    }
    ranges
}

fn assignment_has_value(text: &str, end: usize, strings: &OnceCell<Vec<(usize, usize)>>) -> bool {
    let rest = &text[end..];
    if !has_value(rest) {
        return false;
    }
    let candidate = rest.trim_start().trim_start_matches('`');
    if !PUBLIC_LITERALS
        .iter()
        .any(|literal| candidate.starts_with(literal))
    {
        return true;
    }
    strings
        .get_or_init(|| source_string_ranges(text))
        .iter()
        .find(|(start, close)| *start < end && end <= *close)
        .is_none_or(|(_, close)| has_value(&text[end..*close]))
}

pub fn sensitive_text_category(text: &str) -> Option<SensitiveCategory> {
    let text = normalized(text);
    sensitive_match(&text).map(|(category, _)| category)
}

/// Offset is in normalized text; callers must map it back before displaying it.
fn sensitive_match(text: &str) -> Option<(SensitiveCategory, usize)> {
    sensitive_matches(text, 1).into_iter().next()
}

// Keep category precedence stable for legacy rejection diagnostics. Review
// callers consume every finding, so accepting one cannot conceal a later one.
fn sensitive_matches(text: &str, limit: usize) -> Vec<(SensitiveCategory, usize)> {
    let mut found = Vec::new();
    macro_rules! record {
        ($category:expr, $offset:expr) => {
            found.push(($category, $offset));
            if found.len() >= limit {
                return found;
            }
        };
    }
    for m in KNOWN.find_iter(text) {
        record!(SensitiveCategory::Credential, m.start());
    }
    for capture in BEARER_AUTH.captures_iter(text) {
        let value = &capture[1];
        if value.len() >= 32
            || (value.len() >= 8
                && (value
                    .bytes()
                    .any(|b| b.is_ascii_digit() || b"+/_=-".contains(&b))
                    || (value.bytes().skip(1).any(|b| b.is_ascii_uppercase())
                        && value.bytes().any(|b| b.is_ascii_lowercase()))))
        {
            record!(
                SensitiveCategory::Credential,
                capture.get(0).unwrap().start()
            );
        }
    }
    for capture in BASIC_AUTH.captures_iter(text) {
        // RFC 7617 user-id:password, including omitted base64 padding.
        if STANDARD
            .decode(&capture[1])
            .or_else(|_| STANDARD_NO_PAD.decode(&capture[1]))
            .is_ok_and(|bytes| bytes.contains(&b':'))
        {
            record!(
                SensitiveCategory::Credential,
                capture.get(0).unwrap().start()
            );
        }
    }
    let declarations: Vec<_> = TYPE_DECLARATION.find_iter(text).collect();
    let strings = OnceCell::new();
    for m in ASSIGNMENT.find_iter(text).filter(|m| {
        assignment_has_value(text, m.end(), &strings)
            && !definition_after_assignment(&text[m.end()..])
            && !narrative_authorization(text, m.as_str(), m.start(), m.end())
            && !declarations
                .iter()
                .any(|d| d.start() <= m.start() && m.end() < d.end())
    }) {
        record!(SensitiveCategory::LabelledValue, m.start());
    }
    for m in ENV_ASSIGNMENT.find_iter(text).filter(|m| {
        environment_assignment(text, m.as_str(), m.start())
            && assignment_has_value(text, m.end(), &strings)
            && !definition_after_assignment(&text[m.end()..])
    }) {
        record!(SensitiveCategory::EnvironmentDump, m.start());
    }
    for m in PRIVATE_BLOCK
        .find_iter(text)
        .filter(|m| !placeholder(&text[m.end()..]))
    {
        record!(SensitiveCategory::PrivatePrompt, m.start());
    }
    for offset in ENV_BLOCK
        .find_iter(text)
        .filter_map(|m| environment_block_entry(text, m.end(), &strings))
    {
        record!(SensitiveCategory::EnvironmentDump, offset);
    }
    found
}
/// Findings use complete clause ranges so output composition cannot authorize
/// a match which merely starts inside an approved fragment.
pub fn content_text_ranges(text: &str) -> Vec<(SensitiveCategory, usize, usize, String)> {
    use sha2::{Digest, Sha256};
    let normalized = normalized(text);
    let mut offsets = Vec::new();
    for (original, c) in normalized_chars(text) {
        offsets.extend(std::iter::repeat_n(original, c.len_utf8()));
    }
    sensitive_matches(&normalized, 513)
        .into_iter()
        .map(|(category, start)| {
            let start = start + normalized[start..].len() - normalized[start..].trim_start().len();
            let tail = &normalized[start..];
            let first_line = tail.split('\n').next().unwrap_or("");
            let mut span_end = first_line.len();
            if category == SensitiveCategory::PrivatePrompt || tail.starts_with(['{', '[']) {
                span_end = tail.len();
            } else {
                if let Some(separator) = first_line.find([':', '=']) {
                    let rest = &tail[separator + 1..];
                    let value_start = separator + 1 + rest.len() - rest.trim_start().len();
                    span_end =
                        value_start + tail[value_start..].split('\n').next().unwrap_or("").len();
                }
                // Block scalars and continued mappings/quoted values belong to the
                // same finding; do not bind only the label or the scalar indicator.
                let line_start = normalized[..start].rfind('\n').map_or(0, |p| p + 1);
                let line = &normalized[line_start..];
                let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
                let mut offset = span_end;
                for line in tail[span_end..].split_inclusive('\n') {
                    let trimmed = line.trim_start_matches([' ', '\t']);
                    if !trimmed.trim().is_empty() && line.len() - trimmed.len() <= indent {
                        break;
                    }
                    offset += line.len();
                    span_end = offset;
                }
            }
            let clause = &tail[..span_end];
            let end = start + clause.trim_end().len();
            let original_start = offsets.get(start).copied().unwrap_or(text.len());
            let original_end = offsets.get(end).copied().unwrap_or(text.len());
            (
                category,
                original_start,
                original_end,
                format!(
                    "{:x}",
                    Sha256::digest(format!("{category:?}:{}", clause.trim_end()).as_bytes())
                ),
            )
        })
        .collect()
}
/// Review scan locations are safe metadata, never matched keys or values.
pub(crate) fn review_text_findings(text: &str) -> Vec<(SensitiveCategory, usize, usize, String)> {
    content_text_ranges(text)
        .into_iter()
        .map(|(category, start, _, signature)| {
            let prefix = &text[..start];
            (
                category,
                prefix.bytes().filter(|b| *b == b'\n').count() + 1,
                prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1,
                signature,
            )
        })
        .collect()
}

/// The hard credential boundary is independent of Agent review decisions.
pub fn ensure_no_credentials_text(text: &str) -> Result<()> {
    if sensitive_matches(&normalized(text), 1)
        .iter()
        .any(|(category, _)| *category == SensitiveCategory::Credential)
    {
        return Err(Error::RuleViolation(
            SensitiveCategory::Credential.rejection(),
        ));
    }
    Ok(())
}

pub fn sensitive_field_category(key: &str, value: &Value) -> Option<SensitiveCategory> {
    sensitive_field(key, value).then_some(if env_key(key) {
        SensitiveCategory::EnvironmentDump
    } else {
        SensitiveCategory::LabelledValue
    })
}

pub fn ensure_no_credentials_value(value: &Value) -> Result<()> {
    match value {
        Value::String(text) => ensure_no_credentials_text(text),
        Value::Array(values) => values.iter().try_for_each(ensure_no_credentials_value),
        Value::Object(values) => values.iter().try_for_each(|(key, value)| {
            ensure_no_credentials_text(key)?;
            ensure_no_credentials_value(value)
        }),
        _ => Ok(()),
    }
}

pub fn contains_sensitive_text(text: &str) -> bool {
    sensitive_text_category(text).is_some()
}

fn secret_key(key: &str) -> bool {
    let key = normalized(key);
    let lower = key.to_lowercase();
    let compact: String = lower
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '_' | '-'))
        .collect();
    [
        "apikey",
        "accesstoken",
        "refreshtoken",
        "idtoken",
        "clientsecret",
        "password",
        "passwd",
        "pwd",
        "secret",
        "token",
        "authorization",
        "privateprompt",
        "密码",
        "密碼",
        "口令",
        "令牌",
        "密钥",
        "密鑰",
        "私有提示词",
        "私有提示詞",
        "私有prompt",
    ]
    .contains(&compact.as_str())
        || [
            "_api_key",
            "_access_token",
            "_refresh_token",
            "_client_secret",
            "_password",
            "_passwd",
            "_secret",
            "_token",
        ]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}
fn env_key(key: &str) -> bool {
    let key: String = normalized(key)
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '_' | '-'))
        .collect();
    [
        "env",
        "environment",
        "environmentvariables",
        "environmentvars",
        "environmentdump",
        "环境变量",
        "環境變數",
    ]
    .contains(&key.as_str())
}
fn has_json_value(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) => false,
        Value::String(s) => !placeholder(s),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// A definition carries structure, never a credential value. Extra fields are not ignored:
/// default/example/enum values must be absent, empty or explicitly redacted. This applies
/// equally to JSON Schema, YAML schema fragments and OpenAPI authentication schemes.
fn public_definition(value: &Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    if !fields.contains_key("type") && !fields.contains_key("$ref") {
        return false;
    }
    fields.iter().all(|(key, value)| match key.as_str() {
        "type" => value.as_str().is_some_and(|s| {
            [
                "null",
                "boolean",
                "object",
                "array",
                "number",
                "string",
                "integer",
                "http",
                "apiKey",
                "oauth2",
                "openIdConnect",
            ]
            .contains(&s)
        }),
        "scheme" => value.as_str().is_some_and(|s| {
            ["bearer", "basic", "digest", "negotiate"].contains(&s.to_ascii_lowercase().as_str())
        }),
        "in" => value
            .as_str()
            .is_some_and(|s| ["header", "query", "cookie"].contains(&s)),
        "$ref" | "$schema" | "title" | "description" | "format" | "pattern" | "name"
        | "bearerFormat" | "openIdConnectUrl" => {
            value.as_str().is_some_and(|s| !contains_sensitive_text(s))
        }
        "properties" | "$defs" | "definitions" => value.as_object().is_some_and(|o| {
            o.iter()
                .all(|(k, v)| !contains_sensitive_text(k) && public_definition(v))
        }),
        "items" => public_definition(value),
        "additionalProperties" => value.is_boolean() || public_definition(value),
        "required" => value.as_array().is_some_and(|a| {
            a.iter()
                .all(|v| v.as_str().is_some_and(|s| !contains_sensitive_text(s)))
        }),
        "minLength" | "maxLength" | "minItems" | "maxItems" => value.as_u64().is_some(),
        "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => {
            value.is_number()
        }
        "readOnly" | "writeOnly" | "deprecated" | "nullable" | "uniqueItems" => value.is_boolean(),
        "default" | "example" | "const" => !has_json_value(value),
        "examples" | "enum" => value
            .as_array()
            .is_some_and(|a| a.iter().all(|v| !has_json_value(v))),
        _ => false,
    })
}
fn definition_after_assignment(rest: &str) -> bool {
    const LIMIT: usize = 16 * 1024;
    let trimmed = rest.trim_start();
    if trimmed.starts_with('{') {
        let mut cap = trimmed.len().min(LIMIT);
        while !trimmed.is_char_boundary(cap) {
            cap -= 1;
        }
        let mut values = serde_json::Deserializer::from_str(&trimmed[..cap]).into_iter::<Value>();
        if let Some(Ok(value)) = values.next() {
            let end = values.byte_offset();
            let tail = &trimmed[end..];
            return end <= LIMIT
                && (tail.is_empty()
                    || tail.starts_with(|c: char| c.is_whitespace() || ",;}])`".contains(c)))
                && public_definition(&value)
                // JSON Value keeps the last duplicate key. A raw definition must not
                // hide an earlier default/example by replacing that key with null.
                && yaml_definition(&trimmed[..end]);
        }
        // YAML flow mappings use unquoted keys. Parse only the bounded line, without
        // consuming following prose or accepting trailing non-schema material.
        let line = trimmed.lines().next().unwrap_or("");
        if line.len() <= LIMIT {
            return yaml_definition(line);
        }
    } else if rest[..rest.len() - trimmed.len()].contains('\n') {
        let mut lines = rest.lines().skip_while(|line| line.trim().is_empty());
        if let Some(first) = lines.next() {
            let indent = first.len() - first.trim_start().len();
            if indent == 0 {
                return false;
            }
            let mut block = first.to_owned();
            for line in lines {
                if !line.trim().is_empty() && line.len() - line.trim_start().len() < indent {
                    break;
                }
                if block.len() + line.len() + 1 > LIMIT {
                    return false;
                }
                block.push('\n');
                block.push_str(line);
            }
            return block.len() <= LIMIT && yaml_definition(&block);
        }
    }
    false
}
fn yaml_definition(text: &str) -> bool {
    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text)
        .ok()
        .and_then(|v| serde_json::to_value(v).ok())
        .is_some_and(|v| public_definition(&v))
}
fn sensitive_field(key: &str, value: &Value) -> bool {
    (secret_key(key) || (env_key(key) && (value.is_object() || value.is_array())))
        && has_json_value(value)
        && !public_definition(value)
}
pub fn contains_sensitive_value(value: &Value) -> bool {
    sensitive_value_category(value).is_some()
}
pub fn sensitive_value_category(value: &Value) -> Option<SensitiveCategory> {
    match value {
        Value::String(s) => sensitive_text_category(s),
        Value::Array(values) => values.iter().find_map(sensitive_value_category),
        Value::Object(values) => values.iter().find_map(|(key, value)| {
            sensitive_text_category(key).or_else(|| {
                if sensitive_field(key, value) {
                    Some(if env_key(key) {
                        SensitiveCategory::EnvironmentDump
                    } else {
                        SensitiveCategory::LabelledValue
                    })
                } else {
                    sensitive_value_category(value)
                }
            })
        }),
        _ => None,
    }
}

pub fn ensure_public_text(text: &str) -> Result<()> {
    if let Some(category) = sensitive_text_category(text) {
        Err(Error::RuleViolation(category.rejection()))
    } else {
        Ok(())
    }
}
pub fn ensure_public_bytes(bytes: &[u8]) -> Result<()> {
    ensure_public_text(&String::from_utf8_lossy(bytes))?;
    // JSON reports can encode a private heading inside a string. Inspect decoded data too.
    if matches!(
        bytes.iter().find(|b| !b.is_ascii_whitespace()),
        Some(b'{' | b'[')
    ) && let Ok(value) = serde_json::from_slice::<Value>(bytes)
    {
        ensure_public_value(&value)?;
    }
    Ok(())
}

/// Add safe source coordinates without retaining or echoing the matched content.
/// Decoded structured-only findings have no invented line or column.
pub fn ensure_public_source(bytes: &[u8], locator: &str) -> Result<()> {
    match ensure_public_bytes(bytes) {
        Err(Error::RuleViolation(message)) if sensitive_rejection_details(&message).is_some() => {
            let mut location = crate::DiagnosticLocation {
                locator: Some(locator.into()),
                ..Default::default()
            };
            if let Ok(original) = std::str::from_utf8(bytes) {
                let text = normalized(original);
                if let Some((_, start)) = sensitive_match(&text) {
                    let start = start + text[start..].len() - text[start..].trim_start().len();
                    let mut position = 0;
                    if let Some((offset, _)) = normalized_chars(original).find(|(_, c)| {
                        let found = position == start;
                        position += c.len_utf8();
                        found
                    }) {
                        let prefix = &original[..offset];
                        location.line = Some(prefix.bytes().filter(|b| *b == b'\n').count() + 1);
                        location.column =
                            Some(prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1);
                    }
                }
            }
            Err(Error::SensitiveSource { message, location })
        }
        result => result,
    }
}
pub fn ensure_public_value(value: &Value) -> Result<()> {
    if let Some(category) = sensitive_value_category(value) {
        Err(Error::RuleViolation(category.rejection()))
    } else {
        Ok(())
    }
}
pub fn ensure_public_data<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    ensure_public_value(&serde_json::to_value(value)?)
}
/// Validate the complete source before shortening optional display text. Whitespace
/// folding/truncation can split an otherwise public schema; back up to a word boundary
/// in that case. Callers retain a source reference for the complete original content.
pub fn public_summary(text: &str, limit: usize) -> Result<String> {
    ensure_public_text(text)?;
    if limit == 0 {
        return Ok(String::new());
    }
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= limit && !contains_sensitive_text(&clean) {
        return Ok(clean);
    }
    let mut prefix: String = clean.chars().take(limit.saturating_sub(1)).collect();
    loop {
        let shortened = format!("{}…", prefix.trim_end());
        if !contains_sensitive_text(&shortened) {
            return Ok(shortened);
        }
        match prefix.rfind(char::is_whitespace) {
            Some(end) => prefix.truncate(end),
            None => prefix.clear(),
        }
    }
}
pub fn safe_diagnostic(text: &str) -> String {
    if contains_sensitive_text(text) {
        SENSITIVE_CONTENT_WITHHELD.into()
    } else {
        text.into()
    }
}
pub fn redact_sensitive_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(safe_diagnostic(&s)),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_sensitive_value).collect())
        }
        Value::Object(values) => {
            if values.keys().any(|key| contains_sensitive_text(key)) {
                return Value::String(SENSITIVE_CONTENT_WITHHELD.into());
            }
            Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| {
                        let value = if sensitive_field(&key, &value) {
                            Value::String(SENSITIVE_CONTENT_WITHHELD.into())
                        } else {
                            redact_sensitive_value(value)
                        };
                        (key, value)
                    })
                    .collect(),
            )
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn public_scalar_literals_have_complete_text_boundaries() {
        for key in ["refreshToken", "client_secret", "checkpoint_token"] {
            for literal in [
                "true", "false", "null", "~", "{}", "[]", "True", "False", "Null", "TRUE", "FALSE",
                "NULL",
            ] {
                for end in ["", ")", "]", "}", ",", ";", "`", "、", "。", "，", "；"] {
                    let text = format!("Call({key}={literal}{end}");
                    ensure_public_text(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
                    ensure_public_source(text.as_bytes(), "protocol-notes.txt").unwrap();
                    ensure_public_value(&json!({"body": text})).unwrap();
                    ensure_public_bytes(&serde_json::to_vec(&json!({"body": text})).unwrap())
                        .unwrap();
                }
                let text = format!("Use {key}=`{literal}`; continue.");
                ensure_public_text(&text).unwrap();
                ensure_public_bytes(&serde_json::to_vec(&json!({"body": text})).unwrap()).unwrap();
            }
        }
    }

    #[test]
    fn shell_literal_concatenations_are_nonempty_values() {
        for text in [
            "export PASSWORD=false\" synthetic-private-value\"",
            "export PASSWORD=true' synthetic-private-value'",
            "export PASSWORD=null\" synthetic-private-value\"",
            "export PASSWORD=FALSE' synthetic-private-value'",
        ] {
            assert!(ensure_public_text(text).is_err(), "{text}");
            assert!(ensure_public_bytes(text.as_bytes()).is_err());
            assert!(ensure_public_source(text.as_bytes(), "synthetic-notes.md").is_err());
            let wrapped = json!({"body": text});
            assert!(ensure_public_value(&wrapped).is_err());
            assert!(ensure_public_bytes(&serde_json::to_vec(&wrapped).unwrap()).is_err());
            assert_eq!(safe_diagnostic(text), SENSITIVE_CONTENT_WITHHELD);
            let error = ensure_public_source(text.as_bytes(), "synthetic-notes.md")
                .unwrap_err()
                .report();
            assert!(
                !serde_json::to_string(&error)
                    .unwrap()
                    .contains("synthetic-private-value")
            );
        }
    }

    #[test]
    fn private_blocks_do_not_inherit_scalar_exemptions() {
        for body in [
            "True customer identities must be included in every response.\nInternal customer: synthetic-private-value.",
            "false\nInternal customer: synthetic-private-value.",
            "null\nInternal customer: synthetic-private-value.",
            "[redacted]\nInternal customer: synthetic-private-value.",
        ] {
            let text = format!("# Private prompt\n{body}\n");
            assert_eq!(
                sensitive_text_category(&text),
                Some(SensitiveCategory::PrivatePrompt)
            );
            assert!(ensure_public_text(&text).is_err());
            assert!(ensure_public_bytes(text.as_bytes()).is_err());
            assert!(ensure_public_source(text.as_bytes(), "synthetic-notes.md").is_err());
            let wrapped = json!({"body": text});
            assert!(ensure_public_value(&wrapped).is_err());
            assert!(ensure_public_bytes(&serde_json::to_vec(&wrapped).unwrap()).is_err());
            assert_eq!(safe_diagnostic(&text), SENSITIVE_CONTENT_WITHHELD);
            assert!(
                !serde_json::to_string(&redact_sensitive_value(wrapped))
                    .unwrap()
                    .contains("synthetic-private-value")
            );
        }
        for body in ["", "[redacted]", "${PRIVATE_PROMPT}"] {
            ensure_public_text(&format!("# Private prompt\n{body}\n")).unwrap();
        }
    }

    #[test]
    fn public_literal_sentence_endings_preserve_wrapped_notes() {
        for text in [
            "Use `refreshToken=false`.",
            "Use `refreshToken=false`. Continue with `checkpoint_token=null`.",
            "Use refreshToken=`false`.",
            "account/read(refreshToken=false).",
            "checkpoint_token=null、continue.",
        ] {
            ensure_public_text(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            ensure_public_source(text.as_bytes(), "synthetic-notes.md").unwrap();
            let wrapped = json!({"body": text});
            ensure_public_value(&wrapped).unwrap();
            ensure_public_bytes(&serde_json::to_vec(&wrapped).unwrap()).unwrap();
            let yaml = serde_yaml_ng::to_string(&wrapped).unwrap();
            ensure_public_bytes(yaml.as_bytes()).unwrap();
        }
        for text in [
            "password=false.value",
            "password=null.value",
            "Use `refreshToken=false`. password: synthetic-private-value",
        ] {
            assert!(ensure_public_text(text).is_err(), "{text}");
        }
        for text in [
            r#"{"body":"account/read(refreshToken=false)"}"#,
            r#"{"body":"Say \"public\" then use refreshToken=false"}"#,
            r#"["Use refreshToken=false","Continue"]"#,
            "body: 'Use refreshToken=false'\n",
            "body: 'We''ll use refreshToken=false'\n",
        ] {
            ensure_public_source(text.as_bytes(), "synthetic-notes.txt")
                .unwrap_or_else(|e| panic!("{text}: {e}"));
        }
    }

    #[test]
    fn public_boolean_fields_are_consistent_with_source_bytes() {
        for value in [
            json!({"refreshToken": false, "client_secret": true, "checkpoint_token": null}),
            json!({"options": {"fixture_token": false}}),
        ] {
            ensure_public_value(&value).unwrap();
            ensure_public_bytes(&serde_json::to_vec(&value).unwrap()).unwrap();
        }
        ensure_public_bytes(b"refreshToken: false\nclient_secret: true\n").unwrap();
        ensure_public_bytes(b"refreshToken: False\nclient_secret: TRUE\ncheckpoint_token: Null\n")
            .unwrap();
    }

    #[test]
    fn literal_prefixes_and_quoted_scalar_strings_remain_sensitive() {
        for value in [
            "falsehood",
            "true_value",
            "nullish",
            "false-value",
            "null.value",
            "true/opaque",
            "\"false\"",
            "'true'",
            "\"null\"",
            "'[]'",
            "\"{}\"",
            "\"~\"",
            "null}suffix",
            "[]suffix",
            "{}suffix",
            "`false`opaque",
            "false\"opaque",
            "FALSE-value",
        ] {
            let text = format!("password: {value}");
            let error = ensure_public_text(&text).unwrap_err().report();
            assert!(!serde_json::to_string(&error).unwrap().contains(value));
        }
        for value in [
            json!({"refreshToken": "false"}),
            json!({"password": "null"}),
        ] {
            assert!(ensure_public_value(&value).is_err());
            assert!(ensure_public_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        for text in [
            "refreshToken=false; password: synthetic-private-value",
            "checkpoint_token=null、Basic YTpi",
            "refreshToken=false; export HOME=/synthetic-private-value",
        ] {
            assert!(ensure_public_text(text).is_err(), "{text}");
        }
    }

    #[test]
    fn complete_primitive_type_declarations_are_not_secret_assignments() {
        for text in [
            "interface Login { password: string; }",
            "class Login { password: string; }",
            "```ts\nexport interface Login {\n  username: string;\n  password: string\n}\n```",
            "type Login = { readonly password: string | undefined; token: string[] };",
        ] {
            ensure_public_text(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        }
        for text in [
            "password: string",
            "password = string",
            r#"{"password":"string"}"#,
            "const login = { password: string };",
            "interface Login { password: string = suppliedValue; }",
            "class Login { password: string = suppliedValue; }",
            "interface Login { password: 'suppliedValue'; }",
            "interface Login { password: string; token: suppliedValue; }",
            "interface Login { password: string; }\npassword: suppliedValue",
            "interface Login { password: string; }\nBasic YTpi",
        ] {
            assert!(ensure_public_text(text).is_err(), "{text}");
        }
    }

    #[test]
    fn source_diagnostics_map_normalization_to_original_coordinates_without_echo() {
        for (text, line, column) in [
            ("# 中文\n\n  password: synthetic-private-value", 3, 3),
            ("# 中文\n  ｐａｓｓｗｏｒｄ： synthetic-private-value", 2, 3),
            ("# 中文\n  pa\u{200b}ssword: synthetic-private-value", 2, 3),
            ("# 中文\n  pa\\u0073sword: synthetic-private-value", 2, 3),
        ] {
            let report = ensure_public_source(text.as_bytes(), "README.md")
                .unwrap_err()
                .report();
            assert_eq!(report.code, "RuleViolation");
            let details = report.details.as_ref().unwrap();
            assert_eq!(details["location"]["locator"], "README.md");
            assert_eq!(details["location"]["line"], line);
            assert_eq!(details["location"]["column"], column);
            assert_eq!(details["rule"], "source.public_content");
            assert!(
                !serde_json::to_string(&report)
                    .unwrap()
                    .contains("synthetic-private-value")
            );
            assert!(
                crate::render_diagnostic_details(Some(details))
                    .contains(&format!("README.md:{line}:{column}"))
            );
        }
    }

    #[test]
    fn command_configuration_and_permission_narratives_are_not_dumps_or_headers() {
        for text in [
            "Expected count APP_EXPECTED_TASKS=134; keep the original source.",
            "# APP_EXPECTED_TASKS=134",
            "PROJECT_ROOT=/public/project EXPECTED_ITEMS=134 cargo test --offline",
            "TASK_LIMIT=25 ./check",
            "passed normal native authorization: 官方设备确认后应用继续工作；重开保留原记录。",
            "Completed native authorization: the user confirmed this operation.",
        ] {
            ensure_public_text(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            ensure_public_value(&json!({"summary":text})).unwrap();
            ensure_public_bytes(&serde_json::to_vec(&json!({"summary":text})).unwrap()).unwrap();
            ensure_public_text(&public_summary(text, 240).unwrap()).unwrap();
        }
        for text in [
            "APP_VALUE=unknown-value",
            "HOME=/private PATH=/private/bin",
            "Run export HOME=/private",
            "API_KEY=unknown-value cargo test",
            "Run API_KEY=unknown-value cargo test",
            "Authorization: the user confirmed this operation.",
            "native authorization: unknown-value",
            "native authorization: Bearer word",
            "native authorization: Basic word",
            "native authorization: aBcdEfgHijKlmNopQrStUvWxyz extra words",
            "authorization: 用户明确允许本轮编辑",
        ] {
            assert!(ensure_public_text(text).is_err(), "{text}");
        }
        assert!(
            ensure_public_value(&json!({"authorization":"the user confirmed this operation"}))
                .is_err()
        );
    }

    #[test]
    fn rejection_categories_preserve_the_legacy_code_without_disclosing_values() {
        for (text, category) in [
            ("export HOME=/private", "environment_dump"),
            ("password: synthetic-private-value", "labelled_value"),
            ("Basic YTpi", "credential"),
            ("# Private prompt\nprivate material", "private_prompt"),
        ] {
            let report = ensure_public_text(text).unwrap_err().report();
            assert_eq!(report.code, "RuleViolation");
            assert_eq!(report.details.as_ref().unwrap()["category"], category);
            assert_eq!(
                report.details.as_ref().unwrap()["policy_version"],
                SECRET_POLICY_VERSION
            );
            assert!(!serde_json::to_string(&report).unwrap().contains(text));
        }
    }

    #[test]
    fn labelled_values_and_environment_are_rejected_without_echo() {
        for text in [
            "password: fixture-value-only",
            "API_KEY=fixture-value-only",
            "access_token:\n  fixture-value-only",
            "authorization: fixture-value-only",
            "export HOME=/fixture/private",
            "private_prompt: fixture-value-only",
            "# 私有提示词\nfixture-value-only",
            "env: {HOME: /fixture}",
        ] {
            let error = ensure_public_text(text).unwrap_err();
            assert!(!error.report().message.contains("fixture"));
        }
        for value in [
            json!({"password":1234}),
            json!({"private_prompt":["fixture"]}),
            json!({"environment":{"HOME":"/fixture"}}),
        ] {
            assert!(ensure_public_value(&value).is_err());
        }
    }
    #[test]
    fn ordinary_discussion_and_explicit_placeholders_remain_usable() {
        ensure_public_text(&format!("task-{}", "a".repeat(40))).unwrap();
        for bytes in [
            br#"{"password":null}"#.as_slice(),
            br#"{"env":{}}"#,
            br#"{"env":[]}"#,
        ] {
            ensure_public_bytes(bytes).unwrap();
        }
        ensure_public_text("environment: candidate").unwrap();
        assert!(ensure_public_text("environment:\n  HOME: /fixture/private").is_err());
        for text in [
            "Review API key handling, token budgets, password protection and private prompts.",
            "讨论密码、令牌和环境变量的保护。",
            "password: [redacted]",
            "API_KEY=${EXAMPLE_API_KEY}",
            "token: \"\"",
            "password: null",
            "secret: ***",
        ] {
            ensure_public_text(text).unwrap();
        }
        ensure_public_value(&json!({"pending_secret_conditions":16,"token_budget":5000,"environment":"candidate","api_key":"${EXAMPLE_API_KEY}"})).unwrap();
        assert!(ensure_public_text("password: [redacted]fixture").is_err());
    }
    #[test]
    fn measurement_environment_headings_are_not_environment_dumps() {
        let body = "\n\n- Server commit: `0123456789abcdef0123456789abcdef01234567`\n- Python: `3.12.11`\n- Node: `22.14.0`\n- Operating system: Ubuntu 24.04\n- catalog: 8 domains and 55 children\n- measurement unit: UTF-8 serialized bytes, not model tokens\n";
        for heading in [
            "Environment:",
            "Benchmark environment:",
            "Runtime:",
            "Configuration:",
        ] {
            for text in [
                heading.to_owned(),
                format!("{heading}{body}"),
                format!("{heading}{}", body.replace("\n- ", "\n  - ")),
                format!("{heading}\n\n  - Python: 3.12.11\n  - Operating system: Ubuntu 24.04\n"),
                format!("```text\n{heading}{body}```\n"),
            ] {
                ensure_public_text(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
                ensure_public_source(text.as_bytes(), "measurement.md").unwrap();
                ensure_public_value(&json!({"body":text})).unwrap();
            }
        }
        for text in [
            r#"{"node_modules/std-env": {"version": "3.10.0", "dev": true}}"#,
            r#"{"node_modules/env": {"version": "1.0.0"}}"#,
            r#"{"some.environment": {"version": "1.0.0"}}"#,
            "Environment:\nPublic measurement conditions follow.\n",
            "Environment:\n\n## Results\nThe sample contains 55 tools.\n",
            "Environment:\n\n- Python: 3.12.11\n\nAPI_KEY=${EXAMPLE_API_KEY}\n",
            "environment:\n  HOME: ${HOME}\n  PATH: '[redacted]'\n",
            "environment:\n  HOME:\n  PATH: ${PATH}\n",
            "environment:\n  HOME:\n    ${HOME}\n",
        ] {
            ensure_public_text(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        }
    }

    #[test]
    fn variable_shaped_colon_lists_remain_review_candidates() {
        for text in [
            "environment:\n- HOME: /synthetic-private-value",
            "Environment:\n- Python: 3.12\n  HOME: /synthetic-private-value",
            "Environment:\n- OS: Ubuntu 24.04",
        ] {
            assert_eq!(
                sensitive_text_category(text),
                Some(SensitiveCategory::EnvironmentDump)
            );
        }
    }

    #[test]
    fn environment_diagnostics_point_to_dump_entries_or_mixed_credentials() {
        for (text, category, line) in [
            (
                "Environment:\n\n- Python: 3.12.11\npassword: synthetic-private-value",
                "labelled_value",
                4,
            ),
            (
                "Environment:\n\n- Python: 3.12.11\nexport HOME=/synthetic-private-value",
                "environment_dump",
                4,
            ),
            (
                "Environment:\n\nHOME=/synthetic-private-value\nPATH=/synthetic-bin",
                "environment_dump",
                3,
            ),
            (
                "Environment:\n  - Python: 3.12.11\n  - API key: sk-synthetic-private-value",
                "credential",
                3,
            ),
            (
                "environment:\n  HOME:\n    /synthetic-private-value",
                "environment_dump",
                3,
            ),
            (
                "environment:\n  HOME: /synthetic-private-value",
                "environment_dump",
                2,
            ),
            (
                "environment:\n  HOME: ${HOME}\n  PATH: /synthetic-private-value",
                "environment_dump",
                3,
            ),
            (
                "service:\n  environment:\n    - HOME=/synthetic-private-value",
                "environment_dump",
                3,
            ),
            (
                "environment:\n- HOME=/synthetic-private-value",
                "environment_dump",
                2,
            ),
            (
                "```yaml\nenvironment:\n  HOME: /synthetic-private-value\n```",
                "environment_dump",
                3,
            ),
            (
                "environment:\n  {HOME: /synthetic-private-value}",
                "environment_dump",
                2,
            ),
        ] {
            let report = ensure_public_source(text.as_bytes(), "measurement.md")
                .unwrap_err()
                .report();
            let details = report.details.as_ref().unwrap();
            assert_eq!(details["category"], category, "{text}");
            assert_eq!(details["location"]["line"], line, "{text}");
            assert!(
                !serde_json::to_string(&report)
                    .unwrap()
                    .contains("synthetic-private-value")
            );
        }
    }

    #[test]
    fn basic_prose_remains_usable() {
        for text in [
            "The basic source-intake ledger has no phase declaration.",
            "Review basic authentication and basic validation requirements.",
            "A basic\ncomponent requires an explicit work identity.",
            "The BASIC infrastructure documentation is ready.",
        ] {
            ensure_public_text(text).unwrap();
            ensure_public_value(&json!({"summary": text})).unwrap();
            assert_eq!(safe_diagnostic(text), text);
        }
    }

    #[test]
    fn public_schema_and_authentication_definitions_are_not_values() {
        for text in [
            "Bearer authentication and bearer authorization are protocol concepts.",
            "Discuss Bearer credentials and Bearer authentication requirements.",
            r#"{"token":{"type":"string","pattern":"^[a-z][a-z0-9_.-]{1,63}$"}}"#,
            r#"{"authorization":{"type":"http","scheme":"bearer"}}"#,
            "# Schema\n```json\n{\"password\":{\"type\":\"string\",\"format\":\"password\",\"description\":\"Supplied at runtime\"}}\n```\n",
            "# Schema\n```yaml\ntoken:\n  type: string\n  description: Supplied at runtime\n```\n",
            "password: {type: string, format: password}\n",
            "TOKEN={\"type\":\"string\"}\n",
        ] {
            ensure_public_text(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            ensure_public_bytes(text.as_bytes()).unwrap();
            ensure_public_value(&json!({"body":text})).unwrap();
            assert_eq!(safe_diagnostic(text), text);
        }
        for value in [
            json!({"token":{"type":"string"}}),
            json!({"type":"object","properties":{"password":{"type":"string","format":"password"}},"required":["password"]}),
            json!({"authorization":{"type":"http","scheme":"bearer"}}),
            json!({"token":{"type":"string","default":"[redacted]"}}),
        ] {
            ensure_public_value(&value).unwrap();
            assert_eq!(redact_sensitive_value(value.clone()), value);
        }
        let text = format!(
            "{}Schema: {{\"token\":{{\"type\":\"string\"}}}}",
            "Public discussion. ".repeat(12)
        );
        let short = public_summary(&text, 240).unwrap();
        assert!(short.chars().count() <= 240);
        ensure_public_text(&short).unwrap();
        assert!(
            public_summary(
                &format!("{}password: fixture-value", "Public. ".repeat(80)),
                240
            )
            .is_err()
        );
    }

    #[test]
    fn definitions_cannot_hide_credentials_or_data_values() {
        for value in [
            json!({"token":{"type":"string","default":"fixture-value"}}),
            json!({"token":{"type":"string","example":"fixture-value"}}),
            json!({"token":{"type":"string","enum":["fixture-value"]}}),
            json!({"authorization":{"type":"http","scheme":"bearer","value":"fixture-value"}}),
            json!({"token":{"type":"object","properties":{"client_secret":{"type":"string","default":"fixture-value"}}}}),
            json!({"token":{"type":"string"},"password":"fixture-value"}),
            json!({"token":"{\"type\":\"string\"}"}),
        ] {
            assert!(ensure_public_value(&value).is_err(), "{value}");
            assert!(
                ensure_public_bytes(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{value}"
            );
            assert!(
                !redact_sensitive_value(value)
                    .to_string()
                    .contains("fixture-value")
            );
        }
        for text in [
            "Authorization: Bearer word",
            "Authorization: Bearer authentication",
            "Bearer fixture-token-1234",
            "Bearer aBcdEfgHijKlmNopQrStUvWxyz",
            "token: {type: string, default: fixture-value}\n",
            "token:\n  type: string\n  default: fixture-value\n",
            "token: {\"type\":\"string\"}fixture-value",
            "token: {\"type\":\"string\"}\npassword: fixture-value",
            "token: {\"type\":\"string\",\"default\":\"fixture-value\",\"default\":null}",
            "token:\n  type: string\n  default: fixture-value\n  default: null\n",
        ] {
            assert!(ensure_public_text(text).is_err(), "{text}");
        }
    }
    #[test]
    fn basic_credentials_are_rejected_without_echo() {
        // Public synthetic user/password pairs, including short and unpadded values.
        for text in [
            "Basic dXNlcjpwYXNz",
            "basic dXNlcjo=",
            "Basic dXNlcjo",
            "Basic YTpi",
            "Basic Og==",
            "Ｂａｓｉｃ　dXNlcjpwYXNz",
        ] {
            assert!(ensure_public_text(text).is_err());
            assert!(ensure_public_value(&json!({"body": text})).is_err());
            assert_eq!(safe_diagnostic(text), SENSITIVE_CONTENT_WITHHELD);
        }
    }
    #[test]
    fn unicode_and_escaped_labels_cannot_bypass_detection() {
        for text in [
            "ＡＰＩ＿ＫＥＹ：fixture-value-only",
            "pass\u{200b}word: fixture-value-only",
            "密碼：fixture-value-only",
            r#"{"pa\u0073sword":"fixture-value-only"}"#,
        ] {
            assert!(ensure_public_text(text).is_err());
        }
        assert!(ensure_public_value(&json!({"ｐａｓｓｗｏｒｄ":"fixture-value-only"})).is_err());
    }
    #[test]
    fn recognizable_tokens_and_binary_content_are_rejected() {
        for prefix in ["sk-", "ghp_", "github_pat_", "xoxb-"] {
            let text = format!("{prefix}{}", "a".repeat(40));
            assert!(ensure_public_text(&text).is_err());
            let mut bytes = vec![0xff, 0, 1];
            bytes.extend(text.bytes());
            assert!(ensure_public_bytes(&bytes).is_err());
        }
        assert!(ensure_public_text("-----BEGIN PRIVATE KEY-----").is_err());
        assert!(ensure_public_text("https://fixture-user:fixture-pass@example.invalid").is_err());
        let report =
            serde_json::to_vec(&json!({"body":"# Private prompt\nfixture-value-only"})).unwrap();
        assert!(ensure_public_bytes(&report).is_err());
    }
    #[test]
    fn diagnostics_keep_receipt_identity_but_withhold_sensitive_reason() {
        let value = redact_sensitive_value(
            json!({"proposal_id":"fixture-id","reason":"password=fixture-value-only"}),
        );
        assert_eq!(value["proposal_id"], "fixture-id");
        assert!(!value.to_string().contains("fixture-value-only"));
    }
}
