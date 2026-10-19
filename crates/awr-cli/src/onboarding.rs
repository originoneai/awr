//! Bounded, reviewable project intake. Existing documents remain authoritative.
use awr_core::{AuthorityMode, Error, Result};
use awr_source::{Manifest, ProjectConfig, SourceSpec};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long)]
    pub manifest: Option<PathBuf>,
    #[arg(long)]
    pub accept: bool,
    /// Accept only the source/configuration/ignore effects with this preview fingerprint.
    #[arg(long, requires = "accept")]
    pub expected_preview: Option<String>,
    /// User-stated project goal; otherwise the initial work is to establish one.
    #[arg(long, conflicts_with = "manifest")]
    pub goal: Option<String>,
    /// Interpret an existing ledger status without changing it: --status-map pending=planned.
    #[arg(long, conflicts_with_all=["manifest","from_draft"])]
    pub status_map: Vec<String>,
    /// Map a canonical work field to its original source key/column: --field-map title=task_name.
    #[arg(long, conflicts_with_all=["manifest","from_draft"])]
    pub field_map: Vec<String>,
    /// Save a reviewable JSON draft without initializing the project.
    #[arg(long, conflicts_with_all=["manifest","accept","from_draft"])]
    pub write_draft: Option<PathBuf>,
    /// Apply a reviewed draft whose inventory fingerprint still matches this directory.
    #[arg(long, conflicts_with_all=["manifest","goal"])]
    pub from_draft: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum IntakeCommand {
    /// Scan a file for Agent review, or archive an explicit byte-bound decision.
    Review {
        #[arg(
            long,
            required_unless_present = "from_review",
            conflicts_with = "from_review"
        )]
        source: Option<PathBuf>,
        #[arg(long, conflicts_with = "from_review")]
        write_draft: Option<PathBuf>,
        #[arg(long, conflicts_with = "source")]
        from_review: Option<PathBuf>,
    },
    /// Refresh projections and return ordered organization actions; never edits source files.
    Inspect {
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        source_sha: Option<String>,
    },
}

pub fn inspect(root: &Path, command: &IntakeCommand, json_output: bool) -> Result<()> {
    match command {
        IntakeCommand::Review {
            source,
            write_draft,
            from_review,
        } => {
            if let Some(path) = from_review {
                let bytes = awr_source::read_capped(path, 1024 * 1024)?;
                let review: awr_source::ProjectContentReview = serde_json::from_slice(&bytes)
                    .map_err(|_| Error::InvalidInput("invalid content review JSON".into()))?;
                let archived = awr_source::archive_content_review(root, &review)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"status":"review_archived","receipt":archived,"source_sha256":review.review.assessment.source_sha256,"next_action":"Retry init or source reindex. Editing the source or changing detector policy requires a new review."})
                    )?
                );
            } else {
                let review = awr_source::scan_content_review(root, source.as_deref().unwrap())?;
                let status = if review.review.assessment.findings.is_empty() {
                    "clear"
                } else if review
                    .review
                    .assessment
                    .findings
                    .iter()
                    .any(|f| !f.reviewable)
                {
                    "blocked"
                } else {
                    "review_required"
                };
                let bytes = serde_json::to_vec_pretty(&review)?;
                if let Some(path) = write_draft {
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"status":status,"review":review,"next_action":"Read the exact source locally. Remove or redact real sensitive data and rescan. For unchanged public findings, fill reviewer, reviewed_at (Unix milliseconds) and one decision {finding_id,reason} per finding; submit with intake review --from-review. Nonreviewable findings cannot be approved."})
                    )?
                );
            }
            Ok(())
        }
        IntakeCommand::Inspect { branch, source_sha } => crate::query::status(
            root,
            branch.as_deref(),
            source_sha.as_deref(),
            json_output,
            false,
        ),
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileFact {
    path: String,
    bytes: u64,
    sha256: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntakeDraft {
    schema_version: u32,
    project_root: String,
    inventory_fingerprint: String,
    inventory: Vec<FileFact>,
    git: Value,
    authority_mapping: Manifest,
    generated_files: BTreeMap<String, String>,
    gaps: Vec<String>,
    ambiguous_domains: Vec<String>,
    observations: Vec<String>,
}

const PREFIX: &str = ".awr/intake/";
/// Deepest directory level the intake walk descends into. Subtrees below this
/// are skipped and reported as observations instead of failing the intake.
const DEPTH_LIMIT: usize = 12;
/// Directory names never descended into during intake. SKIP matches at any
/// depth, so only unambiguous framework/dependency output names belong here;
/// ambiguous names (vendor, coverage, Pods) are walked so authored documents
/// under them are still indexed, and overly deep trees are handled by the
/// depth skip above, which reports every skipped subtree as an observation.
const SKIP: &[&str] = &[
    ".git",
    ".awr",
    ".codex",
    ".kimi",
    ".local",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    "_next",
    ".nuxt",
    ".output",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".gradle",
    ".terraform",
    ".dart_tool",
    ".cache",
];
fn document(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("md" | "yaml" | "yml" | "toml" | "json")
    )
}
fn inventory(root: &Path) -> Result<Vec<FileFact>> {
    let mut skipped = vec![];
    inventory_with_skips(root, &mut skipped)
}
fn inventory_with_skips(root: &Path, skipped: &mut Vec<String>) -> Result<Vec<FileFact>> {
    fn walk(
        root: &Path,
        dir: &Path,
        depth: usize,
        out: &mut Vec<FileFact>,
        skipped: &mut Vec<String>,
    ) -> Result<()> {
        if depth > DEPTH_LIMIT {
            skipped.push(
                dir.strip_prefix(root)
                    .unwrap_or(dir)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            return Ok(());
        }
        let mut entries = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if SKIP.contains(&name.as_str())
                || name.starts_with('.')
                || name.ends_with(".pem")
                || name.ends_with(".key")
                || name.contains("credentials")
                || name.contains("secrets")
            {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                walk(root, &entry.path(), depth + 1, out, skipped)?;
            } else if kind.is_file() {
                if out.len() >= 20000 {
                    return Err(Error::InvalidInput(
                        "intake exceeds 20000 files; use an explicit source manifest".into(),
                    ));
                }
                let path = entry.path();
                let bytes = entry.metadata()?.len();
                let hash = if document(&path) && bytes <= awr_source::YAML_READ_CAP {
                    Some(format!(
                        "{:x}",
                        Sha256::digest(awr_source::read_project_document(
                            root,
                            &path,
                            awr_source::YAML_READ_CAP
                        )?)
                    ))
                } else {
                    None
                };
                out.push(FileFact {
                    path: path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    bytes,
                    sha256: hash,
                });
            }
        }
        Ok(())
    }
    let mut files = vec![];
    walk(root, root, 0, &mut files, skipped)?;
    Ok(files)
}
fn git(root: &Path) -> Value {
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()
            .filter(|v| v.status.success())
            .map(|v| String::from_utf8_lossy(&v.stdout).trim().to_owned())
    };
    let head = run(&["rev-parse", "--verify", "HEAD"]);
    let changes = run(&["status", "--porcelain=v1", "--untracked-files=no"]);
    json!({"head":head,"tracked_changes":changes,"meaning":"Observed Git state only; implementation and business acceptance have not been inferred."})
}
fn fingerprint(files: &[FileFact], git: &Value) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(files, git))?)
    ))
}
fn source(domain: &str, path: &str, adapter: &str, primary: bool) -> SourceSpec {
    SourceSpec {
        domain: domain.into(),
        role: if primary { "primary" } else { "supporting" }.into(),
        path: Some(path.into()),
        locator: None,
        adapter: adapter.into(),
        options: Default::default(),
    }
}
fn intake_goal_status(goal: Option<&str>) -> &'static str {
    if goal.is_some_and(|value| !value.trim().is_empty()) {
        "active"
    } else {
        "draft"
    }
}
fn intake_goal_text(goal: Option<&str>) -> &str {
    goal.map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Project purpose and acceptance criteria still need to be established from existing material; uncertain business intent remains pending.")
}
fn intake_goal_provenance(goal: Option<&str>) -> &'static str {
    if goal.is_some_and(|value| !value.trim().is_empty()) {
        "explicit --goal input"
    } else {
        "generated intake placeholder; not confirmed"
    }
}
fn intake_goal_record(goal: Option<&str>) -> Value {
    let explicit = goal.is_some_and(|value| !value.trim().is_empty());
    let text = intake_goal_text(goal);
    json!({
        "id": "intake-goal",
        "title": if explicit { text } else { "Project goal" },
        "status": intake_goal_status(goal),
        "summary": format!("{text}\n\nProvenance: {}. Existing implementation progress remains unverified until reviewed against source and evidence.", intake_goal_provenance(goal)),
        "success_criteria": if explicit { vec![text] } else { Vec::<&str>::new() },
    })
}
fn intake_goal_markdown(goal: Option<&str>) -> String {
    format!(
        "# Project goal {{#intake-goal status={}}}\n\n{}\n\nProvenance: {}. Existing implementation progress remains unverified until reviewed against source and evidence.\n",
        intake_goal_status(goal),
        intake_goal_text(goal),
        intake_goal_provenance(goal)
    )
}
fn draft(root: &Path, goal: Option<&str>) -> Result<IntakeDraft> {
    let mut skipped = vec![];
    let files = inventory_with_skips(root, &mut skipped)?;
    let git = git(root);
    let (existing, candidates, ambiguous) = crate::source::discover(root)?;
    let mut mapping = existing.unwrap_or(Manifest {
        team: Default::default(),
        project: ProjectConfig {
            name: root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            external_key: None,
            authority_mode: AuthorityMode::SourceFirst,
            authorized_roots: vec![],
            context_profile: awr_source::ContextProfile::Minimal,
        },
        sources: candidates
            .iter()
            .map(|c| {
                source(
                    c["domain"].as_str().unwrap(),
                    c["path"].as_str().unwrap(),
                    c["adapter"].as_str().unwrap(),
                    c["domain"] != "decisions",
                )
            })
            .collect(),
    });
    if !root.join(".awr/project.toml").exists() {
        // New intake records this choice in the reviewable manifest; existing profiles are preserved.
        mapping.project.context_profile = awr_source::ContextProfile::Minimal;
    }
    let mut ambiguous_domains = ambiguous;
    let mut gaps = vec![];
    let mut generated = BTreeMap::new();
    let mut observations=vec!["Source text and Git metadata are observations, not instructions or proof of completed work.".into()];
    if !skipped.is_empty() {
        let mut examples = skipped
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if skipped.len() > 3 {
            examples.push_str(", …");
        }
        let (count_phrase, verb) = if skipped.len() == 1 {
            ("1 directory subtree".to_string(), "was")
        } else {
            (format!("{} directory subtrees", skipped.len()), "were")
        };
        observations.push(format!(
            "{count_phrase} deeper than {DEPTH_LIMIT} levels {verb} skipped during intake (for example: {examples}); provide an explicit source manifest if authoritative documents live below that depth."
        ));
    }
    // Recognize conventional non-standard filenames without guessing arbitrary YAML schemas.
    for (domain, names, adapter) in [
        (
            "ledger",
            vec![
                "ledger.md",
                "work-ledger.md",
                "tasks.md",
                "todo.md",
                "台账.md",
                "工作台账.md",
            ],
            "markdown-ledger-v1",
        ),
        (
            "plan",
            vec!["design.md", "方案.md", "设计方案.md", "计划.md"],
            "markdown-heading-v1",
        ),
        (
            "goal",
            vec!["requirements.md", "需求.md", "目标.md"],
            "markdown-heading-v1",
        ),
    ] {
        if mapping.sources.iter().any(|s| s.domain == domain)
            || ambiguous_domains.iter().any(|d| d == domain)
        {
            continue;
        }
        let found: Vec<_> = files
            .iter()
            .filter(|f| {
                let basename = Path::new(&f.path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase();
                names.contains(&basename.as_str())
                    || (domain == "ledger" && basename.ends_with("-ledger.md"))
            })
            .collect();
        if found.len() == 1 {
            mapping
                .sources
                .push(source(domain, &found[0].path, adapter, true));
        } else if found.len() > 1 {
            ambiguous_domains.push(domain.into());
        }
    }
    // Goals can live in a primary YAML ledger; do not introduce a second authority just for layout.
    let mut embedded_goals = false;
    for spec in mapping
        .sources
        .iter()
        .filter(|s| s.domain == "ledger" && s.role == "primary" && s.adapter == "yaml-ledger-v1")
    {
        let snapshot = awr_source::Locator::from_spec(root, &mapping, spec)?
            .read(root, awr_source::YAML_READ_CAP)?;
        let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(snapshot.text()?)
            .map_err(|_| Error::InvalidInput("invalid YAML ledger document".into()))?;
        embedded_goals |= document.get("goals").is_some_and(|g| {
            g.as_sequence().is_some_and(|v| !v.is_empty())
                || g.as_mapping().is_some_and(|v| !v.is_empty())
        });
    }
    let goal_missing = !embedded_goals
        && !mapping.sources.iter().any(|s| s.domain == "goal")
        && !ambiguous_domains.iter().any(|d| d == "goal");
    let ledger_missing = !mapping.sources.iter().any(|s| s.domain == "ledger")
        && !ambiguous_domains.iter().any(|d| d == "ledger");
    // A new YAML ledger can hold the placeholder goal. Do not add a path-derived Markdown key.
    if goal_missing {
        gaps.push(
            "No authoritative goal source was identified; the proposed intake source needs review."
                .into(),
        );
    }
    if ledger_missing {
        gaps.push("No authoritative ledger source was identified; the proposed intake source needs review.".into());
        let mut tasks = vec![
            json!({"id":"INTAKE-001","kind":"intake","title":"Verify project goals, current state, and the next deliverable","status":"ready","priority":"P0","required":true,"depends_on":[],"acceptance":["Confirm goals, existing implementation, unfinished work, and blockers with source references.","Mark uncertain progress as unverified and define acceptance criteria for the next executable task."],"next_action":"Run awr intake inspect and read sources for each gap; add goal references, acceptance criteria, and next actions, then inspect again.","summary":"This is a new intake task; it does not establish whether existing functionality is implemented or accepted."}),
        ];
        // A plan creates reviewable task proposals, never fabricated completed work.
        for spec in mapping
            .sources
            .iter()
            .filter(|s| s.domain == "plan" && s.adapter == "markdown-heading-v1")
        {
            if let Some(path) = &spec.path {
                if !root.join(path).is_file() {
                    continue;
                }
                let snapshot = awr_source::Locator::from_spec(root, &mapping, spec)?
                    .read(root, awr_source::MARKDOWN_READ_CAP)?;
                for section in awr_source::markdown_sections(&snapshot)?
                    .into_iter()
                    .take(100)
                {
                    tasks.push(json!({"id":format!("INTAKE-{:03}",tasks.len()+1),"title":format!("Verify and advance: {}",section.title),"status":"planned","priority":"P1","depends_on":["INTAKE-001"],"acceptance":["Confirm deliverables and acceptance criteria against the original plan, verify existing progress, then execute."],"next_action":format!("Review the original plan section in {}; first add concrete actions and acceptance criteria.",path.display()),"paths":[path],"summary":"Task proposal derived from a plan section, pending review; implementation status is not yet assessed."}));
                }
            }
        }
        let mut document = json!({"work_items": tasks});
        if goal_missing {
            document["goals"] = json!([intake_goal_record(goal)]);
        }
        let body =
            serde_yaml_ng::to_string(&document).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let path = format!("{PREFIX}work-ledger.yaml");
        generated.insert(path.clone(), body);
        mapping
            .sources
            .push(source("ledger", &path, "yaml-ledger-v1", true));
    } else if goal_missing {
        let body = intake_goal_markdown(goal);
        let path = format!("{PREFIX}GOALS.md");
        generated.insert(path.clone(), body);
        mapping
            .sources
            .push(source("goal", &path, "markdown-heading-v1", true));
    }
    for name in ["README.md", "AGENTS.md"] {
        if files.iter().any(|f| f.path == name)
            && !mapping
                .sources
                .iter()
                .any(|s| s.path.as_deref() == Some(Path::new(name)))
        {
            // These are supporting material; a heading is not automatically a hard rule.
            mapping
                .sources
                .push(source("plan", name, "markdown-heading-v1", false));
        }
    }
    if mapping
        .sources
        .iter()
        .any(|s| s.adapter == "markdown-ledger-v1")
    {
        observations.push("The existing Markdown ledger remains authoritative and read-only in AWR; source edits stay in that file and are reindexed.".into());
    }
    Ok(IntakeDraft {
        schema_version: 1,
        project_root: root.to_string_lossy().into_owned(),
        inventory_fingerprint: fingerprint(&files, &git)?,
        inventory: files,
        git,
        authority_mapping: mapping,
        generated_files: generated,
        gaps,
        ambiguous_domains,
        observations,
    })
}

pub fn run(root: &Path, args: &InitArgs, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    if root.join(".awr/project.toml").exists()
        && (!args.field_map.is_empty() || !args.status_map.is_empty())
    {
        return Err(Error::InvalidInput("project already has a manifest; edit its source options and run source reindex to update mappings".into()));
    }
    if args.manifest.is_some()
        || (root.join(".awr/project.toml").exists()
            && args.from_draft.is_none()
            && args.goal.is_none()
            && args.write_draft.is_none())
    {
        return crate::source::initialize(
            &root,
            args.manifest.as_deref(),
            args.accept,
            json_output,
            args.expected_preview.as_deref(),
        );
    }
    let mut candidate = if let Some(path) = &args.from_draft {
        let bytes = awr_source::read_source_capped(&path.canonicalize()?, 4 * 1024 * 1024)?;
        let value: IntakeDraft = serde_json::from_slice(&bytes)
            .map_err(|_| Error::InvalidInput("invalid intake draft JSON".into()))?;
        if value.schema_version != 1
            || value.project_root != root.to_string_lossy()
            || value.inventory_fingerprint != fingerprint(&inventory(&root)?, &git(&root))?
        {
            return Err(Error::SourceConflict("project inventory changed or draft belongs to another directory; regenerate and review the intake draft".into()));
        }
        value
    } else {
        match draft(&root, args.goal.as_deref()) {
            Ok(draft) => draft,
            Err(error) => {
                if let Some(path) = &args.write_draft {
                    if error
                        .report()
                        .details
                        .as_ref()
                        .is_some_and(|v| v["rule"] == "source.public_content")
                    {
                        let diagnostic = json!({"schema_version":1,"status":"content_review_required","project_root":root,"diagnostic":error.report(),"next_action":"Use intake review --source <diagnostic locator> --write-draft <review.json>. This diagnostic contains no source body and cannot be accepted as an intake draft."});
                        let mut file = fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(path)?;
                        file.write_all(&serde_json::to_vec_pretty(&diagnostic)?)?;
                        file.sync_all()?;
                    }
                }
                return Err(error);
            }
        }
    };
    if !args.field_map.is_empty() || !args.status_map.is_empty() {
        let ledgers: Vec<_> = candidate
            .authority_mapping
            .sources
            .iter_mut()
            .filter(|s| s.domain == "ledger" && s.role == "primary")
            .collect();
        if ledgers.len() != 1 || candidate.ambiguous_domains.iter().any(|d| d == "ledger") {
            return Err(Error::InvalidInput("mapping flags require exactly one primary ledger; resolve the draft or supply a manifest".into()));
        }
        let spec = ledgers.into_iter().next().unwrap();
        for (name, entries) in [
            ("field_map", &args.field_map),
            ("status_map", &args.status_map),
        ] {
            if entries.is_empty() {
                continue;
            }
            let mut table = toml::Table::new();
            for entry in entries {
                let (key, value) = entry
                    .split_once('=')
                    .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
                    .ok_or_else(|| {
                        Error::InvalidInput(format!("{name} entries must use KEY=VALUE"))
                    })?;
                if table
                    .insert(key.trim().into(), toml::Value::String(value.trim().into()))
                    .is_some()
                {
                    return Err(Error::SourceConflict(format!("duplicate {name} entry")));
                }
            }
            spec.options.insert(name.into(), toml::Value::Table(table));
        }
        awr_source::LedgerMapping::from_spec(spec)?;
        candidate.observations.push("Explicit ledger field/status mappings interpret source values without rewriting the original documents.".into());
    }
    awr_core::ensure_public_data(&candidate)?;
    let mut generated = candidate.generated_files.clone();
    generated.insert(
        ".awr/intake/inventory.json".into(),
        serde_json::to_string_pretty(&candidate)?,
    );
    generated.insert(
        ".awr/intake/project.toml".into(),
        toml::to_string_pretty(&candidate.authority_mapping)
            .map_err(|e| Error::InvalidInput(e.to_string()))?,
    );
    let preview =
        crate::intake_plan::preview(&root, &candidate.authority_mapping, &generated, false)?;
    crate::intake_plan::check_expected(&preview, args.expected_preview.as_deref())?;
    if let Some(path) = &args.write_draft {
        let bytes = serde_json::to_vec_pretty(&candidate)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    if !args.accept {
        let mut missing = candidate
            .generated_files
            .keys()
            .filter_map(|p| {
                if p.ends_with("GOALS.md") {
                    Some("goal".into())
                } else if p.ends_with("work-ledger.yaml") {
                    Some("ledger".into())
                } else {
                    None
                }
            })
            .collect::<Vec<String>>();
        if candidate
            .generated_files
            .get(&format!("{PREFIX}work-ledger.yaml"))
            .is_some_and(|body| body.contains("id: intake-goal"))
            && !missing.iter().any(|domain| domain == "goal")
        {
            missing.push("goal".into());
        }
        let sources = candidate.authority_mapping.sources.iter().map(|s| json!({"domain":s.domain,"role":s.role,"path":s.path,"adapter":s.adapter,"proposed":s.path.as_ref().is_some_and(|p|candidate.generated_files.contains_key(&p.to_string_lossy().into_owned()))})).collect();
        let mut organization = awr_runtime::OrganizationReport::intake_preview(
            sources,
            &missing,
            &candidate.ambiguous_domains,
        );
        organization.context_profile = if candidate.authority_mapping.project.context_profile
            == awr_source::ContextProfile::Minimal
        {
            "minimal"
        } else {
            "standard"
        };
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"status":"preview","requires_accept":true,"preview":preview,"draft":candidate,"organization":organization,"ambiguous_domains":candidate.ambiguous_domains,"authority_mapping":if candidate.ambiguous_domains.is_empty(){Some(&candidate.authority_mapping)}else{None},"next_action":"Review the draft and organization actions. Use init --accept, or edit an external --write-draft before init --from-draft <file> --accept. Then run awr intake inspect."})
            )?
        );
        return Ok(());
    }
    if !candidate.ambiguous_domains.is_empty() {
        return Err(Error::SourceConflict("multiple authority candidates; supply an explicit manifest or resolve the reviewed draft mapping".into()));
    }
    crate::intake_plan::require_applicable(&preview)?;
    candidate.authority_mapping.validate()?;
    let mut primary = BTreeSet::new();
    for spec in &candidate.authority_mapping.sources {
        if spec.role == "primary" && !primary.insert(&spec.domain) {
            return Err(Error::SourceConflict(
                "multiple primary sources in intake draft".into(),
            ));
        }
    }
    let allowed = ["GOALS.md", "PLAN.md", "RULES.md", "work-ledger.yaml"];
    for path in candidate.generated_files.keys() {
        if !allowed
            .iter()
            .any(|name| path == &format!("{PREFIX}{name}"))
        {
            return Err(Error::RuleViolation(
                "draft may create only the four owned intake source files".into(),
            ));
        }
    }
    if root.join(".awr/project.toml").exists() || root.join(".awr/intake").exists() {
        return Err(Error::SourceConflict(
            "intake files already exist; preserve them and use their explicit manifest".into(),
        ));
    }
    let runtime = crate::source::runtime_dir(&root, true)?;
    let stage = runtime.join(format!("intake-{}", awr_core::Id::new()));
    fs::create_dir(&stage)?;
    for (path, body) in &candidate.generated_files {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stage.join(Path::new(path).file_name().unwrap()))?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    fs::write(
        stage.join("inventory.json"),
        serde_json::to_vec_pretty(&candidate)?,
    )?;
    fs::write(
        stage.join("project.toml"),
        toml::to_string_pretty(&candidate.authority_mapping)
            .map_err(|e| Error::InvalidInput(e.to_string()))?,
    )?;
    // Recheck after staging, before making the intake authoritative.
    if candidate.inventory_fingerprint != fingerprint(&inventory(&root)?, &git(&root))? {
        return Err(Error::SourceConflict(
            "project changed during intake; staged draft retained for inspection".into(),
        ));
    }
    // Staging is inside the ignored runtime; published targets and sources must
    // still match the effect inventory accepted by the host.
    if args.expected_preview.is_some() {
        let current =
            crate::intake_plan::preview(&root, &candidate.authority_mapping, &generated, false)?;
        crate::intake_plan::check_expected(&current, args.expected_preview.as_deref())?;
    }
    fs::rename(&stage, runtime.join("intake"))?;
    crate::source::initialize(
        &root,
        Some(&runtime.join("intake/project.toml")),
        true,
        json_output,
        None,
    )
    .map_err(|error| match error {
        Error::IntakePreflightRejected { issues, .. } => Error::IntakePreflightRejected {
            issues,
            intake_staged: true,
        },
        other => other,
    })
}
