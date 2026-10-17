//! FEAT-PEL-001 · Portable evidence locator — **normalize implemented** (gate 2).
//!
//! 状态：`normalize` 已按设计 v2.2 §0 的 C1/C2 与 v2.0 §3.2 的 N1–N9 实现为**纯字符串函数**
//! （零 IO、无宿主语义、无 `Path` 判据）；`resolve` 仍是 PR-2 契约桩（`unimplemented!`）。
//! 设计依据：`reports/feature-portable-evidence-locator-design-v2.2.md` 与 v2.0 §3.2/§3.3。
//! 绝不改写 `Evidence.locator` 原文；本模块只产出新的归一结构。
//!
//! ## §0 两条硬要求的落点
//!
//! - **C1 · scheme 先于路径与越界**：`normalize` 在切完行尾后缀后**第一步**做 scheme 识别；
//!   只要命中非 `file` 的 scheme 就立即返回 `Opaque`，**不进入**任何路径形态分类或越界判定。
//!   单字母前缀（`C:`）是 Windows 盘符而非 scheme，因此 scheme 至少两个字符（见 `split_scheme`）。
//! - **C2 · 词法归一化先于越界**：固定序 ①剥离 `\\?\`/`\\?\UNC\` ②识别 UNC/盘符/POSIX 绝对
//!   ③首段 `~` ⇒ `Opaque`（不展开 home）④`\`≡`/` ⑤`.`/`..` 栈折叠 ⑥盘符字母降小写
//!   ⑦**然后**才与 root 比较、判 `PathEscape`。栈下溢是 `PathEscape` 的唯一来源。
//!
//! 这些步骤在 `fold` 内按序完成；`normalize` 的越界判定只读 `fold` 的产物（`Folded`）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Id;

/// 归一后的 locator 类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocatorKind {
    /// 位于 project root 之内，跨设备稳定（只有相对结构参与判据）。
    ProjectRelative,
    /// 绝对路径且落在 root 之外：不可移植，显式标记而不是静默当作普通记录。
    ExternalAbsolute,
    /// 其它形式（非 `file` scheme、纯标识符、不可解析）。
    Opaque,
}

/// 不可移植的原因，与"缺失/漂移"严格区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnportableReason {
    /// 词法折叠后（normalize）或 canonicalize 后（resolve）逃出 root。
    PathEscape,
    /// ExternalAbsolute / Opaque / 不可表示的输入。
    NotPortable,
}

/// 归一结果。`original` 逐字节保留输入，永不改写。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceLocatorV1 {
    pub kind: LocatorKind,
    /// `'/'` 分隔、无 `.` / `..` 段（仅 ProjectRelative 时非空）。
    pub relpath: Option<String>,
    pub line: Option<u32>,
    pub span: Option<u32>,
    pub content_sha256: Option<String>,
    /// 该证据绑定的节指纹（来自 `SourceRef.section_fingerprint`）。
    pub section_expected: Option<String>,
    /// 写证据的设备标签：仅诊断，**不作判据**。
    pub origin_hint: Option<String>,
    pub original: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IoOp {
    Canonicalize,
    Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolveOutcome {
    Current {
        sha256: String,
    },
    /// 文件在、但该证据没有绑定内容指纹：**不得**当作 Current。
    Unbound,
    Missing,
    ContentDrift {
        expected: String,
        actual: String,
    },
    SectionDrift {
        expected: Option<String>,
        actual: Option<String>,
    },
    Unportable(UnportableReason),
    /// 环境错误 ≠ Missing：不许静默折叠。
    IoFailed {
        op: IoOp,
    },
}

/// 由 `resolve` 自己累加的 IO 计数（替代已删除的 `ReadCounter`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IoStats {
    pub canonicalize_calls: u32,
    pub hash_reads: u32,
}

/// 节指纹来源：#336 未落 / yaml 台账 ⇒ `Unavailable`（不是错误）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SectionCheck {
    Unavailable,
    Available { actual: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SectionStatus {
    NotEvaluated,
    NotBound,
    Compared { equal: bool },
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveReport {
    pub outcome: ResolveOutcome,
    pub section: SectionStatus,
    pub io: IoStats,
}

/// 注入面：PR-1 只依赖这一个 trait；生产实现仅用于真实文件系统冒烟。
pub trait LocatorIo {
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
    fn sha256_file(&self, path: &Path) -> std::io::Result<String>;
}

/// PR-2 才实现；此处只定义契约。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableGroup {
    pub relpath: String,
    pub line: Option<u32>,
    pub span: Option<u32>,
    pub evidence_ids: Vec<Id>,
    pub outcome: ResolveOutcome,
    pub section_check_unavailable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnportableEntry {
    pub original: String,
    pub reason: UnportableReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableReport {
    pub groups: Vec<PortableGroup>,
    pub unportable: Vec<UnportableEntry>,
}

// ---------------------------------------------------------------------------
// §0 C2 — lexical folding, entirely string-based (no `Path::is_absolute`, no
// `strip_prefix`, no host-dependent comparison).
// ---------------------------------------------------------------------------

/// Path shape after lexical analysis. Host-independent by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Drive,
    Unc,
    Posix,
    Relative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Folded {
    form: Form,
    /// Segments after `.`/`..` folding and separator unification. For absolute
    /// forms the first segments are the root (drive / UNC server+share).
    segments: Vec<String>,
    underflow: bool,
}

fn is_sep(byte: u8) -> bool {
    byte == b'/' || byte == b'\\'
}

/// N4 — shape classification, string only: same input, same class on every host.
fn classify(input: &str) -> Form {
    let bytes = input.as_bytes();
    if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || is_sep(bytes[2]))
    {
        return Form::Drive;
    }
    // `^([\\/]{2})[^\\/]+[\\/][^\\/]+` — two separators, a name, a separator, a name.
    if bytes.len() >= 4 && is_sep(bytes[0]) && is_sep(bytes[1]) {
        let mut first = 2;
        while first < bytes.len() && !is_sep(bytes[first]) {
            first += 1;
        }
        if first > 2 && first < bytes.len() {
            let mut second = first + 1;
            while second < bytes.len() && !is_sep(bytes[second]) {
                second += 1;
            }
            if second > first + 1 {
                return Form::Unc;
            }
        }
    }
    if bytes.first() == Some(&b'/') {
        Form::Posix
    } else {
        Form::Relative
    }
}

/// N5 — lexical fold: strip `\\?\` / `\\?\UNC\`, split on both separators,
/// drop empty and `.` segments, fold `..` with an underflow flag, then lowercase
/// the drive letter (N9 is the only case-normalising step; everything else is
/// byte-exact so cross-device equality is `relpath` equality).
fn fold(input: &str) -> Folded {
    // C2 step ①: `\\?\UNC\srv\share` -> `\\srv\share`, `\\?\C:\x` -> `C:\x`.
    let stripped = if let Some(rest) = input.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = input.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        input.to_string()
    };

    let form = classify(&stripped);
    let mut segments: Vec<String> = Vec::new();
    let mut underflow = false;
    for part in stripped.split(is_sep_char) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            let floor = match form {
                Form::Drive => 1,
                Form::Unc => 2,
                Form::Posix | Form::Relative => 0,
            };
            if segments.len() > floor {
                segments.pop();
            } else {
                underflow = true;
            }
        } else {
            segments.push(part.to_string());
        }
    }
    if form == Form::Drive {
        if let Some(first) = segments.first_mut() {
            let mut chars = first.chars();
            if let Some(letter) = chars.next() {
                let mut lowered = letter.to_ascii_lowercase().to_string();
                lowered.extend(chars);
                *first = lowered;
            }
        }
    }
    Folded {
        form,
        segments,
        underflow,
    }
}

fn is_sep_char(c: char) -> bool {
    c == '/' || c == '\\'
}

// ---------------------------------------------------------------------------
// §0 C1 — scheme detection before any path work.
// ---------------------------------------------------------------------------

/// `^[A-Za-z][A-Za-z0-9+.\-]*:` but requiring at least two characters: a
/// single-letter prefix is a Windows drive (`C:\x`, `C:/x`), and treating it as
/// a scheme would turn every Windows path into `Opaque`.
fn split_scheme(input: &str) -> Option<(&str, &str)> {
    let bytes = input.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b':' if index >= 2 => return Some((&input[..index], &input[index + 1..])),
            b':' => return None,
            byte if byte.is_ascii_alphanumeric()
                || byte == b'+'
                || byte == b'.'
                || byte == b'-' =>
            {
                index += 1;
            }
            _ => return None,
        }
    }
    None
}

/// N3a — decode a `file:` URI payload into a host-independent path string.
/// Returns `None` for malformed forms (percent errors, non-UTF-8) so the caller
/// can map them to `Opaque` per R8.
fn decode_file_uri(rest: &str) -> Option<String> {
    let decoded = if let Some(after) = rest.strip_prefix("//") {
        let (authority, path) = match after.find('/') {
            Some(index) => (&after[..index], &after[index..]),
            None => (after, ""),
        };
        let path = percent_decode(path)?;
        if authority.is_empty() || authority.eq_ignore_ascii_case("localhost") {
            path
        } else {
            let authority = percent_decode(authority)?;
            if authority.is_empty() {
                return None;
            }
            format!(r"\\{}{}", authority, path)
        }
    } else {
        percent_decode(rest)?
    };
    Some(strip_drive_leading_slash(decoded))
}

/// `/C:/x` -> `C:/x` (a `file:` URI puts the drive behind a leading `/`).
fn strip_drive_leading_slash(path: String) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
        && (bytes.len() == 3 || is_sep(bytes[3]))
    {
        return path[1..].to_string();
    }
    path
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            out.push(high * 16 + low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// N2 — `#L<a>(-L<b>)` suffix.
// ---------------------------------------------------------------------------

/// `> ("x.md#L12-L14", ...)` -> `(Some(12), Some(3), "x.md")`.
/// Any malformed / overflowing suffix leaves the whole string as a path.
fn split_line_suffix(input: &str) -> (Option<u32>, Option<u32>, &str) {
    let mut search_from = 0;
    while let Some(offset) = input[search_from..].find("#L") {
        let hash = search_from + offset;
        let rest = &input[hash + 2..];
        if let Some((line, span)) = parse_line_suffix(rest) {
            return (Some(line), span, &input[..hash]);
        }
        search_from = hash + 2;
    }
    (None, None, input)
}

fn parse_line_suffix(rest: &str) -> Option<(u32, Option<u32>)> {
    let (first, second) = match rest.split_once('-') {
        Some((first, second)) => (first, Some(second)),
        None => (rest, None),
    };
    if !is_decimal_1_to_10(first) {
        return None;
    }
    let line: u32 = first.parse::<u32>().ok()?;
    if line < 1 {
        return None;
    }
    match second {
        None => Some((line, None)),
        Some(raw) => {
            // The design's T22 exemplar is `x.md#L12-L14`, so the second bound may
            // repeat the `L` prefix; N2's regex omits it. Accept both spellings.
            let raw = raw.strip_prefix('L').unwrap_or(raw);
            if !is_decimal_1_to_10(raw) {
                return None;
            }
            let end: u32 = raw.parse::<u32>().ok()?;
            if end < line {
                return None;
            }
            let span = end
                .checked_sub(line)
                .and_then(|delta| delta.checked_add(1))?;
            Some((line, Some(span)))
        }
    }
}

fn is_decimal_1_to_10(input: &str) -> bool {
    !input.is_empty() && input.len() <= 10 && input.bytes().all(|byte| byte.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// normalize — pure, zero-IO by signature.
// ---------------------------------------------------------------------------

/// Normalise one `Evidence.locator` into the portable structure.
///
/// Order is part of the contract: line suffix (N2) -> scheme (N3, C1) ->
/// file-URI decode (N3a) -> shape (N4) -> lexical fold (N5, C2) -> root (N6) ->
/// placement/escape (N7). `Path` is only ever read as a string (`to_str`), never
/// with host semantics.
pub fn normalize(
    locator: &str,
    project_root: &Path,
    content_sha256: Option<String>,
    section_expected: Option<String>,
    origin_hint: Option<String>,
) -> Result<EvidenceLocatorV1, UnportableReason> {
    let original = locator.to_string();
    // N1: analysis uses the trimmed form; `original` stays byte-exact.
    let trimmed = locator.trim();

    // N2 runs before anything else so `x.md#L12` and `C:/x#L3` both work.
    let (line, span, without_suffix) = split_line_suffix(trimmed);

    // N6: the root is normalised with the same string rules; it must be absolute
    // and must not underflow.
    let root_str = project_root.to_str().ok_or(UnportableReason::NotPortable)?;
    let root = fold(root_str);
    if root.form == Form::Relative || root.underflow {
        return Err(UnportableReason::NotPortable);
    }

    // C1 / N3: scheme first. A non-`file` scheme is `Opaque` and never reaches
    // the path branch, so `https://example.com/x` can never become a relpath.
    let path_string = match split_scheme(without_suffix) {
        Some((scheme, rest)) => {
            if !scheme.eq_ignore_ascii_case("file") {
                return Ok(make_locator(
                    LocatorKind::Opaque,
                    None,
                    line,
                    span,
                    content_sha256,
                    section_expected,
                    origin_hint,
                    original,
                ));
            }
            match decode_file_uri(rest) {
                Some(decoded) if !decoded.is_empty() => decoded,
                _ => {
                    return Ok(make_locator(
                        LocatorKind::Opaque,
                        None,
                        line,
                        span,
                        content_sha256,
                        section_expected,
                        origin_hint,
                        original,
                    ));
                }
            }
        }
        None => without_suffix.to_string(),
    };

    // C2 / N4 / N5: shape and lexical fold BEFORE any root comparison.
    let folded = fold(&path_string);

    // C2 step ③: a relative locator whose first segment is `~` is opaque; home is
    // never expanded (that would make the record host-dependent).
    if folded.form == Form::Relative && folded.segments.first().map(String::as_str) == Some("~") {
        return Ok(make_locator(
            LocatorKind::Opaque,
            None,
            line,
            span,
            content_sha256,
            section_expected,
            origin_hint,
            original,
        ));
    }

    // N7: placement, still without IO.
    match folded.form {
        Form::Relative => {
            if folded.underflow {
                return Err(UnportableReason::PathEscape);
            }
            let relpath = folded.segments.join("/");
            if relpath.is_empty() {
                return Err(UnportableReason::NotPortable);
            }
            Ok(make_locator(
                LocatorKind::ProjectRelative,
                Some(relpath),
                line,
                span,
                content_sha256,
                section_expected,
                origin_hint,
                original,
            ))
        }
        form if form == root.form => {
            if folded.segments.len() >= root.segments.len()
                && folded.segments[..root.segments.len()] == root.segments[..]
            {
                let relpath = folded.segments[root.segments.len()..].join("/");
                if relpath.is_empty() {
                    return Err(UnportableReason::NotPortable);
                }
                Ok(make_locator(
                    LocatorKind::ProjectRelative,
                    Some(relpath),
                    line,
                    span,
                    content_sha256,
                    section_expected,
                    origin_hint,
                    original,
                ))
            } else {
                Ok(make_locator(
                    LocatorKind::ExternalAbsolute,
                    None,
                    line,
                    span,
                    content_sha256,
                    section_expected,
                    origin_hint,
                    original,
                ))
            }
        }
        _ => Ok(make_locator(
            LocatorKind::ExternalAbsolute,
            None,
            line,
            span,
            content_sha256,
            section_expected,
            origin_hint,
            original,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn make_locator(
    kind: LocatorKind,
    relpath: Option<String>,
    line: Option<u32>,
    span: Option<u32>,
    content_sha256: Option<String>,
    section_expected: Option<String>,
    origin_hint: Option<String>,
    original: String,
) -> EvidenceLocatorV1 {
    EvidenceLocatorV1 {
        kind,
        relpath,
        line,
        span,
        content_sha256,
        section_expected,
        origin_hint,
        original,
    }
}

pub fn resolve(
    loc: &EvidenceLocatorV1,
    local_root: &Path,
    io: &dyn LocatorIo,
    section: SectionCheck,
) -> ResolveReport {
    let mut stats = IoStats::default();
    let finish = |outcome: ResolveOutcome, section: SectionStatus, stats: IoStats| ResolveReport {
        outcome,
        section,
        io: stats,
    };

    // R0/R1: non-portable kinds and unusable relpaths return before any IO.
    match loc.kind {
        LocatorKind::ExternalAbsolute | LocatorKind::Opaque => {
            return finish(
                ResolveOutcome::Unportable(UnportableReason::NotPortable),
                SectionStatus::NotEvaluated,
                stats,
            );
        }
        LocatorKind::ProjectRelative => {}
    }
    let relpath = match &loc.relpath {
        Some(relpath) if !relpath.is_empty() => relpath,
        _ => {
            return finish(
                ResolveOutcome::Unportable(UnportableReason::NotPortable),
                SectionStatus::NotEvaluated,
                stats,
            );
        }
    };

    // R2: defensive checks (normalize already guarantees these), still zero IO.
    if relpath.contains('\\')
        || relpath.starts_with('/')
        || relpath.split('/').any(|segment| segment == "..")
    {
        return finish(
            ResolveOutcome::Unportable(UnportableReason::PathEscape),
            SectionStatus::NotEvaluated,
            stats,
        );
    }

    // R3/R4: canonicalize only through the injected IO.
    let canonical_root = match io.canonicalize(local_root) {
        Ok(path) => {
            stats.canonicalize_calls += 1;
            path
        }
        Err(_) => {
            stats.canonicalize_calls += 1;
            return finish(
                ResolveOutcome::IoFailed {
                    op: IoOp::Canonicalize,
                },
                SectionStatus::NotEvaluated,
                stats,
            );
        }
    };
    let target = canonical_root.join(relpath);
    let canonical_target = match io.canonicalize(&target) {
        Ok(path) => {
            stats.canonicalize_calls += 1;
            path
        }
        Err(error) => {
            stats.canonicalize_calls += 1;
            if error.kind() == std::io::ErrorKind::NotFound {
                return finish(ResolveOutcome::Missing, SectionStatus::NotEvaluated, stats);
            }
            return finish(
                ResolveOutcome::IoFailed {
                    op: IoOp::Canonicalize,
                },
                SectionStatus::NotEvaluated,
                stats,
            );
        }
    };

    // R5: a symlink resolving outside the root is a PathEscape, with hash_reads == 0.
    if !canonical_target.starts_with(&canonical_root) {
        return finish(
            ResolveOutcome::Unportable(UnportableReason::PathEscape),
            SectionStatus::NotEvaluated,
            stats,
        );
    }

    // R6: existence comes from the injected hasher, never from `is_file()`.
    let actual = match io.sha256_file(&canonical_target) {
        Ok(value) => {
            stats.hash_reads += 1;
            value
        }
        Err(error) => {
            stats.hash_reads += 1;
            if error.kind() == std::io::ErrorKind::NotFound {
                return finish(ResolveOutcome::Missing, SectionStatus::NotEvaluated, stats);
            }
            return finish(
                ResolveOutcome::IoFailed { op: IoOp::Hash },
                SectionStatus::NotEvaluated,
                stats,
            );
        }
    };

    // R7-R12: content first, section only when the content matches.
    match &loc.content_sha256 {
        None => finish(ResolveOutcome::Unbound, SectionStatus::NotEvaluated, stats),
        Some(expected) if expected == &actual => {
            let (outcome, status) = match (&loc.section_expected, &section) {
                (None, _) => (
                    ResolveOutcome::Current {
                        sha256: actual.clone(),
                    },
                    SectionStatus::NotBound,
                ),
                (Some(_), SectionCheck::Unavailable) => (
                    ResolveOutcome::Current {
                        sha256: actual.clone(),
                    },
                    SectionStatus::Unavailable,
                ),
                (Some(_), SectionCheck::Available { actual: None }) => (
                    ResolveOutcome::Current {
                        sha256: actual.clone(),
                    },
                    SectionStatus::Unavailable,
                ),
                (
                    Some(expected_section),
                    SectionCheck::Available {
                        actual: Some(actual_section),
                    },
                ) => {
                    if expected_section == actual_section {
                        (
                            ResolveOutcome::Current {
                                sha256: actual.clone(),
                            },
                            SectionStatus::Compared { equal: true },
                        )
                    } else {
                        (
                            ResolveOutcome::SectionDrift {
                                expected: Some(expected_section.clone()),
                                actual: Some(actual_section.clone()),
                            },
                            SectionStatus::Compared { equal: false },
                        )
                    }
                }
            };
            finish(outcome, status, stats)
        }
        Some(expected) => finish(
            ResolveOutcome::ContentDrift {
                expected: expected.clone(),
                actual,
            },
            SectionStatus::NotEvaluated,
            stats,
        ),
    }
}

// ---------------------------------------------------------------------------
// ⑨ PortableGroup boundary: four-part group key + fixed seven-level priority.
// ---------------------------------------------------------------------------

/// Four-part group key: `(work_item.external_key, relpath, line, span)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroupKey {
    pub work_item_external_key: String,
    pub relpath: String,
    pub line: Option<u32>,
    pub span: Option<u32>,
}

/// A record joins a group only when `kind == ProjectRelative` and `relpath` is non-empty.
pub fn group_key_for(work_item_external_key: &str, loc: &EvidenceLocatorV1) -> Option<GroupKey> {
    if loc.kind != LocatorKind::ProjectRelative {
        return None;
    }
    let relpath = loc.relpath.as_ref()?;
    if relpath.is_empty() {
        return None;
    }
    Some(GroupKey {
        work_item_external_key: work_item_external_key.to_string(),
        relpath: relpath.clone(),
        line: loc.line,
        span: loc.span,
    })
}

/// Fixed priority (high to low): `IoFailed > Unportable > SectionDrift > ContentDrift > Missing > Unbound > Current`.
pub fn outcome_priority(outcome: &ResolveOutcome) -> u8 {
    match outcome {
        ResolveOutcome::IoFailed { .. } => 7,
        ResolveOutcome::Unportable(_) => 6,
        ResolveOutcome::SectionDrift { .. } => 5,
        ResolveOutcome::ContentDrift { .. } => 4,
        ResolveOutcome::Missing => 3,
        ResolveOutcome::Unbound => 2,
        ResolveOutcome::Current { .. } => 1,
    }
}

pub struct GroupInput {
    pub work_item_external_key: String,
    pub evidence_id: Id,
    pub locator: EvidenceLocatorV1,
    pub outcome: ResolveOutcome,
    pub section_check_unavailable: bool,
}

/// Pure grouping: the group outcome takes the highest priority; ties break on
/// ascending `evidence_id`. Conservation: `sum(group ids) + unportable == inputs`.
pub fn group_records(inputs: &[GroupInput]) -> PortableReport {
    let mut buckets: std::collections::BTreeMap<GroupKey, Vec<usize>> =
        std::collections::BTreeMap::new();
    let mut unportable: Vec<UnportableEntry> = Vec::new();

    for (index, input) in inputs.iter().enumerate() {
        match group_key_for(&input.work_item_external_key, &input.locator) {
            Some(key) => buckets.entry(key).or_default().push(index),
            None => unportable.push(UnportableEntry {
                original: input.locator.original.clone(),
                reason: UnportableReason::NotPortable,
            }),
        }
    }

    let groups = buckets
        .into_iter()
        .map(|(key, indexes)| {
            let mut evidence_ids: Vec<Id> =
                indexes.iter().map(|&i| inputs[i].evidence_id).collect();
            evidence_ids.sort();

            let mut worst = indexes[0];
            for &index in &indexes[1..] {
                let candidate = outcome_priority(&inputs[index].outcome);
                let current = outcome_priority(&inputs[worst].outcome);
                if candidate > current
                    || (candidate == current
                        && inputs[index].evidence_id < inputs[worst].evidence_id)
                {
                    worst = index;
                }
            }

            PortableGroup {
                relpath: key.relpath,
                line: key.line,
                span: key.span,
                evidence_ids,
                outcome: inputs[worst].outcome.clone(),
                section_check_unavailable: indexes
                    .iter()
                    .any(|&i| inputs[i].section_check_unavailable),
            }
        })
        .collect();

    unportable.sort_by(|a, b| a.original.cmp(&b.original));
    PortableReport { groups, unportable }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(locator: &str, root: &str) -> Result<EvidenceLocatorV1, UnportableReason> {
        normalize(
            locator,
            Path::new(root),
            Some("a".repeat(64)),
            Some("section-a".to_string()),
            Some("device-x".to_string()),
        )
    }

    // T1 — POSIX and Windows spellings of the same tree give the same relpath,
    // with no `#[cfg]` branch: host-independent by construction.
    #[test]
    fn t1_posix_and_windows_styles_agree() {
        let posix = at("/home/alice/proj/src/a.md", "/home/alice/proj").expect("posix");
        assert_eq!(posix.kind, LocatorKind::ProjectRelative);
        assert_eq!(posix.relpath.as_deref(), Some("src/a.md"));

        let windows = at(r"C:\proj\src\a.md", r"C:\proj").expect("windows");
        assert_eq!(windows.kind, LocatorKind::ProjectRelative);
        assert_eq!(windows.relpath.as_deref(), Some("src/a.md"));

        let uri = at("file:///home/alice/proj/src/a.md", "/home/alice/proj").expect("file uri");
        assert_eq!(uri.relpath.as_deref(), Some("src/a.md"));

        let windows_uri = at("file:///C:/proj/src/a.md", r"C:\proj").expect("windows file uri");
        assert_eq!(windows_uri.relpath.as_deref(), Some("src/a.md"));
    }

    // T3 — a relative path that underflows is refused at normalize time (zero IO).
    #[test]
    fn t3_relative_underflow_is_path_escape() {
        for locator in ["../../etc/passwd", "a/../../b", "a/b/../../../c"] {
            assert_eq!(
                at(locator, "/home/alice/proj"),
                Err(UnportableReason::PathEscape),
                "{locator} must be refused"
            );
        }
        // Folding to the root itself is not portable either (relpath would be empty).
        assert_eq!(
            at("src/..", "/home/alice/proj"),
            Err(UnportableReason::NotPortable)
        );
    }

    // T4 — a non-`file` scheme is Opaque before any path analysis: no relpath, no escape.
    #[test]
    fn t4_non_file_scheme_is_opaque() {
        for locator in [
            "https://example.com/x",
            "s3://bucket/key",
            "ssh://host/x",
            "urn:isbn:0451450523",
            "HTTPS://Example.com/X",
        ] {
            let got = at(locator, "/home/alice/proj").expect("opaque is Ok");
            assert_eq!(got.kind, LocatorKind::Opaque, "{locator}");
            assert_eq!(got.relpath, None, "{locator}");
            assert_eq!(got.original, locator, "original must stay byte-exact");
        }
    }

    // T22 — the `#L` suffix: span is inclusive, malformed/overflowing suffixes
    // fall back to "whole string is the path".
    #[test]
    fn t22_line_suffix_parsing() {
        let ranged = at("src/a.md#L12-L14", "/home/alice/proj").expect("range");
        assert_eq!((ranged.line, ranged.span), (Some(12), Some(3)));
        assert_eq!(ranged.relpath.as_deref(), Some("src/a.md"));

        let single = at("src/a.md#L12", "/home/alice/proj").expect("single");
        assert_eq!((single.line, single.span), (Some(12), None));

        let reversed = at("src/a.md#L14-L12", "/home/alice/proj").expect("reversed");
        assert_eq!((reversed.line, reversed.span), (None, None));
        assert_eq!(reversed.relpath.as_deref(), Some("src/a.md#L14-L12"));

        let label = at("src/a#Label.md", "/home/alice/proj").expect("label");
        assert_eq!((label.line, label.span), (None, None));
        assert_eq!(label.relpath.as_deref(), Some("src/a#Label.md"));

        let overflow = at("src/a.md#L99999999999", "/home/alice/proj").expect("overflow");
        assert_eq!((overflow.line, overflow.span), (None, None));
    }

    // N4/N5 — `\\?\` prefixes, UNC, dot segments, separator equivalence, drive lowercase.
    #[test]
    fn n4_n5_lexical_folding() {
        let extended = at(r"\\?\C:\proj\src\a.md", r"C:\proj").expect("extended prefix");
        assert_eq!(extended.relpath.as_deref(), Some("src/a.md"));

        let unc = at(r"\\?\UNC\server\share\src\a.md", r"\\server\share").expect("unc prefix");
        assert_eq!(unc.relpath.as_deref(), Some("src/a.md"));

        let unc_uri = at("file://server/share/src/a.md", r"\\server\share").expect("unc uri");
        assert_eq!(unc_uri.relpath.as_deref(), Some("src/a.md"));

        let localhost = at("file://localhost/C:/proj/src/a.md", r"C:\proj").expect("localhost");
        assert_eq!(localhost.relpath.as_deref(), Some("src/a.md"));

        let mixed = at(r"C:\proj\a\.\b\..\c", r"c:/proj").expect("mixed separators");
        assert_eq!(mixed.relpath.as_deref(), Some("a/c"));

        let drive_case = at(r"c:\PROJ\src\a.md", r"C:\PROJ").expect("drive letter case");
        assert_eq!(drive_case.relpath.as_deref(), Some("src/a.md"));
        assert_eq!(drive_case.kind, LocatorKind::ProjectRelative);

        // C2 step ③: a leading `~` is opaque, never expanded through `HOME`.
        let home = at("~/proj/src/a.md", "/home/alice").expect("tilde");
        assert_eq!(home.kind, LocatorKind::Opaque);
        assert_eq!(home.relpath, None);
    }

    // R6/R9 — absolute outside the root stays absolute; a bad root is refused.
    #[test]
    fn n7_placement_and_root_guards() {
        let outside = at(r"D:\other\x.md", r"C:\proj").expect("outside");
        assert_eq!(outside.kind, LocatorKind::ExternalAbsolute);
        assert_eq!(outside.relpath, None);

        let other_style = at("/home/bob/x.md", "/home/alice/proj").expect("other style");
        assert_eq!(other_style.kind, LocatorKind::ExternalAbsolute);

        assert_eq!(
            at("src/a.md", "relative/root"),
            Err(UnportableReason::NotPortable)
        );
        assert_eq!(
            at(r"C:\proj", r"C:\proj"),
            Err(UnportableReason::NotPortable),
            "the root itself is not a file locator"
        );
    }

    // R8 — malformed `file:` payloads are Opaque, not errors and not paths.
    #[test]
    fn r8_malformed_file_uri_is_opaque() {
        for locator in ["file:///a%2.md", "file:///a%zz", "file:", "file://"] {
            let got = at(locator, "/home/alice/proj").expect("opaque is Ok");
            assert_eq!(got.kind, LocatorKind::Opaque, "{locator}");
            assert_eq!(got.relpath, None, "{locator}");
        }
    }

    // C1 regression guard: a Windows drive must never be mistaken for a scheme.
    #[test]
    fn c1_drive_letters_are_not_schemes() {
        let got = at(r"C:\proj\src\a.md", r"C:\proj").expect("drive");
        assert_eq!(got.kind, LocatorKind::ProjectRelative);
        let relative = at(r"C:proj\src\a.md", r"C:\proj").expect("drive-relative");
        assert_eq!(relative.kind, LocatorKind::ProjectRelative);
        assert_eq!(relative.relpath.as_deref(), Some("C:proj/src/a.md"));
    }

    // C1 ordering guard: if scheme detection ran after shape/escape analysis, these
    // would become relative paths and `http:../../etc/passwd` would underflow.
    #[test]
    fn c1_non_file_schemes_never_reach_escape() {
        for locator in [
            "http:../../etc/passwd",
            "node:fs/promises",
            "http://example.com/../../x",
        ] {
            let got = at(locator, "/home/alice/proj").expect("opaque is Ok");
            assert_eq!(got.kind, LocatorKind::Opaque, "{locator}");
            assert_eq!(got.relpath, None, "{locator}");
            assert_eq!(got.original, locator, "{locator}");
        }
    }

    // C2 ordering guard: lexical folding strips `\\?\` BEFORE shape/root analysis.
    // Without that, the raw `\\?\C:` prefix would classify as UNC and the locator
    // would be reported as ExternalAbsolute instead of ProjectRelative.
    #[test]
    fn c2_extended_prefix_folds_before_escape() {
        let got = at(r"\\?\C:\a\..\a\b.md", r"C:\").expect("extended prefix");
        assert_eq!(got.kind, LocatorKind::ProjectRelative);
        assert_eq!(got.relpath.as_deref(), Some("a/b.md"));
    }

    // N1 — `original` is the untrimmed, byte-exact input.
    #[test]
    fn n1_original_is_byte_exact() {
        let got = at("  https://example.com/x  ", "/home/alice/proj").expect("opaque");
        assert_eq!(got.original, "  https://example.com/x  ");
        assert_eq!(got.kind, LocatorKind::Opaque);
        assert_eq!(got.content_sha256, Some("a".repeat(64)));
        assert_eq!(got.section_expected.as_deref(), Some("section-a"));
        assert_eq!(got.origin_hint.as_deref(), Some("device-x"));
    }

    // ---- gate 2 item 3: injected IO + self-written `IoStats` (no `ReadCounter`) ----

    /// Any IO call panics: used to prove Unportable/Opaque paths cost zero IO.
    struct NeverIo;

    impl LocatorIo for NeverIo {
        fn canonicalize(&self, _path: &Path) -> std::io::Result<PathBuf> {
            panic!("resolve must not touch IO for this input");
        }
        fn sha256_file(&self, _path: &Path) -> std::io::Result<String> {
            panic!("resolve must not touch IO for this input");
        }
    }

    struct FakeIo {
        canonical: std::collections::BTreeMap<PathBuf, PathBuf>,
        hashes: std::collections::BTreeMap<PathBuf, String>,
    }

    impl LocatorIo for FakeIo {
        fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
            self.canonical
                .get(path)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "canonical"))
        }
        fn sha256_file(&self, path: &Path) -> std::io::Result<String> {
            self.hashes
                .get(path)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "hash"))
        }
    }

    fn resolve_with(locator: &str, root: &str, io: &dyn LocatorIo) -> ResolveReport {
        let loc = normalize(
            locator,
            Path::new(root),
            Some("actual".to_string()),
            None,
            None,
        )
        .expect("normalize");
        resolve(&loc, Path::new(root), io, SectionCheck::Unavailable)
    }

    // T2/T4 - non-portable kinds cost zero IO and report io == default.
    #[test]
    fn t2_t4_unportable_costs_no_io() {
        let external = resolve_with("/etc/passwd", "/root", &NeverIo);
        assert_eq!(
            external.outcome,
            ResolveOutcome::Unportable(UnportableReason::NotPortable)
        );
        assert_eq!(external.io, IoStats::default());

        let opaque = resolve_with("https://example.com/x", "/root", &NeverIo);
        assert_eq!(
            opaque.outcome,
            ResolveOutcome::Unportable(UnportableReason::NotPortable)
        );
        assert_eq!(opaque.section, SectionStatus::NotEvaluated);
        assert_eq!(opaque.io, IoStats::default());
    }

    // Item 3 - `resolve` writes its own counts: 2 canonicalize calls + 1 hash read.
    #[test]
    fn resolve_counts_io_on_current() {
        let mut canonical = std::collections::BTreeMap::new();
        canonical.insert(PathBuf::from("/root"), PathBuf::from("/real/root"));
        canonical.insert(
            PathBuf::from("/real/root/src/a.md"),
            PathBuf::from("/real/root/src/a.md"),
        );
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert(PathBuf::from("/real/root/src/a.md"), "actual".to_string());
        let io = FakeIo { canonical, hashes };

        let report = resolve_with("src/a.md", "/root", &io);
        assert_eq!(
            report.outcome,
            ResolveOutcome::Current {
                sha256: "actual".to_string()
            }
        );
        assert_eq!(
            report.io,
            IoStats {
                canonicalize_calls: 2,
                hash_reads: 1
            }
        );
    }

    // Boundary: a different measured sha is ContentDrift, never Current/Missing.
    #[test]
    fn resolve_reports_content_drift() {
        let mut canonical = std::collections::BTreeMap::new();
        canonical.insert(PathBuf::from("/root"), PathBuf::from("/root"));
        canonical.insert(
            PathBuf::from("/root/src/a.md"),
            PathBuf::from("/root/src/a.md"),
        );
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert(PathBuf::from("/root/src/a.md"), "actual".to_string());
        let io = FakeIo { canonical, hashes };

        let loc = normalize(
            "src/a.md",
            Path::new("/root"),
            Some("expected".to_string()),
            None,
            None,
        )
        .expect("normalize");
        let report = resolve(&loc, Path::new("/root"), &io, SectionCheck::Unavailable);
        assert_eq!(
            report.outcome,
            ResolveOutcome::ContentDrift {
                expected: "expected".to_string(),
                actual: "actual".to_string()
            }
        );
    }

    // ---- gate 2 item 3: PortableGroup boundary ----

    fn group_locator(relpath: &str, line: Option<u32>) -> EvidenceLocatorV1 {
        EvidenceLocatorV1 {
            kind: LocatorKind::ProjectRelative,
            relpath: Some(relpath.to_string()),
            line,
            span: None,
            content_sha256: None,
            section_expected: None,
            origin_hint: None,
            original: format!("{relpath}#L{line:?}"),
        }
    }

    fn opaque_locator(original: &str) -> EvidenceLocatorV1 {
        EvidenceLocatorV1 {
            kind: LocatorKind::Opaque,
            relpath: None,
            line: None,
            span: None,
            content_sha256: None,
            section_expected: None,
            origin_hint: None,
            original: original.to_string(),
        }
    }

    // T25 - conservation and the four-part key: line/span split groups; opaque never joins.
    #[test]
    fn t25_group_conservation_and_key_quad() {
        let inputs = vec![
            GroupInput {
                work_item_external_key: "W-1".to_string(),
                evidence_id: Id::from(1u128),
                locator: group_locator("a/b.md", None),
                outcome: ResolveOutcome::Current {
                    sha256: "s1".to_string(),
                },
                section_check_unavailable: false,
            },
            GroupInput {
                work_item_external_key: "W-1".to_string(),
                evidence_id: Id::from(2u128),
                locator: group_locator("a/b.md", None),
                outcome: ResolveOutcome::Unbound,
                section_check_unavailable: true,
            },
            GroupInput {
                work_item_external_key: "W-1".to_string(),
                evidence_id: Id::from(3u128),
                locator: group_locator("a/b.md", Some(3)),
                outcome: ResolveOutcome::Missing,
                section_check_unavailable: false,
            },
            GroupInput {
                work_item_external_key: "W-1".to_string(),
                evidence_id: Id::from(4u128),
                locator: opaque_locator("https://example.com/x"),
                outcome: ResolveOutcome::Unportable(UnportableReason::NotPortable),
                section_check_unavailable: false,
            },
        ];

        let report = group_records(&inputs);
        assert_eq!(report.groups.len(), 2, "line is part of the key");
        assert_eq!(report.unportable.len(), 1, "opaque never joins a group");

        let grouped: usize = report.groups.iter().map(|g| g.evidence_ids.len()).sum();
        assert_eq!(
            grouped + report.unportable.len(),
            inputs.len(),
            "conservation"
        );

        let merged = report
            .groups
            .iter()
            .find(|g| g.line.is_none())
            .expect("merged group");
        assert_eq!(
            merged.evidence_ids,
            vec![Id::from(1u128), Id::from(2u128)],
            "ids sorted ascending"
        );
        assert_eq!(merged.outcome, ResolveOutcome::Unbound, "priority 2 > 1");
        assert!(merged.section_check_unavailable);
    }

    // T26 - fixed seven-level priority and deterministic tie handling.
    #[test]
    fn t26_group_outcome_priority_is_fixed() {
        let io_failed = outcome_priority(&ResolveOutcome::IoFailed { op: IoOp::Hash });
        let unportable =
            outcome_priority(&ResolveOutcome::Unportable(UnportableReason::PathEscape));
        let section = outcome_priority(&ResolveOutcome::SectionDrift {
            expected: Some("e".to_string()),
            actual: Some("a".to_string()),
        });
        let content = outcome_priority(&ResolveOutcome::ContentDrift {
            expected: "e".to_string(),
            actual: "a".to_string(),
        });
        let missing = outcome_priority(&ResolveOutcome::Missing);
        let unbound = outcome_priority(&ResolveOutcome::Unbound);
        let current = outcome_priority(&ResolveOutcome::Current {
            sha256: "s".to_string(),
        });
        assert!(io_failed > unportable);
        assert!(unportable > section);
        assert!(section > content);
        assert!(content > missing);
        assert!(missing > unbound);
        assert!(unbound > current);

        let inputs = vec![
            GroupInput {
                work_item_external_key: "W-1".to_string(),
                evidence_id: Id::from(10u128),
                locator: group_locator("a/b.md", None),
                outcome: ResolveOutcome::ContentDrift {
                    expected: "e".to_string(),
                    actual: "a".to_string(),
                },
                section_check_unavailable: false,
            },
            GroupInput {
                work_item_external_key: "W-1".to_string(),
                evidence_id: Id::from(11u128),
                locator: group_locator("a/b.md", None),
                outcome: ResolveOutcome::Missing,
                section_check_unavailable: false,
            },
            GroupInput {
                work_item_external_key: "W-1".to_string(),
                evidence_id: Id::from(12u128),
                locator: group_locator("a/b.md", None),
                outcome: ResolveOutcome::Current {
                    sha256: "s".to_string(),
                },
                section_check_unavailable: false,
            },
        ];
        let report = group_records(&inputs);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(
            report.groups[0].outcome,
            ResolveOutcome::ContentDrift {
                expected: "e".to_string(),
                actual: "a".to_string()
            }
        );
    }
}
