//! Unattended, resumable takeover of an existing project (originoneai/awr#620).
//!
//! Takes a project that has no AWR runtime to an accepted `awr init` and an organization state, in steps
//! that are persisted one by one, with no question asked on the way. It is a thin layer over the CLI's own
//! commands: every step runs this binary again through argv, so each one behaves exactly as when a person
//! runs it. The design and the cases come from the DeepSeek Harness plugin's module
//! (`originoneai/dsh-awr`, `lib/takeover.js`), where it was proven on real projects with a real model.
//!
//! ```text
//! preflight  the project exists and has no AWR runtime (or `--adopt`), no active session, Git HEAD noted
//! preview    `init` preview and `init --write-draft` into a state directory outside the project
//! review     the reviewer command turns the generated draft into a reviewed one (none: applied as generated)
//! accept     `init --from-draft` preview, a check of what it would write, then `--accept --expected-preview`
//! organize   `intake inspect`
//! receipt    `.awr/takeover/receipt.json`: what was mapped, created and reviewed, what awaits confirmation
//! ```
//!
//! Everything before accept leaves the project byte-identical. After accept it gains `.awr/` and the
//! `.gitignore` entries AWR's own init lists in its preview; a preview that lists any other path outside
//! `.awr/` stops the run before accept. Nothing is committed or pushed. A run that cannot go on stops with a
//! recorded reason and is resumed by running the command again; a step that finished is not repeated.
use awr_core::{Error, Result};
use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RECORD_VERSION: u64 = 1;
const STEPS: [&str; 6] = [
    "preflight",
    "preview",
    "review",
    "accept",
    "organize",
    "receipt",
];
const MAX_GENERATED_FILE_BYTES: usize = 1024 * 1024;
const MAX_REVIEWER_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// What AWR's init may add outside `.awr/`: its ignore entries for runtime files.
const ALLOWED_OUTSIDE_AWR: [&str; 1] = [".gitignore"];
/// Stops at `accept` that come from the reviewed draft, before anything was written.
const REVIEW_REJECTED_AT_ACCEPT: [&str; 3] =
    ["ambiguous_authority", "unexpected_write", "cannot_apply"];
const SETTLED_STATES: [&str; 5] = [
    "ready",
    "blocked",
    "awaiting_verification",
    "completed",
    "closed_without_completion",
];

#[derive(Debug, Args)]
pub struct TakeoverArgs {
    #[command(subcommand)]
    pub command: Option<TakeoverCommand>,
    /// Program that reviews the generated draft (JSON on stdin and stdout).
    #[arg(long)]
    pub reviewer: Option<PathBuf>,
    /// One argument for the reviewer program; repeat for several.
    #[arg(
        long = "reviewer-arg",
        requires = "reviewer",
        allow_hyphen_values = true
    )]
    pub reviewer_args: Vec<String>,
    /// Seconds the reviewer may take before the run stops.
    #[arg(long, default_value_t = 900, requires = "reviewer")]
    pub review_timeout_secs: u64,
    /// Source list the owner chose (a manifest); the takeover maps and reads exactly these sources.
    #[arg(long)]
    pub manifest: Option<PathBuf>,
    /// Continue organizing an already initialized project that has no active session.
    #[arg(long)]
    pub adopt: bool,
    /// Preview, review and accept rounds when the project keeps changing.
    #[arg(long, default_value_t = 3)]
    pub max_rounds: u32,
    /// Directory for the run record and drafts, outside the project.
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    /// Start a new run although the last one completed.
    #[arg(long)]
    pub rerun: bool,
}

#[derive(Debug, Subcommand)]
pub enum TakeoverCommand {
    /// Show the record and receipt of the last run for this project.
    Status,
}

/// Why a step ended the run without it being an error of the command: a recorded reason.
#[derive(Debug, Clone)]
struct Stop {
    reason: String,
    message: String,
    detail: Option<Value>,
}

impl Stop {
    fn new(reason: &str, message: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            message: message.into(),
            detail: None,
        }
    }
    fn with(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

/// A step's way out: a recorded stop, a request to start over from preview, or an unexpected failure.
enum StepEnd {
    Stop(Stop),
    Retry(String),
    Failed(String),
}

impl From<Stop> for StepEnd {
    fn from(stop: Stop) -> Self {
        Self::Stop(stop)
    }
}

impl From<std::io::Error> for StepEnd {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<serde_json::Error> for StepEnd {
    fn from(error: serde_json::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

// --- running awr -------------------------------------------------------------------------------------------

/// What a child `awr` reported when it failed: the error report it printed, as data.
#[derive(Debug, Clone)]
struct Failure {
    code: String,
    message: String,
    details: Value,
}

struct Awr {
    exe: PathBuf,
    root: PathBuf,
}

impl Awr {
    fn run(&self, args: &[&str]) -> std::result::Result<Value, Failure> {
        let output = Command::new(&self.exe)
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| Failure {
                code: "invalid_output".into(),
                message: format!("could not run awr: {error}"),
                details: Value::Null,
            })?;
        if output.status.success() {
            return serde_json::from_slice(&output.stdout).map_err(|_| Failure {
                code: "invalid_output".into(),
                message: format!("awr {} printed output that is not JSON", args.join(" ")),
                details: Value::Null,
            });
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let report = stderr
            .lines()
            .rev()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .find(|value| value.get("code").is_some());
        Err(match report {
            Some(report) => Failure {
                code: report["code"].as_str().unwrap_or("unknown").into(),
                message: report["message"].as_str().unwrap_or_default().into(),
                details: report.get("details").cloned().unwrap_or(Value::Null),
            },
            None => Failure {
                code: "invalid_output".into(),
                message: format!("awr {} failed without an error report", args.join(" ")),
                details: Value::Null,
            },
        })
    }
}

fn describe(failure: &Failure) -> String {
    format!("{}: {}", failure.code, failure.message)
}

/// Why an error from awr ends the run, or `None` when it is not one the command knows.
fn classify(failure: &Failure, root: &Path) -> Stop {
    if failure.code == "RuleViolation" && failure.message.contains("sensitive content") {
        let location = &failure.details["location"];
        // Where it is, never what it is.
        let detail = json!({
            "category": failure.details["category"],
            "path": location["locator"].as_str().and_then(|locator| relative_locator(locator, root)),
            "line": location["line"],
        });
        let next = failure.details["next_action"]
            .as_str()
            .unwrap_or(&failure.message);
        return Stop::new(
            "secret_boundary",
            format!(
                "AWR refused source content that looks like a {}: {next}",
                failure.details["category"].as_str().unwrap_or("secret")
            ),
        )
        .with(detail);
    }
    if failure.code == "SourceConflict" && failure.message.contains("multiple authority") {
        return Stop::new(
            "ambiguous_authority",
            format!(
                "{}. The reviewed draft has to choose: add the authoritative source of each ambiguous domain to authority_mapping.sources and clear ambiguous_domains.",
                failure.message
            ),
        );
    }
    if failure.code == "SourceConflict" {
        return Stop::new("inventory_changed", failure.message.clone());
    }
    if failure.code == "invalid_output" {
        return Stop::new("invalid_output", failure.message.clone());
    }
    Stop::new("awr_error", describe(failure)).with(json!({ "code": failure.code }))
}

/// A path in one comparable spelling: forward slashes, no `\\?\` verbatim prefix, and no leading slash before a
/// drive letter (a file URL spells `C:\x` as `/C:/x`).
fn normalized_path(path: &str) -> String {
    let mut text = path.replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_string();
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        text.remove(0);
    }
    text
}

/// A path from an AWR locator (`file:///...`) relative to the project, so that no absolute path of this machine
/// is kept. AWR reports real paths (`/private/var/...` for `/var/...` on macOS, `C:/...` for a `\\?\C:\...` root
/// on Windows), so both spellings of the root are tried on normalized text; drive letters ignore case on Windows.
fn relative_locator(locator: &str, root: &Path) -> Option<String> {
    let path = locator.strip_prefix("file://").unwrap_or(locator);
    let path = normalized_path(&percent_decode(path.split('#').next().unwrap_or(path)));
    let mut roots = vec![root.to_path_buf()];
    if let Ok(real) = fs::canonicalize(root) {
        roots.push(real);
    }
    for base in roots {
        let base = normalized_path(&base.to_string_lossy());
        let base = base.trim_end_matches('/');
        let under = path.len() > base.len()
            && path.is_char_boundary(base.len())
            && path.as_bytes()[base.len()] == b'/'
            && if cfg!(windows) {
                path[..base.len()].eq_ignore_ascii_case(base)
            } else {
                &path[..base.len()] == base
            };
        if under {
            return Some(path[base.len() + 1..].to_string());
        }
    }
    Some(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(value) = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn inside_awr(path: &str) -> bool {
    path == ".awr" || path.starts_with(".awr/")
}

/// Paths of an init preview that are neither under `.awr/` nor AWR's own ignore entries. A path that climbs out of
/// the project (`..`) or is absolute never counts as inside `.awr/`.
fn unexpected_writes(writes: &[Value]) -> Vec<String> {
    writes
        .iter()
        .filter_map(|write| write["path"].as_str())
        .filter(|path| {
            let escapes = Path::new(path).is_absolute()
                || Path::new(path)
                    .components()
                    .any(|c| matches!(c, Component::ParentDir));
            escapes || (!inside_awr(path) && !ALLOWED_OUTSIDE_AWR.contains(path))
        })
        .map(String::from)
        .collect()
}

// --- the guards on a reviewed draft --------------------------------------------------------------------------

/// Byte offset of the line that starts the top-level `work_items:` key.
fn work_items_start(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_text = line.trim_end_matches(['\n', '\r']);
        if let Some(rest) = line_text.strip_prefix("work_items")
            && rest.trim_start().starts_with(':')
        {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

/// Whether a ledger text has at least one work item under its top-level `work_items:` key.
fn has_work_items(text: &str) -> bool {
    let Some(start) = work_items_start(text) else {
        return false;
    };
    let mut lines = text[start..].lines();
    let key_line = lines.next().unwrap_or_default();
    let inline = key_line
        .split_once(':')
        .map(|(_, rest)| rest.trim())
        .unwrap_or_default();
    if inline.starts_with('[') {
        return inline.replace(' ', "") != "[]";
    }
    lines.any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("- ") || trimmed == "-"
    })
}

/// A ledger with no work items cannot be bound to an AWR session or listed as ready work (a real model did write
/// `work_items: []`, removing the generated intake item). When the reviewer returns a YAML ledger without work items
/// and the draft's ledger had some, the draft's `work_items:` section is put back, as long as nothing but the empty
/// key follows the reviewer's goals. Returns the paths that were restored.
fn keep_intake_work(
    files: &mut Map<String, Value>,
    draft_files: &Map<String, Value>,
) -> Vec<String> {
    let mut kept = Vec::new();
    let paths: Vec<String> = files.keys().cloned().collect();
    for path in paths {
        let lower = path.to_ascii_lowercase();
        if !(lower.ends_with(".yaml") || lower.ends_with(".yml")) {
            continue;
        }
        let (Some(text), Some(original)) = (
            files[&path].as_str().map(str::to_string),
            draft_files.get(&path).and_then(Value::as_str),
        ) else {
            continue;
        };
        if has_work_items(&text) || !has_work_items(original) {
            continue;
        }
        let cut = work_items_start(&text);
        if let Some(cut) = cut
            && text[cut..]
                .lines()
                .skip(1)
                .any(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        {
            continue;
        }
        let head = cut.map_or(text.as_str(), |cut| &text[..cut]).trim_end();
        let section = original[work_items_start(original).unwrap_or(0)..].trim_end();
        files.insert(path.clone(), Value::String(format!("{head}\n{section}\n")));
        kept.push(path);
    }
    kept
}

/// Goals are the owner's to confirm. Whatever the reviewer wrote, a goal in the intake files is at most a candidate:
/// `status=active`/`confirmed` in a Markdown goal heading and `status: active`/`confirmed` in the `goals:` part of a
/// YAML ledger are lowered to `candidate`. Work items are not touched. Returns how many statuses were lowered.
fn cap_goal_statuses(files: &mut Map<String, Value>) -> usize {
    let mut lowered = 0;
    let paths: Vec<String> = files.keys().cloned().collect();
    for path in paths {
        let Some(text) = files[&path].as_str().map(str::to_string) else {
            continue;
        };
        let lower = path.to_ascii_lowercase();
        let next = if lower.ends_with(".md") {
            let (text, count) = cap_markdown_goals(&text);
            lowered += count;
            text
        } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
            let cut = work_items_start(&text).unwrap_or(text.len());
            let (goals, count) = cap_yaml_goals(&text[..cut]);
            lowered += count;
            format!("{goals}{}", &text[cut..])
        } else {
            continue;
        };
        files.insert(path, Value::String(next));
    }
    lowered
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `{#id status=active}` and `status=confirmed` inside a heading's braces become `status=candidate`.
fn cap_markdown_goals(text: &str) -> (String, usize) {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut index = 0;
    while index < text.len() {
        if lower[index..].starts_with("{#") {
            let end = text[index..]
                .find(['}', '\n'])
                .map_or(text.len(), |at| index + at);
            let span = &text[index..end];
            let span_lower = &lower[index..end];
            let mut rewritten = String::new();
            let mut cursor = 0;
            let mut search = 0;
            while let Some(at) = span_lower[search..].find("status=") {
                let start = search + at;
                let before_ok = start == 0 || !is_word_byte(span.as_bytes()[start - 1]);
                let value_start = start + "status=".len();
                let word = ["active", "confirmed"].into_iter().find(|word| {
                    span_lower[value_start..].starts_with(word)
                        && span
                            .as_bytes()
                            .get(value_start + word.len())
                            .is_none_or(|b| !is_word_byte(*b))
                });
                if before_ok && let Some(word) = word {
                    rewritten.push_str(&span[cursor..value_start]);
                    rewritten.push_str("candidate");
                    cursor = value_start + word.len();
                    count += 1;
                    search = cursor;
                } else {
                    search = value_start;
                }
            }
            rewritten.push_str(&span[cursor..]);
            out.push_str(&rewritten);
            index = end;
            continue;
        }
        let ch_len = text[index..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[index..index + ch_len]);
        index += ch_len;
        let _ = bytes;
    }
    (out, count)
}

/// `status: active` and `status: confirmed` lines (with an optional list dash and quotes) become `candidate`.
fn cap_yaml_goals(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let ending = &line[body.len()..];
        let indent = body.len() - body.trim_start().len();
        let mut rest = body[indent..].trim_start_matches("- ").trim_start();
        let dash_len = body[indent..].len() - rest.len();
        if let Some(after) = rest.strip_prefix("status") {
            let after = after.trim_start();
            if let Some(value) = after.strip_prefix(':') {
                let value = value.trim();
                let (quote, bare) = match value.chars().next() {
                    Some(q @ ('"' | '\'')) if value.len() >= 2 && value.ends_with(q) => {
                        (Some(q), &value[1..value.len() - 1])
                    }
                    _ => (None, value),
                };
                if bare.eq_ignore_ascii_case("active") || bare.eq_ignore_ascii_case("confirmed") {
                    let prefix = &body[..indent + dash_len];
                    let q = quote.map(String::from).unwrap_or_default();
                    out.push_str(&format!("{prefix}status: {q}candidate{q}{ending}"));
                    count += 1;
                    continue;
                }
            }
            rest = "";
        }
        let _ = rest;
        out.push_str(line);
    }
    (out, count)
}

/// Reject generated files that leave `.awr/intake/`, are not text, or are larger than 1 MiB.
fn validate_files(files: &Map<String, Value>) -> Option<String> {
    if files.is_empty() {
        return Some("generated_files must be an object of intake file text".into());
    }
    for (path, text) in files {
        let p = Path::new(path);
        let escapes = p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir));
        if !path.starts_with(".awr/intake/") || escapes {
            return Some(format!("generated file \"{path}\" is outside .awr/intake/"));
        }
        let Some(text) = text.as_str() else {
            return Some(format!("generated file \"{path}\" must be text"));
        };
        if text.len() > MAX_GENERATED_FILE_BYTES {
            return Some(format!("generated file \"{path}\" is larger than 1 MiB"));
        }
    }
    None
}

/// The reviewer's answer from its output: the whole output, or the last ```json fence, or the outermost braces.
fn extract_json(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        return Some(value);
    }
    if let Some(at) = text.rfind("```json") {
        let body = &text[at + "```json".len()..];
        let body = body.split("```").next().unwrap_or(body);
        if let Ok(value) = serde_json::from_str::<Value>(body.trim()) {
            return Some(value);
        }
    }
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(start..=end)?).ok()
}

// --- state ---------------------------------------------------------------------------------------------------

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn state_base(args: &TakeoverArgs) -> PathBuf {
    if let Some(dir) = &args.state_dir {
        return dir.clone();
    }
    if let Ok(dir) = std::env::var("AWR_TAKEOVER_STATE")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    home.join("awr").join("takeovers")
}

fn run_dir_for(root: &Path, base: &Path) -> PathBuf {
    let name: String = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let digest = format!("{:x}", Sha256::digest(root.to_string_lossy().as_bytes()));
    base.join(format!("{name}-{}", &digest[..12]))
}

fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(temporary, path)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(true)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

enum Locked {
    Held(Lock),
    Busy(u64),
}

/// A lock older than this is replaced whatever its process id says: process ids are reused, and hosts without a
/// liveness check (Windows) have only the age to go by. A run takes minutes; the reviewer has its own timeout.
const STALE_LOCK_MS: u64 = 2 * 60 * 60 * 1000;

/// Whether a recorded lock still belongs to a running takeover.
fn lock_is_live(held: &Value, now: u64, alive: impl Fn(u32) -> bool) -> Option<u64> {
    let pid = held["pid"].as_u64()?;
    let fresh = now.saturating_sub(held["at"].as_u64().unwrap_or(0)) < STALE_LOCK_MS;
    (pid != u64::from(std::process::id()) && fresh && alive(pid as u32)).then_some(pid)
}

fn acquire_lock(run_dir: &Path) -> std::io::Result<Locked> {
    let path = run_dir.join("lock");
    if let Some(pid) =
        read_json(&path).and_then(|held| lock_is_live(&held, now_ms(), process_alive))
    {
        return Ok(Locked::Busy(pid));
    }
    write_json(&path, &json!({ "pid": std::process::id(), "at": now_ms() }))?;
    Ok(Locked::Held(Lock(path)))
}

fn git_head(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|head| !head.is_empty())
}

// --- the steps -----------------------------------------------------------------------------------------------

struct Run<'a> {
    awr: Awr,
    root: PathBuf,
    run_dir: PathBuf,
    record: Value,
    args: &'a TakeoverArgs,
}

impl Run<'_> {
    fn step(&self, name: &str) -> &Value {
        &self.record["steps"][name]
    }

    fn save(&mut self) -> std::io::Result<()> {
        self.record["updated_at"] = json!(now_ms());
        write_json(&self.run_dir.join("takeover.json"), &self.record)
    }

    fn mark(&mut self, step: &str, fields: Value) -> std::io::Result<()> {
        if let (Some(target), Some(extra)) = (
            self.record["steps"][step].as_object_mut(),
            fields.as_object(),
        ) {
            target.extend(extra.clone());
        } else {
            self.record["steps"][step] = fields;
        }
        self.save()
    }

    fn preflight(&mut self) -> std::result::Result<Value, StepEnd> {
        match fs::metadata(&self.root) {
            Ok(info) if info.is_dir() => {}
            Ok(_) => {
                return Err(Stop::new(
                    "not_a_directory",
                    format!("{} is not a directory", self.root.display()),
                )
                .into());
            }
            Err(_) => {
                return Err(Stop::new(
                    "not_a_directory",
                    format!("{} does not exist", self.root.display()),
                )
                .into());
            }
        }
        let initialized = self.root.join(".awr").join("project.toml").is_file();
        let mut adopted = false;
        if initialized {
            let sessions = self
                .awr
                .run(&["session", "list", "--active"])
                .map_err(|f| StepEnd::Stop(classify(&f, &self.root)))?;
            let open: Vec<&Value> = sessions["sessions"]
                .as_array()
                .map(|list| list.iter().filter(|s| s["status"] == "active").collect())
                .unwrap_or_default();
            if !open.is_empty() {
                let detail: Vec<Value> = open
                    .iter()
                    .take(5)
                    .map(|s| json!({ "id": s["id"], "agent": s["agent_id"], "started_at": s["started_at"] }))
                    .collect();
                return Err(Stop::new(
                    "active_session",
                    format!("The project already has {} active AWR session(s); a takeover does not run next to one.", open.len()),
                )
                .with(json!({ "sessions": detail }))
                .into());
            }
            if !self.args.adopt {
                return Err(Stop::new(
                    "already_initialized",
                    "The project is already initialized; a takeover starts from a project without an AWR runtime (or use --adopt).",
                )
                .into());
            }
            adopted = true;
        }
        Ok(
            json!({ "initialized": initialized, "adopted": adopted, "git_head": git_head(&self.root) }),
        )
    }

    /// A stop of the preview step; a credential stop says how to go on with a manifest.
    fn preview_stop(&self, failure: &Failure) -> StepEnd {
        let mut stop = classify(failure, &self.root);
        if stop.reason == "secret_boundary" {
            stop.message.push_str(if self.args.manifest.is_some() {
                " The manifest lists this file: redact the value, or leave the file out of the manifest."
            } else {
                " Redact the value, or list the sources AWR may read in a manifest and run the takeover again with --manifest <file>."
            });
        }
        StepEnd::Stop(stop)
    }

    fn preview(&mut self) -> std::result::Result<Value, StepEnd> {
        let draft_path = self.run_dir.join("draft.json");
        // awr will not overwrite a draft file, and a draft left by an interrupted attempt is this run's own.
        let _ = fs::remove_file(&draft_path);
        let draft_arg = draft_path.to_string_lossy().into_owned();
        let manifest = self
            .args
            .manifest
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        // With a manifest the draft is written by the first command: it maps and reads exactly the listed sources,
        // and its preview is the one to judge. Without one the preview comes first and the draft after it.
        let shown = match &manifest {
            Some(manifest) => {
                self.awr
                    .run(&["init", "--manifest", manifest, "--write-draft", &draft_arg])
            }
            None => self.awr.run(&["init"]),
        }
        .map_err(|failure| {
            // AWR could not make sense of a source it found (for example a ledger that is not valid YAML).
            if failure.code == "InvalidInput" {
                return StepEnd::Stop(
                    Stop::new(
                        "source_unreadable",
                        format!("AWR cannot read the project's sources: {}", failure.message),
                    )
                    .with(json!({ "code": "InvalidInput" })),
                );
            }
            self.preview_stop(&failure)
        })?;
        let issues = shown["preview"]["source_issues"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !issues.is_empty() {
            let sample: Vec<Value> = issues
                .iter()
                .take(5)
                .map(|issue| {
                    json!({
                        "domain": issue["domain"],
                        "code": issue["error"]["code"],
                        "message": issue["error"]["message"],
                        "path": issue["source"]["path"],
                    })
                })
                .collect();
            return Err(Stop::new(
                "source_unreadable",
                format!(
                    "AWR cannot read {} source(s) of this project.",
                    issues.len()
                ),
            )
            .with(json!({ "issues": sample }))
            .into());
        }
        if shown["status"] != "preview" || shown["preview"]["can_apply"] != true {
            return Err(Stop::new(
                "cannot_apply",
                "AWR reports that the initialization cannot be applied to this project.",
            )
            .with(json!({ "status": shown["status"] }))
            .into());
        }
        if manifest.is_none() {
            self.awr
                .run(&["init", "--write-draft", &draft_arg])
                .map_err(|failure| self.preview_stop(&failure))?;
        }
        let draft = read_json(&draft_path).unwrap_or(Value::Null);
        if !draft["inventory_fingerprint"].is_string() || !draft["generated_files"].is_object() {
            return Err(Stop::new(
                "invalid_draft",
                "awr wrote a draft this command does not recognize.",
            )
            .into());
        }
        Ok(json!({
            "draft_path": draft_path,
            "manifest": manifest,
            "inventory_fingerprint": draft["inventory_fingerprint"],
            "inventory": draft["inventory"].as_array().map_or(0, Vec::len),
            "sources": summarize_sources(&draft["authority_mapping"]),
            "ambiguous_domains": draft["ambiguous_domains"].clone(),
            "gaps": draft["gaps"].as_array().map(|g| g.iter().take(10).cloned().collect::<Vec<_>>()),
            "generated_files": draft["generated_files"].as_object().map(|f| f.keys().cloned().collect::<Vec<_>>()),
            "git": draft["git"].clone(),
        }))
    }

    fn review(&mut self) -> std::result::Result<Value, StepEnd> {
        let draft_path = self.run_dir.join("draft.json");
        let draft = read_json(&draft_path)
            .ok_or_else(|| StepEnd::Failed("the draft of this run is missing".into()))?;
        let round = self.record["rounds"].as_u64().unwrap_or(1);
        let mut reviewed = draft.clone();
        let mut notes: Option<String> = None;
        if let Some(program) = self.args.reviewer.clone() {
            let request = json!({
                "project_root": self.root,
                "draft": draft,
                "draft_path": draft_path,
                "round": round,
                "sources": summarize_sources(&draft["authority_mapping"]),
                "ambiguous_domains": draft["ambiguous_domains"].clone(),
            });
            let answer = run_reviewer(
                &program,
                &self.args.reviewer_args,
                &request,
                Duration::from_secs(self.args.review_timeout_secs),
            )
            .map_err(StepEnd::Stop)?;
            let mut returned = answer["generated_files"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            if returned.is_empty() {
                return Err(Stop::new(
                    "review_failed",
                    "The reviewer's answer has no generated_files.",
                )
                .into());
            }
            if let Some(problem) = validate_files(&returned) {
                return Err(Stop::new("review_rejected", problem).into());
            }
            let lowered = cap_goal_statuses(&mut returned);
            let draft_files = draft["generated_files"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            let kept = keep_intake_work(&mut returned, &draft_files);
            let mut merged = draft_files;
            merged.extend(returned);
            reviewed["generated_files"] = Value::Object(merged);
            // The reviewer may settle an ambiguity by choosing the authoritative source of a domain.
            for key in ["authority_mapping", "ambiguous_domains"] {
                if let Some(value) = answer.get(key) {
                    reviewed[key] = value.clone();
                }
            }
            let mut parts = Vec::new();
            if let Some(text) = answer["notes"].as_str() {
                let single: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                parts.push(single.chars().take(600).collect::<String>());
            }
            if lowered > 0 {
                parts.push(format!("{lowered} goal status(es) the reviewer set to active or confirmed were lowered to candidate: only the owner confirms a goal."));
            }
            if !kept.is_empty() {
                parts.push(format!(
                    "The reviewer left no work items, so the generated intake work was kept ({}): without a work item a conversation cannot be bound to AWR.",
                    kept.join(", ")
                ));
            }
            if !parts.is_empty() {
                notes = Some(parts.join(" "));
            }
        }
        let reviewed_path = self.run_dir.join("reviewed.json");
        write_json(&reviewed_path, &reviewed)?;
        let draft_files = draft["generated_files"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let changed: Vec<String> = reviewed["generated_files"]
            .as_object()
            .map(|files| {
                files
                    .iter()
                    .filter(|(path, text)| draft_files.get(*path) != Some(text))
                    .map(|(path, _)| path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut result = json!({ "reviewed_path": reviewed_path, "by_reviewer": self.args.reviewer.is_some(), "changed_files": changed });
        if let Some(notes) = notes {
            result["notes"] = json!(notes);
        }
        Ok(result)
    }

    fn accept(&mut self) -> std::result::Result<Value, StepEnd> {
        let reviewed_path = self.run_dir.join("reviewed.json");
        let attempted = self.step("accept")["attempted"] == true;
        if self.root.join(".awr").join("project.toml").is_file() {
            if attempted {
                return Ok(
                    json!({ "recovered": true, "note": "The project was initialized by an earlier attempt of this run that did not finish recording it." }),
                );
            }
            return Err(Stop::new(
                "already_initialized",
                "The project was initialized while the run was in progress.",
            )
            .into());
        }
        // AWR takes the reviewer's word that an ambiguity is settled and refuses the draft while it is still listed.
        let reviewed = read_json(&reviewed_path)
            .ok_or_else(|| StepEnd::Failed("the reviewed draft of this run is missing".into()))?;
        let ambiguous: Vec<String> = reviewed["ambiguous_domains"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|d| d.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !ambiguous.is_empty() {
            return Err(ambiguous_authority(&ambiguous, None).into());
        }
        let reviewed_arg = reviewed_path.to_string_lossy().into_owned();
        let shown = match self.awr.run(&["init", "--from-draft", &reviewed_arg]) {
            Ok(shown) => shown,
            Err(failure) => {
                let stop = classify(&failure, &self.root);
                return Err(match stop.reason.as_str() {
                    "inventory_changed" => StepEnd::Retry(stop.message),
                    _ => StepEnd::Stop(stop),
                });
            }
        };
        let writes: Vec<Value> = shown["preview"]["writes"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|w| json!({ "action": w["action"], "path": w["path"] }))
                    .collect()
            })
            .unwrap_or_default();
        let outside = unexpected_writes(&writes);
        if !outside.is_empty() {
            return Err(Stop::new(
                "unexpected_write",
                "AWR would write outside .awr/ beyond its own ignore entries; nothing was written.",
            )
            .with(json!({ "paths": outside }))
            .into());
        }
        let Some(fingerprint) = shown["preview"]["fingerprint"].as_str().map(String::from) else {
            return Err(Stop::new(
                "cannot_apply",
                "AWR returned no preview fingerprint for the reviewed draft.",
            )
            .into());
        };
        self.mark(
            "accept",
            json!({ "attempted": true, "fingerprint": fingerprint }),
        )?;
        if let Err(failure) = self.awr.run(&[
            "init",
            "--from-draft",
            &reviewed_arg,
            "--accept",
            "--expected-preview",
            &fingerprint,
        ]) {
            let stop = classify(&failure, &self.root);
            if self.root.join(".awr").join("project.toml").is_file() {
                return Err(Stop::new(
                    "accept_failed_partial",
                    format!(
                        "Accepting failed part way and left an incomplete .awr/ in the project: {}",
                        stop.message
                    ),
                )
                .into());
            }
            return Err(StepEnd::Stop(stop));
        }
        let written_outside: Vec<&str> = writes
            .iter()
            .filter_map(|w| w["path"].as_str())
            .filter(|path| !inside_awr(path))
            .collect();
        Ok(
            json!({ "fingerprint": fingerprint, "writes": writes, "written_outside_awr": written_outside }),
        )
    }

    fn organize(&mut self) -> std::result::Result<Value, StepEnd> {
        let inspected = self
            .awr
            .run(&["intake", "inspect"])
            .map_err(|f| StepEnd::Stop(classify(&f, &self.root)))?;
        let organization = if inspected["organization"].is_object() {
            &inspected["organization"]
        } else {
            &inspected
        };
        let state = organization["state"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        if state == "source_unreadable" {
            return Err(Stop::new("source_unreadable", "AWR cannot read the sources of the initialized project.")
                .with(json!({ "gaps": organization["gaps"].as_array().map(|g| g.iter().take(5).cloned().collect::<Vec<_>>()) }))
                .into());
        }
        if state == "not_initialized" {
            return Err(Stop::new(
                "accept_incomplete",
                "The project is not initialized after accept.",
            )
            .into());
        }
        let _ = SETTLED_STATES;
        let sample = |key: &str| {
            organization[key]
                .as_array()
                .map(|list| list.iter().take(20).cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        Ok(json!({
            "state": state,
            "gap_total": organization["gap_total"].as_u64().unwrap_or(0),
            "goal_total": organization["goal_total"].as_u64().unwrap_or(0),
            "executable_work_total": organization["executable_work_total"].as_u64().unwrap_or(0),
            "goals": sample("goals").iter().map(|goal| json!({
                "key": goal["key"],
                "status": goal.get("status").or_else(|| goal.get("declared_status")).cloned().unwrap_or(Value::Null),
                "title": goal["title"],
            })).collect::<Vec<_>>(),
            "actions": organization["actions"].as_array().map(|a| a.iter().map(|x| x["id"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "gaps": sample("gaps").iter().map(|gap| json!({
                "code": gap["code"],
                "action": gap["action"],
                "detail": gap["detail"].as_str().unwrap_or_default().chars().take(240).collect::<String>(),
                "target": gap["target"],
            })).collect::<Vec<_>>(),
            "truncated": organization["truncated"].as_bool().unwrap_or(false),
        }))
    }

    fn receipt(&mut self) -> std::result::Result<Value, StepEnd> {
        let head_after = git_head(&self.root);
        let organized = self.step("organize")["result"].clone();
        let accepted = self.step("accept")["result"].clone();
        let preview = self.step("preview")["result"].clone();
        let review = self.step("review")["result"].clone();
        let head_before = self.step("preflight")["result"]["git_head"].clone();
        let awaiting: Vec<Value> = organized["goals"]
            .as_array()
            .map(|goals| {
                goals
                    .iter()
                    .filter(|goal| {
                        let status = goal["status"]
                            .as_str()
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                        status != "active" && status != "confirmed"
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let state = organized["state"].as_str().unwrap_or("unknown");
        let mut reviewed = json!({
            "by_reviewer": review["by_reviewer"].as_bool().unwrap_or(false),
            "changed_files": review["changed_files"].clone(),
            "rounds": self.record["rounds"].clone(),
        });
        if let Some(notes) = review.get("notes") {
            reviewed["notes"] = notes.clone();
        }
        let body = json!({
            "version": RECORD_VERSION,
            "run": self.record["id"],
            "project": self.root,
            "started_at": self.record["started_at"],
            "finished_at": now_ms(),
            "result": state,
            "mapped": { "sources": preview["sources"].clone(), "inventory": preview["inventory"].clone() },
            "created": {
                "writes": accepted["writes"].clone(),
                "written_outside_awr": accepted["written_outside_awr"].clone(),
                "recovered": accepted["recovered"].as_bool().unwrap_or(false),
            },
            "reviewed": reviewed,
            "organization": {
                "state": organized["state"], "gap_total": organized["gap_total"], "goal_total": organized["goal_total"],
                "executable_work_total": organized["executable_work_total"], "actions": organized["actions"], "gaps": organized["gaps"],
            },
            "awaiting_confirmation": awaiting,
            "git": { "head_before": head_before, "head_after": head_after, "committed": false, "pushed": false, "head_moved": head_before != json!(head_after) },
        });
        let directory = self.root.join(".awr").join("takeover");
        fs::create_dir_all(&directory)?;
        let in_project = directory.join("receipt.json");
        write_json(&in_project, &body)?;
        write_json(&self.run_dir.join("receipt.json"), &body)?;
        Ok(json!({ "receipt_path": in_project }))
    }
}

fn summarize_sources(mapping: &Value) -> Vec<Value> {
    mapping["sources"]
        .as_array()
        .map(|sources| {
            sources
                .iter()
                .map(|s| json!({ "path": s["path"], "domain": s["domain"], "adapter": s["adapter"], "role": s["role"] }))
                .collect()
        })
        .unwrap_or_default()
}

fn ambiguous_authority(domains: &[String], message: Option<String>) -> Stop {
    Stop::new(
        "ambiguous_authority",
        message.unwrap_or_else(|| {
            format!(
                "AWR found several candidates for {} and the reviewed draft did not choose. Add the authoritative source of each ambiguous domain to authority_mapping.sources (role: primary) and clear ambiguous_domains.",
                if domains.is_empty() { "a domain".to_string() } else { domains.join(", ") }
            )
        }),
    )
    .with(json!({ "domains": domains }))
}

/// Run the reviewer: the request as JSON on stdin, its answer as JSON on stdout, within `timeout`.
fn run_reviewer(
    program: &Path,
    args: &[String],
    request: &Value,
    timeout: Duration,
) -> std::result::Result<Value, Stop> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            Stop::new(
                "review_failed",
                format!("The reviewer could not be started: {error}"),
            )
        })?;
    let payload = serde_json::to_vec(request).unwrap_or_default();
    let mut stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin.write_all(&payload);
        }
        drop(stdin);
    });
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(stream) = stdout.as_mut() {
            let _ = stream
                .take(MAX_REVIEWER_OUTPUT_BYTES as u64 + 1)
                .read_to_end(&mut buffer);
        }
        buffer
    });
    let err = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(stream) = stderr.as_mut() {
            let _ = stream.take(8192).read_to_end(&mut buffer);
        }
        buffer
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Stop::new(
                    "review_failed",
                    format!("The reviewer took more than {} seconds.", timeout.as_secs()),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                return Err(Stop::new(
                    "review_failed",
                    format!("The reviewer could not be waited for: {error}"),
                ));
            }
        }
    };
    let _ = writer.join();
    let output = out.join().unwrap_or_default();
    let diagnostics = err.join().unwrap_or_default();
    if !status.success() {
        let text = awr_core::safe_diagnostic(&String::from_utf8_lossy(&diagnostics));
        let shown: String = text.chars().take(300).collect();
        return Err(Stop::new(
            "review_failed",
            format!(
                "The reviewer exited with {status}{}",
                if shown.is_empty() {
                    String::new()
                } else {
                    format!(": {shown}")
                }
            ),
        ));
    }
    if output.len() > MAX_REVIEWER_OUTPUT_BYTES {
        return Err(Stop::new(
            "review_rejected",
            "The reviewer's answer is larger than 8 MiB.",
        ));
    }
    extract_json(&String::from_utf8_lossy(&output)).ok_or_else(|| {
        Stop::new(
            "review_failed",
            "The reviewer did not return a JSON object with the reviewed intake files.",
        )
    })
}

// --- the run -------------------------------------------------------------------------------------------------

fn emit(json_output: bool, step: &str, status: &str) {
    if !json_output {
        eprintln!("takeover: {step} {status}");
    }
}

fn execute(run: &mut Run, name: &str) -> std::result::Result<Value, StepEnd> {
    match name {
        "preflight" => run.preflight(),
        "preview" => run.preview(),
        "review" => run.review(),
        "accept" => run.accept(),
        "organize" => run.organize(),
        "receipt" => run.receipt(),
        _ => Err(StepEnd::Failed(format!("unknown step {name}"))),
    }
}

/// Run, or resume, the takeover of one project. Returns the outcome object printed by the command.
fn run_takeover(root: &Path, args: &TakeoverArgs, json_output: bool) -> Result<Value> {
    let exe = std::env::current_exe()?;
    let base = state_base(args);
    let run_dir = run_dir_for(root, &base);
    fs::create_dir_all(&run_dir)?;
    let _lock = match acquire_lock(&run_dir)? {
        Locked::Held(lock) => lock,
        Locked::Busy(pid) => {
            return Ok(json!({
                "outcome": "stopped",
                "run_dir": run_dir,
                "record": { "status": "stopped", "project_root": root, "stop": { "step": "preflight", "reason": "locked", "message": format!("Another process ({pid}) is taking this project over.") } },
            }));
        }
    };
    let record_path = run_dir.join("takeover.json");
    let mut record = read_json(&record_path)
        .filter(|r| r["version"] == RECORD_VERSION && r["project_root"] == json!(root));
    if let Some(done) = record
        .as_ref()
        .filter(|r| r["status"] == "completed" && !args.rerun)
    {
        let receipt = read_json(&run_dir.join("receipt.json"));
        return Ok(
            json!({ "outcome": "completed", "record": done, "receipt": receipt, "run_dir": run_dir }),
        );
    }
    if record.as_ref().is_none_or(|r| r["status"] == "completed") {
        let digest = format!(
            "{:x}",
            Sha256::digest(
                format!("{}{}{}", now_ms(), std::process::id(), root.display()).as_bytes()
            )
        );
        record = Some(json!({
            "version": RECORD_VERSION, "id": &digest[..12], "project_root": root, "started_at": now_ms(),
            "updated_at": now_ms(), "status": "running", "rounds": 1, "steps": {}, "stop": null,
        }));
    }
    let mut record = record.expect("a record exists");
    record["status"] = json!("running");
    record["stop"] = Value::Null;
    let mut run = Run {
        awr: Awr {
            exe,
            root: root.to_path_buf(),
        },
        root: root.to_path_buf(),
        run_dir: run_dir.clone(),
        record,
        args,
    };
    // A saved preview belongs to the manifest it was made with; another one starts again from the preview, unless
    // accept already began (then the project is initialized and the run goes on from there).
    if let Some(wanted) = &args.manifest
        && run.step("preview")["status"] == "done"
        && run.step("preview")["result"]["manifest"] != json!(wanted.to_string_lossy())
        && run.step("accept")["attempted"] != true
        && let Some(steps) = run.record["steps"].as_object_mut()
    {
        for step in ["preview", "review", "accept"] {
            steps.remove(step);
        }
    }
    run.save()?;

    let mut receipt_body = None;
    let mut index = 0;
    while index < STEPS.len() {
        let name = STEPS[index];
        if run.step(name)["status"] == "done" {
            index += 1;
            continue;
        }
        let adopted = run.step("preflight")["result"]["adopted"] == true;
        if name != "preflight" && adopted && ["preview", "review", "accept"].contains(&name) {
            run.record["steps"][name] = json!({ "status": "skipped", "finished_at": now_ms() });
            run.save()?;
            index += 1;
            continue;
        }
        let existing = run.record["steps"][name].clone();
        run.mark(name, json!({ "status": "running", "started_at": now_ms(), "error": Value::Null, "attempted": existing["attempted"] }))?;
        emit(json_output, name, "running");
        match execute(&mut run, name) {
            Ok(result) => {
                let stored = if name == "receipt" {
                    json!({ "receipt_path": result["receipt_path"] })
                } else {
                    result.clone()
                };
                let mut fields =
                    json!({ "status": "done", "finished_at": now_ms(), "result": stored });
                if name == "accept" && result["recovered"] == true {
                    fields["status"] = json!("done");
                }
                run.mark(name, fields)?;
                emit(json_output, name, "done");
                if name == "receipt" {
                    receipt_body = read_json(&run_dir.join("receipt.json"));
                }
                index += 1;
            }
            Err(StepEnd::Retry(message)) => {
                let rounds = run.record["rounds"].as_u64().unwrap_or(1);
                if rounds >= u64::from(args.max_rounds) {
                    run.mark(
                        "accept",
                        json!({ "status": "stopped", "finished_at": now_ms() }),
                    )?;
                    run.record["status"] = json!("stopped");
                    run.record["stop"] = json!({ "step": "accept", "reason": "inventory_changed", "message": format!("The project kept changing while it was being reviewed ({rounds} rounds): {message}") });
                    run.save()?;
                    emit(json_output, "accept", "stopped");
                    return Ok(
                        json!({ "outcome": "stopped", "record": run.record, "run_dir": run_dir }),
                    );
                }
                run.record["rounds"] = json!(rounds + 1);
                for earlier in ["preview", "review", "accept"] {
                    if let Some(steps) = run.record["steps"].as_object_mut() {
                        steps.remove(earlier);
                    }
                }
                run.save()?;
                emit(json_output, "accept", "retry");
                index = STEPS.iter().position(|s| *s == "preview").unwrap_or(1);
            }
            Err(end) => {
                let (status, stop) = match end {
                    StepEnd::Stop(stop) => ("stopped", stop),
                    StepEnd::Failed(message) => ("failed", Stop::new("unexpected_error", message)),
                    StepEnd::Retry(_) => unreachable!("handled above"),
                };
                run.mark(name, json!({ "status": status, "finished_at": now_ms() }))?;
                // The reviewed draft was what accept refused, and nothing was written yet: the next run reviews again,
                // so a reviewer that settles the ambiguity or avoids the extra write can take effect.
                if name == "accept"
                    && REVIEW_REJECTED_AT_ACCEPT.contains(&stop.reason.as_str())
                    && let Some(steps) = run.record["steps"].as_object_mut()
                {
                    steps.remove("review");
                    steps.remove("accept");
                }
                run.record["status"] = json!(status);
                let mut stop_record =
                    json!({ "step": name, "reason": stop.reason, "message": stop.message });
                if let Some(detail) = stop.detail {
                    stop_record["detail"] = detail;
                }
                run.record["stop"] = stop_record;
                run.save()?;
                emit(json_output, name, status);
                return Ok(json!({ "outcome": status, "record": run.record, "run_dir": run_dir }));
            }
        }
    }
    run.record["status"] = json!("completed");
    run.save()?;
    Ok(
        json!({ "outcome": "completed", "record": run.record, "receipt": receipt_body, "run_dir": run_dir }),
    )
}

fn print_outcome(outcome: &Value, json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(outcome).expect("outcome serializes")
        );
        return;
    }
    match outcome["outcome"].as_str().unwrap_or("unknown") {
        "completed" => {
            let receipt = &outcome["receipt"];
            println!(
                "Takeover completed: {} (run {})",
                receipt["result"].as_str().unwrap_or("done"),
                outcome["record"]["id"].as_str().unwrap_or("?")
            );
            println!(
                "  mapped {} source(s) from {} inventory file(s)",
                receipt["mapped"]["sources"].as_array().map_or(0, Vec::len),
                receipt["mapped"]["inventory"]
            );
            if let Some(goals) = receipt["awaiting_confirmation"]
                .as_array()
                .filter(|g| !g.is_empty())
            {
                let keys: Vec<&str> = goals.iter().filter_map(|g| g["key"].as_str()).collect();
                println!("  goals awaiting your confirmation: {}", keys.join(", "));
            }
            if let Some(notes) = receipt["reviewed"]["notes"].as_str() {
                println!("  review: {notes}");
            }
            if let Some(path) =
                outcome["record"]["steps"]["receipt"]["result"]["receipt_path"].as_str()
            {
                println!("  receipt: {path}");
            }
        }
        other => {
            let stop = &outcome["record"]["stop"];
            println!(
                "Takeover {other} at {}: {}: {}",
                stop["step"].as_str().unwrap_or("?"),
                stop["reason"].as_str().unwrap_or("?"),
                stop["message"].as_str().unwrap_or("")
            );
            if let (Some(path), Some(line)) = (
                stop["detail"]["path"].as_str(),
                stop["detail"]["line"].as_u64(),
            ) {
                println!("  at {path}:{line}");
            }
            println!("  Run the command again to resume; finished steps are not repeated.");
        }
    }
}

fn status(root: &Path, args: &TakeoverArgs, json_output: bool) -> Result<()> {
    let run_dir = run_dir_for(root, &state_base(args));
    let record = read_json(&run_dir.join("takeover.json")).ok_or_else(|| {
        Error::NotFound(format!("no takeover run recorded for {}", root.display()))
    })?;
    let receipt = read_json(&run_dir.join("receipt.json"));
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({ "record": record, "receipt": receipt, "run_dir": run_dir })
            )
            .expect("status serializes")
        );
        return Ok(());
    }
    println!(
        "Takeover of {}: {} (run {})",
        root.display(),
        record["status"].as_str().unwrap_or("?"),
        record["id"].as_str().unwrap_or("?")
    );
    for name in STEPS {
        println!(
            "  {name}: {}",
            record["steps"][name]["status"]
                .as_str()
                .unwrap_or("pending")
        );
    }
    if let Some(stop) = record["stop"].as_object() {
        println!(
            "  stopped at {}: {}: {}",
            stop["step"].as_str().unwrap_or("?"),
            stop["reason"].as_str().unwrap_or("?"),
            stop["message"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

pub fn run(project: &Path, args: &TakeoverArgs, json_output: bool) -> Result<()> {
    let root = fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    if let Some(TakeoverCommand::Status) = args.command {
        return status(&root, args, json_output);
    }
    let outcome = run_takeover(&root, args, json_output)?;
    print_outcome(&outcome, json_output);
    match outcome["outcome"].as_str() {
        Some("completed") => Ok(()),
        _ => {
            let stop = &outcome["record"]["stop"];
            Err(Error::WithDetails {
                error: Box::new(Error::InvalidTransition(format!(
                    "takeover {} at {}: {}",
                    outcome["outcome"].as_str().unwrap_or("stopped"),
                    stop["step"].as_str().unwrap_or("?"),
                    stop["reason"].as_str().unwrap_or("?")
                ))),
                details: json!({ "takeover": { "step": stop["step"], "reason": stop["reason"], "run_dir": outcome["run_dir"] } }),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect()
    }

    const INTAKE: &str = "goals:\n- id: intake-goal\n  status: draft\n  title: Project goal\nwork_items:\n- id: INTAKE-001\n  kind: intake\n  status: ready\n  title: Verify project goals\n";
    const GOALS: &str = "goals:\n- id: g1\n  status: candidate\n  title: A real goal\n";
    const PATH: &str = ".awr/intake/work-ledger.yaml";

    #[test]
    fn work_items_are_found_in_block_and_flow_form() {
        assert!(has_work_items(INTAKE));
        assert!(has_work_items("goals: []\nwork_items: [{id: W-1}]\n"));
        assert!(!has_work_items(&format!("{GOALS}work_items: []\n")));
        assert!(!has_work_items(&format!("{GOALS}work_items: [ ]\n")));
        assert!(!has_work_items(&format!("{GOALS}work_items:\n")));
        assert!(!has_work_items(GOALS));
        assert!(!has_work_items("# work_items: not a key\n"));
    }

    #[test]
    fn a_ledger_without_work_gets_the_draft_intake_work_back_exactly_once() {
        let draft = map(&[(PATH, INTAKE)]);
        for empty in [
            format!("{GOALS}work_items: []\n"),
            format!("{GOALS}work_items:\n"),
            format!("{GOALS}work_items: [ ]\n# none yet\n"),
            GOALS.to_string(),
        ] {
            let mut files = map(&[(PATH, &empty)]);
            assert_eq!(
                keep_intake_work(&mut files, &draft),
                vec![PATH.to_string()],
                "{empty:?}"
            );
            let text = files[PATH].as_str().unwrap();
            assert!(
                text.starts_with("goals:\n- id: g1\n"),
                "the reviewer's goals stay"
            );
            assert!(text.ends_with("work_items:\n- id: INTAKE-001\n  kind: intake\n  status: ready\n  title: Verify project goals\n"));
            assert_eq!(text.matches("work_items:").count(), 1);
        }
    }

    #[test]
    fn work_the_reviewer_wrote_is_never_replaced() {
        let draft = map(&[(PATH, INTAKE)]);
        let real = format!("{GOALS}work_items:\n- id: W-1\n  status: planned\n");
        let flow = format!("{GOALS}work_items: [{{id: W-2}}]\n");
        for text in [real, flow] {
            let mut files = map(&[(PATH, &text)]);
            assert!(keep_intake_work(&mut files, &draft).is_empty());
            assert_eq!(files[PATH], Value::String(text));
        }
        // a draft without work, a file the draft lacks, Markdown, and content after the empty key
        let mut files = map(&[(PATH, &format!("{GOALS}work_items: []\n"))]);
        assert!(keep_intake_work(&mut files, &map(&[(PATH, GOALS)])).is_empty());
        assert!(keep_intake_work(&mut files, &Map::new()).is_empty());
        let mut markdown = map(&[(".awr/intake/GOALS.md", "# G\n")]);
        assert!(
            keep_intake_work(&mut markdown, &map(&[(".awr/intake/GOALS.md", INTAKE)])).is_empty()
        );
        let trailing = format!("{GOALS}work_items: []\nextras:\n  a: 1\n");
        let mut files = map(&[(PATH, &trailing)]);
        assert!(keep_intake_work(&mut files, &map(&[(PATH, INTAKE)])).is_empty());
        assert_eq!(files[PATH], Value::String(trailing));
    }

    #[test]
    fn goals_are_capped_at_candidate_and_work_is_left_alone() {
        let yaml = "goals:\n- id: g\n  status: active\n  title: T\n- id: h\n  status: \"Confirmed\"\n- id: k\n  status: needs_confirmation\nwork_items:\n- id: W\n  status: active\n";
        let markdown = "# Goal {#g status=active}\n\n# Other {#o owner=x status=Confirmed}\n\n# Fine {#f status=candidate}\n\nstatus=active outside braces\n";
        let mut files = map(&[
            (PATH, yaml),
            (".awr/intake/GOALS.md", markdown),
            (".awr/intake/note.txt", "status: active"),
        ]);
        assert_eq!(cap_goal_statuses(&mut files), 4);
        let yaml_out = files[PATH].as_str().unwrap();
        assert!(yaml_out.contains("- id: g\n  status: candidate\n"));
        assert!(yaml_out.contains("status: \"candidate\"\n"));
        assert!(yaml_out.contains("status: needs_confirmation"));
        assert!(
            yaml_out.ends_with("work_items:\n- id: W\n  status: active\n"),
            "work is not touched"
        );
        let md = files[".awr/intake/GOALS.md"].as_str().unwrap();
        assert!(
            md.contains("{#g status=candidate}") && md.contains("{#o owner=x status=candidate}")
        );
        assert!(md.contains("status=active outside braces"));
        assert_eq!(
            files[".awr/intake/note.txt"],
            Value::String("status: active".into())
        );
        assert_eq!(cap_goal_statuses(&mut files), 0);
    }

    #[test]
    fn generated_files_stay_inside_the_intake_directory() {
        assert!(validate_files(&map(&[(".awr/intake/a.md", "x")])).is_none());
        for path in [
            "README.md",
            ".awr/other.md",
            "/etc/passwd",
            ".awr/intake/../../x.md",
        ] {
            assert!(
                validate_files(&map(&[(path, "x")]))
                    .is_some_and(|p| p.contains("outside .awr/intake/")),
                "{path}"
            );
        }
        assert!(validate_files(&Map::new()).is_some());
        let mut not_text = Map::new();
        not_text.insert(".awr/intake/a.md".into(), json!(5));
        assert!(validate_files(&not_text).is_some_and(|p| p.contains("must be text")));
        assert!(
            validate_files(&map(&[(
                ".awr/intake/a.md",
                &"x".repeat(MAX_GENERATED_FILE_BYTES + 1)
            )]))
            .is_some_and(|p| p.contains("larger than 1 MiB"))
        );
    }

    #[test]
    fn the_reviewers_answer_is_read_from_plain_json_a_fence_or_braces() {
        let object = json!({ "generated_files": { ".awr/intake/a.md": "x" }, "notes": "n" });
        let text = object.to_string();
        assert_eq!(extract_json(&text), Some(object.clone()));
        assert_eq!(
            extract_json(&format!("I read it.\n```json\n{text}\n```\nbye")),
            Some(object.clone())
        );
        assert_eq!(
            extract_json(&format!(
                "```json\n{{\"a\":1}}\n```\nthen\n```json\n{text}\n```"
            )),
            Some(object.clone())
        );
        assert_eq!(extract_json(&format!("prefix {text} suffix")), Some(object));
        assert_eq!(extract_json("no json here"), None);
    }

    #[test]
    fn locators_become_paths_inside_the_project_and_the_state_directory_is_stable() {
        let root = Path::new("/tmp/some project");
        assert_eq!(
            relative_locator("file:///tmp/some%20project/docs/README.md#L4", root).as_deref(),
            Some("docs/README.md")
        );
        assert_eq!(
            relative_locator("/elsewhere/x.md", root).as_deref(),
            Some("/elsewhere/x.md")
        );
        // Windows spellings: a file URL `/C:/...`, a `\\?\` verbatim root and a root with backslashes.
        for root in [
            "C:\\Users\\r\\Temp\\p",
            "\\\\?\\C:\\Users\\r\\Temp\\p",
            "C:/Users/r/Temp/p",
        ] {
            assert_eq!(
                relative_locator(
                    "file:///C:/Users/r/Temp/p/docs/README.md#L4",
                    Path::new(root)
                )
                .as_deref(),
                Some("docs/README.md"),
                "{root}"
            );
        }
        // a sibling directory that only shares the prefix is not inside the project
        assert_eq!(
            relative_locator(
                "file:///C:/Users/r/Temp/p2/README.md",
                Path::new("C:\\Users\\r\\Temp\\p")
            )
            .as_deref(),
            Some("C:/Users/r/Temp/p2/README.md")
        );
        // A literal `%` before a multibyte character, or too close to the end to be an escape, is kept as it is.
        for (locator, expected) in [
            ("file:///tmp/some%20project/100%é.md", "100%é.md"),
            // the byte range after `%` ends inside `é`: slicing the `&str` there panics
            ("file:///tmp/some%20project/x%aé.md", "x%aé.md"),
            ("file:///tmp/some%20project/50%", "50%"),
            ("file:///tmp/some%20project/a%2", "a%2"),
        ] {
            assert_eq!(relative_locator(locator, root).as_deref(), Some(expected));
        }
        let base = Path::new("/state");
        let one = run_dir_for(Path::new("/p/My Project"), base);
        assert_eq!(one, run_dir_for(Path::new("/p/My Project"), base));
        assert_ne!(one, run_dir_for(Path::new("/q/My Project"), base));
        assert!(
            one.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("My_Project-")
        );
    }

    #[test]
    fn a_preview_that_writes_outside_awr_beyond_the_ignore_entries_is_caught() {
        let writes = |paths: &[&str]| -> Vec<Value> {
            paths
                .iter()
                .map(|p| json!({ "action": "create", "path": p }))
                .collect()
        };
        assert!(
            unexpected_writes(&writes(&[
                ".awr/project.toml",
                ".awr/intake/work-ledger.yaml",
                ".gitignore"
            ]))
            .is_empty()
        );
        assert_eq!(
            unexpected_writes(&writes(&[".awr/project.toml", "README.md", "src/main.rs"])),
            vec!["README.md", "src/main.rs"]
        );
        assert_eq!(
            unexpected_writes(&writes(&[".awrx/a", ".awr-backup", "docs/.gitignore"])),
            vec![".awrx/a", ".awr-backup", "docs/.gitignore"]
        );
        assert_eq!(
            unexpected_writes(&writes(&[".awr/../README.md", "/etc/hosts"])),
            vec![".awr/../README.md", "/etc/hosts"]
        );
    }

    #[test]
    fn a_lock_blocks_only_while_its_process_runs_and_it_is_fresh() {
        let now = 10 * STALE_LOCK_MS;
        let lock = |pid: u64, at: u64| json!({ "pid": pid, "at": at });
        let other = u64::from(std::process::id()) + 1;
        assert_eq!(
            lock_is_live(&lock(other, now - 1000), now, |_| true),
            Some(other)
        );
        assert_eq!(
            lock_is_live(&lock(other, now - 1000), now, |_| false),
            None,
            "its process is gone"
        );
        assert_eq!(
            lock_is_live(&lock(other, now - STALE_LOCK_MS), now, |_| true),
            None,
            "an old lock expires"
        );
        assert_eq!(
            lock_is_live(&lock(u64::from(std::process::id()), now), now, |_| true),
            None,
            "our own lock"
        );
        assert_eq!(lock_is_live(&json!({}), now, |_| true), None);
        assert_eq!(
            lock_is_live(&json!({ "pid": other }), now, |_| true),
            None,
            "no timestamp counts as old"
        );
    }

    #[test]
    fn awr_errors_become_recorded_stops_that_name_where_and_never_what() {
        let root = Path::new("/tmp/p");
        let secret = Failure {
            code: "RuleViolation".into(),
            message: "rule violation: sensitive content is not accepted; category=credential"
                .into(),
            details: json!({ "category": "credential", "location": { "locator": "file:///tmp/p/README.md", "line": 12 }, "next_action": "Keep this file outside registered sources." }),
        };
        let stop = classify(&secret, root);
        assert_eq!(stop.reason, "secret_boundary");
        assert_eq!(stop.detail.as_ref().unwrap()["path"], "README.md");
        assert_eq!(stop.detail.as_ref().unwrap()["line"], 12);
        assert!(
            stop.message.contains("credential")
                && stop.message.contains("outside registered sources")
        );
        let ambiguous = Failure {
            code: "SourceConflict".into(),
            message: "multiple authority candidates for ledger".into(),
            details: Value::Null,
        };
        assert_eq!(classify(&ambiguous, root).reason, "ambiguous_authority");
        let changed = Failure {
            code: "SourceConflict".into(),
            message: "inventory fingerprint changed".into(),
            details: Value::Null,
        };
        assert_eq!(classify(&changed, root).reason, "inventory_changed");
        let other = Failure {
            code: "Storage".into(),
            message: "disk".into(),
            details: Value::Null,
        };
        assert_eq!(classify(&other, root).reason, "awr_error");
    }
}
