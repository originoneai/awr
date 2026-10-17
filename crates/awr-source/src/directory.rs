use crate::{
    Locator, Manifest, ParseContext, SourceAdapter, SourceSnapshot, SourceSpec, locator::git,
    markdown_sections,
};
use awr_core::{Decision, DecisionStatus, EntityKind, Error, ProjectionBatch, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryOptions {
    #[serde(default = "yes")]
    recursive: bool,
}
fn yes() -> bool {
    true
}
fn options(spec: &SourceSpec) -> Result<DirectoryOptions> {
    if spec.domain != "decisions" {
        return Err(Error::InvalidInput(
            "directory adapter requires decisions domain".into(),
        ));
    }
    spec.options
        .clone()
        .try_into()
        .map_err(|e| Error::InvalidInput(format!("directory options: {e}")))
}
fn markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("md") || v.eq_ignore_ascii_case("markdown"))
}
fn visible(path: &Path) -> bool {
    path.components()
        .all(|part| !part.as_os_str().to_string_lossy().starts_with('.'))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectoryInventory {
    pub files: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectoryDelta {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub removed: Vec<String>,
}
impl DirectoryInventory {
    pub fn diff(&self, previous: &Self) -> DirectoryDelta {
        DirectoryDelta {
            added: self
                .files
                .keys()
                .filter(|key| !previous.files.contains_key(*key))
                .cloned()
                .collect(),
            modified: self
                .files
                .iter()
                .filter(|(key, value)| previous.files.get(*key).is_some_and(|old| old != *value))
                .map(|(key, _)| key.clone())
                .collect(),
            removed: previous
                .files
                .keys()
                .filter(|key| !self.files.contains_key(*key))
                .cloned()
                .collect(),
        }
    }
}

pub struct MarkdownDirectoryAdapter;
impl MarkdownDirectoryAdapter {
    pub fn source_identity(
        &self,
        root: &Path,
        manifest: &Manifest,
        spec: &SourceSpec,
        child: &Locator,
    ) -> Result<String> {
        match (Locator::from_spec(root, manifest, spec)?, child) {
            (Locator::Git { revision, .. }, Locator::Git { path, .. }) => Locator::Git {
                revision,
                path: path.clone(),
            }
            .identity(),
            _ => child.identity(),
        }
    }
    pub fn scan(
        &self,
        root: &Path,
        manifest: &Manifest,
        spec: &SourceSpec,
        cap: u64,
    ) -> Result<DirectoryInventory> {
        let cap = cap.min(crate::MARKDOWN_READ_CAP);
        let mut inventory = DirectoryInventory::default();
        for locator in self.discover(root, manifest, spec)? {
            inventory.files.insert(
                self.source_identity(root, manifest, spec, &locator)?,
                locator.read(root, cap)?.fingerprint,
            );
        }
        Ok(inventory)
    }
}
impl SourceAdapter for MarkdownDirectoryAdapter {
    fn name(&self) -> &'static str {
        "markdown-directory-v1"
    }
    fn discover(
        &self,
        root: &Path,
        manifest: &Manifest,
        spec: &SourceSpec,
    ) -> Result<Vec<Locator>> {
        let options = options(spec)?;
        let mut files = match Locator::from_spec(root, manifest, spec)? {
            Locator::File(directory) => {
                if !directory.is_dir() {
                    return Err(Error::InvalidInput(
                        "decision source must be a directory".into(),
                    ));
                }
                let mut pending = vec![directory];
                let mut files = vec![];
                while let Some(directory) = pending.pop() {
                    for entry in crate::open_dir_exact(&directory)?
                        .entries()
                        .map_err(|e| Error::SourceUnavailable(e.to_string()))?
                    {
                        let entry = entry.map_err(|e| Error::SourceUnavailable(e.to_string()))?;
                        if entry.file_name().to_string_lossy().starts_with('.') {
                            continue;
                        }
                        let kind = entry
                            .file_type()
                            .map_err(|e| Error::SourceUnavailable(e.to_string()))?;
                        // Symlink children are not implicit new authority roots.
                        if kind.is_dir() && options.recursive {
                            pending.push(directory.join(entry.file_name()));
                        } else if kind.is_file() && markdown_path(Path::new(&entry.file_name())) {
                            files.push(Locator::File(directory.join(entry.file_name())));
                        }
                        if files.len() + pending.len() > 4096 {
                            return Err(Error::BudgetExceeded {
                                required: files.len() + pending.len(),
                                budget: 4096,
                            });
                        }
                    }
                }
                files
            }
            Locator::Git { revision, path } => {
                git_files(root, &revision, &path, options.recursive)?
            }
        };
        files.sort_by_key(|locator| match locator {
            Locator::File(path) => path.to_string_lossy().into_owned(),
            Locator::Git { path, .. } => path.to_string_lossy().into_owned(),
        });
        Ok(files)
    }
    fn parse(
        &self,
        snapshot: &SourceSnapshot,
        context: &ParseContext<'_>,
        spec: &SourceSpec,
    ) -> Result<ProjectionBatch> {
        options(spec)?;
        let text = snapshot.text()?;
        let sections = markdown_sections(snapshot)?;
        let header = sections.iter().find(|section| section.level > 0);
        let mut metadata = frontmatter(text)?;
        let mut warnings = vec![];
        let header_fields = header
            .map(|h| header_fields(&h.body))
            .transpose()?
            .unwrap_or_default();
        for (key, raw) in header_fields {
            let value = if ["affected_keys", "paths"].contains(&key.as_str()) {
                Value::Array(
                    raw.split([',', '，', ';', '；'])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| Value::String(s.into()))
                        .collect(),
                )
            } else {
                Value::String(raw)
            };
            insert_metadata(&mut metadata, &key, value)?;
        }
        let raw_status = metadata_string(&metadata, "status")?.unwrap_or_default();
        let mut status = decision_status(&raw_status);
        let adoption: Option<awr_core::DecisionAdoption> = if metadata["adoption"].is_null() {
            None
        } else {
            Some(serde_json::from_value(metadata["adoption"].clone())?)
        };
        let superseded_by: Option<awr_core::DocumentVersion> =
            if metadata["superseded_by"].is_null() {
                None
            } else {
                Some(serde_json::from_value(metadata["superseded_by"].clone())?)
            };
        let adoption_valid = adoption
            .as_ref()
            .map(|a| -> Result<bool> {
                a.actor.validate()?;
                Ok(a.version == 1
                    && a.candidate.external_key == metadata["id"].as_str().unwrap_or("")
                    && a.candidate.source_id == context.source.id
                    && a.content_fingerprint
                        == crate::adoption::decision_content_fingerprint(text)?)
            })
            .transpose()?;
        if adoption_valid == Some(false) {
            if status == DecisionStatus::Accepted {
                status = DecisionStatus::Unknown;
            }
            warnings.push("adoption content or object identity changed; explicit review of this version is required".into());
        }
        if status == DecisionStatus::Unknown {
            warnings.push(format!("decision status unresolved: {raw_status:?}"));
        }
        let title = metadata_string(&metadata, "title")?
            .or_else(|| header.map(|h| h.title.clone()))
            .unwrap_or_else(|| context.source.locator.clone());
        let body = |names: &[&str]| {
            let mut chosen = vec![];
            let mut index = 0;
            while let Some(section) = sections.get(index) {
                index += 1;
                if !names.contains(&section.title.trim().to_lowercase().as_str()) {
                    continue;
                }
                chosen.push(section.body.clone());
                while let Some(child) = sections
                    .get(index)
                    .filter(|child| child.level > section.level)
                {
                    chosen.push(format!("{}\n\n{}", child.title, child.body));
                    index += 1;
                }
            }
            chosen.join("\n\n")
        };
        let explicit_decision = metadata_string(&metadata, "decision")?
            .unwrap_or_else(|| body(&["decision", "decisions", "决策", "决定"]));
        let decision = if explicit_decision.trim().is_empty() {
            header
                .map(|section| without_header_fields(&section.body))
                .unwrap_or_default()
        } else {
            explicit_decision
        };
        if decision.trim().is_empty() {
            warnings.push("no decision statement identified; consult the source reference".into());
        }
        let rationale = metadata_string(&metadata, "rationale")?.unwrap_or_else(|| {
            body(&["rationale", "context", "理由", "背景", "理由与影响", "原因"])
        });
        let key =
            metadata_string(&metadata, "id")?.unwrap_or_else(|| context.source.locator.clone());
        let lines = text.lines().count();
        if lines == 0 {
            return Err(Error::InvalidInput("empty decision document".into()));
        }
        let mut meta = context.meta(
            EntityKind::Decision,
            &key,
            snapshot,
            Some("document".into()),
            Some((1, lines)),
        )?;
        meta.source_ref.section_fingerprint = Some(snapshot.section_fingerprint(1, lines)?);
        let decision = Decision {
            specification: if let Some(value) = metadata.get("specification") {
                let specification: awr_core::Specification = serde_json::from_value(value.clone())
                    .map_err(|_| Error::InvalidInput("/specification must contain version and items with explicit id/disposition and optional superseded_by".into()))?;
                specification.validate()?;
                Some(specification)
            } else {
                None
            },
            meta,
            title,
            status,
            raw_status,
            adoption,
            adoption_valid,
            superseded_by,
            decision,
            rationale,
            affected_keys: metadata_strings(&metadata, "affected_keys")?,
            paths: metadata_strings(&metadata, "paths")?,
        };
        Ok(ProjectionBatch {
            decisions: vec![decision],
            warnings,
            ..Default::default()
        })
    }
}

fn git_files(root: &Path, revision: &str, path: &Path, recursive: bool) -> Result<Vec<Locator>> {
    let commit = git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )?;
    let commit = std::str::from_utf8(&commit)
        .map_err(|e| Error::InvalidInput(e.to_string()))?
        .trim();
    let prefix = git(root, &["rev-parse", "--show-prefix"])?;
    let prefix = std::str::from_utf8(&prefix)
        .map_err(|e| Error::InvalidInput(e.to_string()))?
        .trim_end_matches('\n');
    let relative = path
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(value) => Some(
                value
                    .to_str()
                    .ok_or_else(|| Error::InvalidInput("Git path must be UTF-8".into())),
            ),
            _ => None,
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");
    let object = format!("{commit}:{prefix}{relative}");
    if git(root, &["cat-file", "-t", &object])? != b"tree\n" {
        return Err(Error::InvalidInput(
            "Git decision source must be a tree".into(),
        ));
    }
    let args = if recursive {
        vec!["ls-tree", "-r", "-z", &object]
    } else {
        vec!["ls-tree", "-z", &object]
    };
    let listing = git(root, &args)?;
    let mut files = vec![];
    for record in listing
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record = std::str::from_utf8(record).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let (entry, name) = record
            .split_once('\t')
            .ok_or_else(|| Error::SourceUnavailable("invalid Git tree entry".into()))?;
        let file = PathBuf::from(name);
        if (entry.starts_with("100644 blob ") || entry.starts_with("100755 blob "))
            && visible(&file)
            && markdown_path(&file)
        {
            // Read the pinned tree we just enumerated. Ref identity is restored by the indexer registration mapping.
            files.push(Locator::Git {
                revision: commit.into(),
                path: path.join(file),
            });
        }
        if files.len() > 4096 {
            return Err(Error::BudgetExceeded {
                required: files.len(),
                budget: 4096,
            });
        }
    }
    Ok(files)
}

pub(crate) fn frontmatter(text: &str) -> Result<Value> {
    let mut lines = text.split_inclusive('\n');
    if lines.next().is_none_or(|line| line.trim() != "---") {
        return Ok(serde_json::json!({}));
    }
    let mut body = String::new();
    for line in lines {
        if line.trim() == "---" || line.trim() == "..." {
            let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&body)
                .map_err(|e| Error::InvalidInput(format!("ADR metadata: {e}")))?;
            let value = serde_json::to_value(value)?;
            if !value.is_object() {
                return Err(Error::InvalidInput("ADR metadata must be a mapping".into()));
            }
            let mut metadata = serde_json::json!({});
            for (key, value) in value.as_object().unwrap() {
                let canonical = metadata_key(key).unwrap_or(key);
                insert_metadata(&mut metadata, canonical, value.clone())?;
            }
            return Ok(metadata);
        }
        body.push_str(line);
    }
    Err(Error::InvalidInput(
        "unterminated ADR metadata block".into(),
    ))
}
fn metadata_string(value: &Value, key: &str) -> Result<Option<String>> {
    match &value[key] {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(Error::InvalidInput(format!("ADR {key} must be a string"))),
    }
}
fn metadata_strings(value: &Value, key: &str) -> Result<Vec<String>> {
    match &value[key] {
        Value::Null => Ok(vec![]),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| Error::InvalidInput(format!("ADR {key} must contain strings")))
            })
            .collect(),
        _ => Err(Error::InvalidInput(format!("ADR {key} must be a list"))),
    }
}
pub(crate) fn header_fields(body: &str) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = header_field(line) else {
            break;
        };
        if let Some(previous) = fields.insert(key.to_owned(), value.to_owned()) {
            if !metadata_equal(key, &Value::String(previous), &Value::String(value.into())) {
                return Err(Error::SourceConflict(
                    "conflicting ADR header fields".into(),
                ));
            }
        }
    }
    Ok(fields)
}
fn without_header_fields(body: &str) -> String {
    let lines: Vec<_> = body.lines().collect();
    let start = lines
        .iter()
        .position(|line| !line.trim().is_empty() && header_field(line).is_none())
        .unwrap_or(lines.len());
    lines[start..].join("\n").trim().into()
}

fn decision_status(raw: &str) -> DecisionStatus {
    match raw.trim().to_lowercase().as_str() {
        "proposed" | "提议" | "拟议" | "提議" | "擬議" | "草案" => {
            DecisionStatus::Proposed
        }
        "accepted" | "已接受" | "已采纳" | "已採納" | "已通过" | "已通過" => {
            DecisionStatus::Accepted
        }
        "superseded" | "已取代" | "已替代" => DecisionStatus::Superseded,
        "rejected" | "已拒绝" | "已拒絕" | "已否决" | "已否決" => {
            DecisionStatus::Rejected
        }
        _ => DecisionStatus::Unknown,
    }
}
fn metadata_key(key: &str) -> Option<&'static str> {
    match key.trim().to_lowercase().as_str() {
        "status" | "状态" | "狀態" => Some("status"),
        "date" | "日期" => Some("date"),
        "deciders" | "决策者" | "決策者" => Some("deciders"),
        "id" | "编号" | "編號" => Some("id"),
        "title" | "标题" | "標題" => Some("title"),
        "decision" | "决策" | "決策" | "决定" | "決定" => Some("decision"),
        "rationale" | "理由" => Some("rationale"),
        "affected_keys" | "关联任务" | "關聯任務" => Some("affected_keys"),
        "paths" | "路径" | "路徑" => Some("paths"),
        _ => None,
    }
}
fn header_field(line: &str) -> Option<(&'static str, &str)> {
    let line = line.trim();
    let line = ["- ", "* ", "+ "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .unwrap_or(line)
        .trim();
    let (key, value) = line.split_once([':', '：'])?;
    let key = key.trim();
    // Both **Status**: Accepted and **Status:** Accepted are common ADR headers.
    let value = if key.starts_with("**") && !key.ends_with("**") {
        value.trim().strip_prefix("**").unwrap_or(value)
    } else {
        value
    };
    let key = metadata_key(key.trim_matches(['*', '_', '`']))?;
    Some((key, value.trim().trim_matches('`').trim()))
}
fn metadata_equal(key: &str, left: &Value, right: &Value) -> bool {
    if key == "status"
        && let (Some(a), Some(b)) = (left.as_str(), right.as_str())
    {
        return a.trim().eq_ignore_ascii_case(b.trim())
            || (decision_status(a) != DecisionStatus::Unknown
                && decision_status(a) == decision_status(b));
    }
    left == right
}
pub(crate) fn insert_metadata(metadata: &mut Value, key: &str, value: Value) -> Result<()> {
    if let Some(previous) = metadata.get(key) {
        if !metadata_equal(key, previous, &value) {
            return Err(Error::SourceConflict(
                "conflicting ADR metadata declarations".into(),
            ));
        }
    } else {
        metadata[key] = value;
    }
    Ok(())
}
