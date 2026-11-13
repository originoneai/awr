//! Field-level YAML edits. Parser events locate structure; original bytes remain authoritative.
use awr_core::{Error, Result};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, ops::Range};
use yaml_rust2::{
    parser::{Event, Parser},
    scanner::TScalarStyle,
};

fn unsupported(message: &str) -> Error {
    Error::MutationUnsupported(message.into())
}

/// Refusal that names the exact source position so callers can inspect the record
/// instead of guessing which line tripped the byte-level editor.
fn unsupported_at(text: &str, byte: usize, message: &str) -> Error {
    let prefix = text.get(..byte).unwrap_or(text);
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().map_or(0, |s| s.chars().count()) + 1;
    Error::MutationUnsupported(format!("{message} at line {line}, column {column}"))
}

struct Node {
    range: Range<usize>,
    kind: Kind,
}
enum Kind {
    Scalar(TScalarStyle, String),
    Mapping(Vec<(String, Node)>, bool, usize), // fields, flow, field indentation
    Sequence(Vec<Node>),
}
struct Reader<'a> {
    text: &'a str,
    parser: Parser<std::str::Chars<'a>>,
    offsets: Vec<usize>,
}
impl<'a> Reader<'a> {
    fn token(&mut self) -> Result<(Event, usize)> {
        let (event, marker) = self
            .parser
            .next_token()
            .map_err(|e| Error::InvalidInput(format!("YAML mutation syntax: {e}")))?;
        let byte = *self
            .offsets
            .get(marker.index())
            .ok_or_else(|| unsupported("YAML marker outside source"))?;
        Ok((event, byte))
    }

    fn node(
        &mut self,
        first: (Event, usize),
        parent_indent: usize,
        depth: usize,
        in_flow: bool,
    ) -> Result<Node> {
        if depth > 128 {
            return Err(unsupported("YAML mutation nesting exceeds 128 levels"));
        }
        let (event, mut start) = first;
        let (end, kind) = match event {
            Event::MappingStart(anchor, tag) => {
                reject_properties(anchor, tag.is_some())?;
                let flow = self.text[start..].starts_with('{');
                let mut fields = Vec::new();
                let mut keys = BTreeSet::new();
                let mut indent = parent_indent;
                let marker_end = loop {
                    let (key, key_start) = self.token()?;
                    if key == Event::MappingEnd {
                        break key_start;
                    }
                    let Event::Scalar(key, style, anchor, tag) = key else {
                        return Err(unsupported("complex mapping keys require manual editing"));
                    };
                    reject_properties(anchor, tag.is_some())?;
                    if key == "<<" || !keys.insert(key.clone()) {
                        return Err(unsupported(
                            "merge or duplicate mapping keys require manual editing",
                        ));
                    }
                    // A plain key has no escapes and one line, so the scanned value must
                    // match the source bytes at its marker. A mismatch means the scanner's
                    // marker positions drifted (upstream block-scalar accounting) and every
                    // computed range would silently point at the wrong bytes.
                    if style == TScalarStyle::Plain
                        && !self.text[key_start..].starts_with(key.as_str())
                    {
                        return Err(unsupported_at(
                            self.text,
                            key_start,
                            "YAML scanner position does not match the mapping key; refusing byte-level edit on drifted positions",
                        ));
                    }
                    if fields.is_empty() {
                        indent = column(self.text, key_start);
                        if !flow {
                            start = key_start;
                        }
                    }
                    let mut value = self.token()?;
                    let key_end = scalar_end(self.text, key_start, style, true, flow, indent)?;
                    let colon = self.text[key_end..]
                        .find(':')
                        .map(|n| key_end + n)
                        .ok_or_else(|| {
                            unsupported_at(self.text, key_end, "missing YAML mapping colon")
                        })?;
                    if matches!(
                        value.0,
                        Event::Scalar(_, TScalarStyle::Literal | TScalarStyle::Folded, _, _)
                    ) {
                        // yaml-rust2 locates nonempty block scalars at their first content
                        // character. Recover the header from this field's lexical colon.
                        let header = colon + 1 + self.text[colon + 1..].len()
                            - self.text[colon + 1..].trim_start().len();
                        if !self.text[header..].starts_with(['|', '>']) {
                            return Err(unsupported(
                                "block scalar header separated by a comment requires manual editing",
                            ));
                        }
                        value.1 = header;
                    }
                    // An implicit null has no token bytes. Its parser marker can belong to the
                    // next key, so recover the insertion point from this key's colon instead.
                    let mut child = if matches!(&value.0, Event::Scalar(s, TScalarStyle::Plain, 0, None) if s.is_empty())
                    {
                        Node {
                            range: colon + 1..colon + 1,
                            kind: Kind::Scalar(TScalarStyle::Plain, String::new()),
                        }
                    } else {
                        self.node(value, column(self.text, key_start), depth + 1, flow)?
                    };
                    if matches!(&child.kind, Kind::Mapping(_, false, _) | Kind::Sequence(_))
                        && !self.text[child.range.start..].starts_with(['[', '{'])
                    {
                        child.range.start = colon + 1;
                    }
                    fields.push((key, child));
                };
                let end = if flow {
                    close(self.text, marker_end, '}')?
                } else {
                    fields.last().map_or(start, |(_, n)| n.range.end)
                };
                (end, Kind::Mapping(fields, flow, indent))
            }
            Event::SequenceStart(anchor, tag) => {
                reject_properties(anchor, tag.is_some())?;
                let flow = self.text[start..].starts_with('[');
                let mut children = Vec::new();
                let marker_end = loop {
                    let child = self.token()?;
                    if child.0 == Event::SequenceEnd {
                        break child.1;
                    }
                    children.push(self.node(child, parent_indent, depth + 1, flow)?);
                };
                let end = if flow {
                    close(self.text, marker_end, ']')?
                } else {
                    children.last().map_or(start, |n| n.range.end)
                };
                (end, Kind::Sequence(children))
            }
            Event::Scalar(value, style, anchor, tag) => {
                reject_properties(anchor, tag.is_some())?;
                let end = scalar_end(self.text, start, style, false, in_flow, parent_indent)?;
                (end, Kind::Scalar(style, value))
            }
            Event::Alias(_) => return Err(unsupported("YAML aliases require manual editing")),
            _ => return Err(unsupported("unexpected YAML node boundary")),
        };
        Ok(Node {
            range: start..end,
            kind,
        })
    }
}
fn reject_properties(anchor: usize, tagged: bool) -> Result<()> {
    if anchor != 0 || tagged {
        Err(unsupported(
            "anchored or tagged YAML requires manual editing",
        ))
    } else {
        Ok(())
    }
}
fn close(text: &str, end: usize, expected: char) -> Result<usize> {
    if text[end..].starts_with(expected) {
        Ok(end + 1)
    } else {
        Err(unsupported("could not bound a YAML flow collection"))
    }
}
fn column(text: &str, start: usize) -> usize {
    let line = text[..start].rfind('\n').map_or(0, |n| n + 1);
    text[line..start].chars().count()
}
fn line_end(text: &str, start: usize) -> usize {
    text[start..]
        .find('\n')
        .map_or(text.len(), |n| start + n + 1)
}
fn scalar_end(
    text: &str,
    start: usize,
    style: TScalarStyle,
    key: bool,
    flow: bool,
    indent: usize,
) -> Result<usize> {
    let bytes = text.as_bytes();
    match style {
        TScalarStyle::SingleQuoted | TScalarStyle::DoubleQuoted => {
            let quote = if style == TScalarStyle::SingleQuoted {
                b'\''
            } else {
                b'"'
            };
            if bytes.get(start) != Some(&quote) {
                return Err(unsupported("quoted scalar marker mismatch"));
            }
            let mut pos = start + 1;
            while pos < bytes.len() {
                if quote == b'"' && bytes[pos] == b'\\' {
                    pos += 2;
                    continue;
                }
                if bytes[pos] == quote {
                    if quote == b'\'' && bytes.get(pos + 1) == Some(&quote) {
                        pos += 2;
                        continue;
                    }
                    return Ok(pos + 1);
                }
                pos += 1;
            }
            Err(unsupported("unclosed quoted scalar"))
        }
        TScalarStyle::Literal | TScalarStyle::Folded => {
            let mut end = line_end(text, start);
            while end < text.len() {
                let next = line_end(text, end);
                let line = &text[end..next];
                let spaces = line.bytes().take_while(|b| *b == b' ').count();
                if !line.trim().is_empty() && spaces <= indent {
                    break;
                }
                end = next;
            }
            Ok(end)
        }
        TScalarStyle::Plain => {
            let mut end = start;
            while end < bytes.len() {
                let b = bytes[end];
                if b == b'\n'
                    || b == b'\r'
                    || (b == b'#' && (end == start || bytes[end - 1].is_ascii_whitespace()))
                    || (b == b':'
                        && (key
                            || bytes
                                .get(end + 1)
                                .is_none_or(|b| b.is_ascii_whitespace() || b",]}".contains(b))))
                    || (flow && b",]}".contains(&b))
                {
                    break;
                }
                end += 1;
            }
            end = start + text[start..end].trim_end().len();
            if !key && !flow {
                let mut line = line_end(text, end);
                while line < text.len() {
                    let next = line_end(text, line);
                    let content = &text[line..next];
                    if !content.trim().is_empty() && !content.trim_start().starts_with('#') {
                        let spaces = content.bytes().take_while(|b| *b == b' ').count();
                        if spaces <= indent {
                            break;
                        }
                        let comment = content.char_indices().find_map(|(i, c)| {
                            (c == '#'
                                && (i == 0 || content.as_bytes()[i - 1].is_ascii_whitespace()))
                            .then_some(i)
                        });
                        end = line + content[..comment.unwrap_or(content.len())].trim_end().len();
                        if comment.is_some() {
                            break;
                        }
                    }
                    line = next;
                }
            }
            Ok(end)
        }
    }
}
fn json(value: &Value) -> Result<String> {
    Ok(serde_json::to_string(value)?
        .replace('\u{85}', "\\u0085")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

fn render(text: &str, node: &Node, value: &Value, newline: &str) -> Result<String> {
    let original = &text[node.range.clone()];
    let Kind::Scalar(style, old_value) = &node.kind else {
        // Collection values are one edited field. Do not silently discard its comments.
        // A conservative '#' rejection also catches quoted hashes; callers can edit children.
        if text[node.range.start..line_end(text, node.range.end)].contains('#') {
            return Err(unsupported(
                "collection replacement would lose comments; edit individual fields",
            ));
        }
        let prefix = if original.starts_with(char::is_whitespace) {
            " "
        } else {
            ""
        };
        return Ok(format!("{prefix}{}", json(value)?));
    };
    if *style == TScalarStyle::Plain
        && !node.range.is_empty()
        && original.trim() != old_value
        && (original.contains('#')
            || serde_yaml_ng::from_str::<Value>(original)
                .ok()
                .as_ref()
                .and_then(Value::as_str)
                != Some(old_value.as_str()))
    {
        return Err(unsupported(
            "ambiguous multiline plain scalar boundaries require manual editing",
        ));
    }
    let Some(string) = value.as_str() else {
        return json(value);
    };
    match *style {
        TScalarStyle::SingleQuoted
            if !string.contains(['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}']) =>
        {
            Ok(format!("'{}'", string.replace('\'', "''")))
        }
        TScalarStyle::SingleQuoted => {
            if string.contains(['\r', '\u{85}', '\u{2028}', '\u{2029}'])
                || string.lines().any(|l| l.trim() != l)
            {
                return Err(unsupported(
                    "single-quoted multiline replacement cannot preserve edge whitespace or special line breaks",
                ));
            }
            let indent = " ".repeat(column(text, node.range.start) + 2);
            let mut body = String::new();
            let mut chars = string.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\n' {
                    body.push_str(newline);
                    body.push_str(newline);
                    body.push_str(&indent);
                    while chars.peek() == Some(&'\n') {
                        chars.next();
                        body.push_str(newline);
                        body.push_str(&indent);
                    }
                } else {
                    body.push(c);
                    if c == '\'' {
                        body.push(c);
                    }
                }
            }
            Ok(format!("'{body}'"))
        }
        TScalarStyle::DoubleQuoted => json(value),
        TScalarStyle::Plain => {
            let safe = !string.is_empty()
                && !string.contains([
                    '\n', '\r', ',', '[', ']', '{', '}', '#', '\u{85}', '\u{2028}', '\u{2029}',
                ])
                && string.trim() == string
                && !string.contains(": ")
                && serde_yaml_ng::from_str::<Value>(string).ok().as_ref() == Some(value);
            Ok(if safe {
                string.to_owned()
            } else {
                json(value)?
            })
        }
        TScalarStyle::Literal | TScalarStyle::Folded => {
            if string.contains(['\r', '\u{85}', '\u{2028}', '\u{2029}']) {
                return Err(unsupported(
                    "block scalar cannot preserve these line break characters",
                ));
            }
            let header_end = original.find('\n').map_or(original.len(), |n| n + 1);
            let header = original[..header_end].trim_end_matches(['\r', '\n']);
            let indicator_end = header
                .find(|c: char| c.is_whitespace() || c == '#')
                .unwrap_or(header.len());
            let indicator = &header[..indicator_end];
            let width = indicator.chars().find(|c| c.is_ascii_digit());
            let tail = &header[indicator_end..];
            let trailing = string.len() - string.trim_end_matches('\n').len();
            let chomp = if trailing == 0 {
                "-"
            } else if trailing == 1 {
                ""
            } else {
                "+"
            };
            let content = &original[header_end..];
            let spaces = content
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.bytes().take_while(|b| *b == b' ').count())
                .unwrap_or_else(|| column(text, node.range.start) + 2);
            let indent = " ".repeat(spaces);
            let mut body = string.trim_end_matches('\n').to_owned();
            if *style == TScalarStyle::Folded {
                // Folding preserves breaks adjacent to more-indented lines. For ordinary
                // paragraphs add exactly one blank line per run of logical line breaks.
                let lines: Vec<_> = body.split('\n').collect();
                let mut folded = String::new();
                for (i, line) in lines.iter().enumerate() {
                    if i > 0 {
                        folded.push('\n');
                    }
                    folded.push_str(line);
                    if !line.is_empty()
                        && !line.starts_with([' ', '\t'])
                        && i + 1 < lines.len()
                        && lines[i + 1..]
                            .iter()
                            .find(|l| !l.is_empty())
                            .is_some_and(|l| !l.starts_with([' ', '\t']))
                    {
                        folded.push('\n');
                    }
                }
                body = folded;
            }
            let mut output = format!(
                "{}{}{chomp}{tail}{newline}",
                if *style == TScalarStyle::Literal {
                    '|'
                } else {
                    '>'
                },
                width.map_or(String::new(), |c| c.to_string())
            );
            if !body.is_empty() {
                for line in body.split('\n') {
                    output.push_str(&indent);
                    output.push_str(line);
                    output.push_str(newline);
                }
            }
            for _ in 1..trailing {
                output.push_str(newline);
            }
            Ok(output)
        }
    }
}

fn parse_tree(text: &str) -> Result<Node> {
    let offsets = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut reader = Reader {
        text,
        parser: Parser::new_from_str(text),
        offsets,
    };
    if reader.token()?.0 != Event::StreamStart || reader.token()?.0 != Event::DocumentStart {
        return Err(unsupported("YAML stream has no document"));
    }
    let first = reader.token()?;
    let root = reader.node(first, 0, 0, false)?;
    if reader.token()?.0 != Event::DocumentEnd || reader.token()?.0 != Event::StreamEnd {
        return Err(unsupported(
            "multiple YAML documents require manual editing",
        ));
    }
    Ok(root)
}

/// Best-effort exact value location. Complex YAML retains its JSON pointer instead
/// of inventing a line; this read never restricts which YAML the ledger accepts.
pub(crate) fn pointer_location(text: &str, pointer: &str) -> Option<(usize, usize)> {
    let root = parse_tree(text).ok()?;
    let mut target = &root;
    if !pointer.is_empty() {
        for encoded in pointer.strip_prefix('/')?.split('/') {
            let part = encoded.replace("~1", "/").replace("~0", "~");
            target = match &target.kind {
                Kind::Mapping(fields, ..) => &fields.iter().find(|(key, _)| key == &part)?.1,
                Kind::Sequence(items) => items.get(part.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
    }
    let prefix = text.get(..target.range.start)?;
    Some((
        prefix.bytes().filter(|c| *c == b'\n').count() + 1,
        prefix.rsplit('\n').next()?.chars().count() + 1,
    ))
}

pub(crate) fn edit_fields(
    text: &str,
    pointer: &str,
    changes: &Map<String, Value>,
) -> Result<String> {
    let root = parse_tree(text)?;
    let mut target = &root;
    let parts = if pointer.is_empty() {
        Vec::new()
    } else {
        pointer
            .strip_prefix('/')
            .ok_or_else(|| unsupported("expected JSON pointer"))?
            .split('/')
            .collect::<Vec<_>>()
    };
    for part in parts {
        let key = part.replace("~1", "/").replace("~0", "~");
        target = match &target.kind {
            Kind::Mapping(fields, ..) => fields.iter().find(|(k, _)| k == &key).map(|(_, n)| n),
            Kind::Sequence(nodes) => key.parse::<usize>().ok().and_then(|i| nodes.get(i)),
            _ => None,
        }
        .ok_or_else(|| Error::SourceConflict("exact YAML record pointer is missing".into()))?;
    }
    let Kind::Mapping(fields, flow, indent) = &target.kind else {
        return Err(unsupported("the exact YAML record must be a mapping"));
    };
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut edits = Vec::new();
    let mut added = Vec::new();
    for (key, value) in changes {
        if let Some((_, node)) = fields.iter().find(|(k, _)| k == key) {
            let mut replacement = render(text, node, value, newline)?;
            if node.range.is_empty() {
                replacement.insert(0, ' ');
            }
            edits.push((node.range.clone(), replacement));
        } else {
            added.push(format!(
                "{}: {}",
                json(&Value::String(key.clone()))?,
                json(value)?
            ));
        }
    }
    if !added.is_empty() {
        if *flow {
            let at = target.range.end - 1;
            edits.push((
                at..at,
                format!(
                    "{}{}",
                    if fields.is_empty() { "" } else { ", " },
                    added.join(", ")
                ),
            ));
        } else {
            let end = target.range.end;
            let at = if text[..end].ends_with('\n') {
                end
            } else {
                line_end(text, end)
            };
            let prefix = if at > 0 && !text[..at].ends_with('\n') {
                newline
            } else {
                ""
            };
            let mut addition = prefix.to_owned();
            for value in added {
                addition += &format!("{}{value}{newline}", " ".repeat(*indent));
            }
            edits.push((at..at, addition));
        }
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut output = text.to_owned();
    let mut boundary = text.len();
    for (range, replacement) in edits {
        if range.end > boundary {
            return Err(unsupported("overlapping YAML field edits"));
        }
        boundary = range.start;
        output.replace_range(range, &replacement);
    }
    Ok(output)
}

/// Append one record without serializing any existing record or renumbering keys.
pub(crate) fn append_work(text: &str, key: &str, fields: &[(String, Value)]) -> Result<String> {
    let root = parse_tree(text)?;
    let Kind::Mapping(root_fields, ..) = &root.kind else {
        return Err(unsupported("work creation needs a mapping document"));
    };
    let (_, collection) = root_fields
        .iter()
        .find(|(k, _)| k == "work_items")
        .ok_or_else(|| {
            unsupported("register a YAML ledger with an explicit work_items list or map")
        })?;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let record = Value::Object(fields.iter().cloned().collect());
    let (at, addition) = match &collection.kind {
        Kind::Sequence(items) if text[collection.range.start..].starts_with('[') => (
            collection.range.end - 1,
            format!(
                "{}{}",
                if items.is_empty() { "" } else { ", " },
                json(&record)?
            ),
        ),
        Kind::Sequence(items) => {
            let first = items
                .first()
                .ok_or_else(|| unsupported("empty block sequences require explicit [] syntax"))?;
            let line = text[..first.range.start].rfind('\n').map_or(0, |i| i + 1);
            let dash = text[line..line_end(text, line)]
                .find('-')
                .map(|i| line + i)
                .ok_or_else(|| unsupported("could not locate the ledger list indentation"))?;
            if !text[line..dash].trim().is_empty() {
                return Err(unsupported("unsupported compact ledger nesting"));
            }
            let indent = column(text, dash);
            let at = if text[..collection.range.end].ends_with('\n') {
                collection.range.end
            } else {
                line_end(text, collection.range.end)
            };
            let mut addition = if text[..at].ends_with('\n') {
                String::new()
            } else {
                newline.into()
            };
            for (i, (field, value)) in fields.iter().enumerate() {
                addition += &format!(
                    "{}{}{}: {}{newline}",
                    " ".repeat(indent),
                    if i == 0 { "- " } else { "  " },
                    json(&Value::String(field.clone()))?,
                    json(value)?
                );
            }
            (at, addition)
        }
        Kind::Mapping(items, true, _) => (
            collection.range.end - 1,
            format!(
                "{}{}: {}",
                if items.is_empty() { "" } else { ", " },
                json(&Value::String(key.into()))?,
                json(&record)?
            ),
        ),
        Kind::Mapping(_, false, indent) => {
            let at = if text[..collection.range.end].ends_with('\n') {
                collection.range.end
            } else {
                line_end(text, collection.range.end)
            };
            let mut addition = if text[..at].ends_with('\n') {
                String::new()
            } else {
                newline.into()
            };
            addition += &format!(
                "{}{}:{newline}",
                " ".repeat(*indent),
                json(&Value::String(key.into()))?
            );
            for (field, value) in fields {
                addition += &format!(
                    "{}{}: {}{newline}",
                    " ".repeat(*indent + 2),
                    json(&Value::String(field.clone()))?,
                    json(value)?
                );
            }
            (at, addition)
        }
        _ => {
            return Err(unsupported(
                "work_items must be an explicit list or keyed map",
            ));
        }
    };
    let mut output = text.to_owned();
    output.insert_str(at, &addition);
    Ok(output)
}
