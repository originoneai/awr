use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type Id = ulid::Ulid;
pub type Revision = u64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Goal,
    Plan,
    Rule,
    WorkItem,
    Decision,
    Evidence,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityMode {
    SourceFirst,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Draft,
    Planned,
    Ready,
    Claimed,
    InProgress,
    Blocked,
    Completed,
    Cancelled,
    Unknown,
}

impl WorkStatus {
    pub fn normalize(raw: &str) -> Self {
        match raw {
            "draft" => Self::Draft,
            "planned" => Self::Planned,
            "ready" => Self::Ready,
            "claimed" => Self::Claimed,
            "in_progress" => Self::InProgress,
            "blocked" => Self::Blocked,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Hard,
    Soft,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Project,
    Path,
    Tag,
    WorkItem,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    #[serde(rename = "type")]
    pub kind: ScopeKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    Designed,
    Implemented,
    LocallyVerified,
    RealEnvironmentValidated,
    ReleaseCandidate,
    Released,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub source_id: Id,
    pub locator: String,
    pub source_revision: Revision,
    pub source_fingerprint: String,
    pub pointer: Option<String>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    #[serde(default)]
    pub section_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionMeta {
    pub id: Id,
    pub external_key: String,
    pub revision: Revision,
    pub source_ref: SourceRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub external_key: String,
    pub name: String,
    pub root: PathBuf,
    pub authority_mode: AuthorityMode,
    pub current_branch_id: Option<Id>,
    pub project_revision: Revision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: Id,
    pub project_id: Id,
    pub domain: String,
    pub role: String,
    pub locator: String,
    pub format: String,
    pub adapter: String,
    pub revision: Revision,
    pub fingerprint: String,
    pub freshness: Freshness,
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    #[serde(flatten)]
    pub meta: ProjectionMeta,
    pub title: String,
    pub status: String,
    pub priority: Option<String>,
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    #[serde(flatten)]
    pub meta: ProjectionMeta,
    pub title: String,
    pub status: String,
    pub scope: Vec<String>,
    pub summary: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    #[serde(flatten)]
    pub meta: ProjectionMeta,
    pub text: String,
    pub severity: Option<Severity>,
    pub scope: Option<Scope>,
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinary_completion: Option<crate::OrdinaryCompletion>,
    #[serde(flatten)]
    pub meta: ProjectionMeta,
    pub title: String,
    pub kind: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    pub required: bool,
    pub raw_status: String,
    pub status: WorkStatus,
    pub priority: Option<String>,
    pub milestone: Option<String>,
    pub score: Option<i64>,
    pub evidence_level: Option<EvidenceLevel>,
    pub summary: String,
    pub next_action: String,
    pub blocker: Option<String>,
    pub acceptance: Vec<String>,
    pub tags: Vec<String>,
    pub paths: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: Id,
    pub project_id: Id,
    pub from_kind: EntityKind,
    pub from_key: String,
    pub relation: String,
    pub to_kind: EntityKind,
    pub to_key: String,
    pub required: bool,
    pub revision: Revision,
    pub source_ref: SourceRef,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Superseded,
    Rejected,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    #[serde(flatten)]
    pub meta: ProjectionMeta,
    pub title: String,
    pub status: DecisionStatus,
    #[serde(default)]
    pub raw_status: String,
    pub decision: String,
    pub rationale: String,
    pub affected_keys: Vec<String>,
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adoption: Option<DecisionAdoption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<DocumentVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DocumentVersion {
    pub source_id: Id,
    pub external_key: String,
    pub source_fingerprint: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecisionAdoption {
    pub version: u32,
    pub request_key: String,
    pub actor: crate::HostActor,
    pub reason: String,
    pub candidate: DocumentVersion,
    pub supersedes: Option<DocumentVersion>,
    pub content_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub id: Id,
    pub project_id: Id,
    pub work_item_id: Option<Id>,
    pub external_key: String,
    pub evidence_type: String,
    pub level: EvidenceLevel,
    pub summary: String,
    pub locator: String,
    pub sha256: Option<String>,
    pub source_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_fingerprint: Option<String>,
    pub command: Option<String>,
    pub scope: Vec<String>,
    pub source_ref: Option<SourceRef>,
    pub branch_id: Option<Id>,
    pub revision: Revision,
    pub verified_at: Option<i64>,
}

/// Parsed source facts only. Runtime state is never reconstructed by an adapter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectionBatch {
    pub goals: Vec<Goal>,
    pub plans: Vec<Plan>,
    pub rules: Vec<Rule>,
    pub work_items: Vec<WorkItem>,
    pub edges: Vec<Edge>,
    pub decisions: Vec<Decision>,
    pub evidence: Vec<Evidence>,
    pub warnings: Vec<String>,
    /// Explicit source-owned scope definitions; absence retains legacy behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workstream_projection: Option<crate::WorkstreamProjection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Id,
    pub project_id: Id,
    pub work_item_id: Option<Id>,
    pub session_id: Option<Id>,
    pub branch_id: Option<Id>,
    pub event_type: String,
    pub importance: String,
    pub summary: String,
    pub payload: serde_json::Value,
    pub project_revision: Revision,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDraft {
    pub work_item_id: Option<Id>,
    pub session_id: Option<Id>,
    pub branch_id: Option<Id>,
    pub event_type: String,
    pub importance: String,
    pub summary: String,
    pub payload: serde_json::Value,
}

impl EventDraft {
    pub fn new(event_type: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            work_item_id: None,
            session_id: None,
            branch_id: None,
            event_type: event_type.into(),
            importance: "normal".into(),
            summary: summary.into(),
            payload: serde_json::json!({}),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Id,
    pub project_id: Id,
    pub work_item_id: Option<Id>,
    pub branch_id: Option<Id>,
    pub agent_id: String,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub start_project_revision: Revision,
    pub end_project_revision: Option<Revision>,
    pub last_checkpoint_id: Option<Id>,
    pub revision: Revision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub id: Id,
    pub project_id: Id,
    pub work_item_id: Id,
    pub session_id: Id,
    pub agent_id: String,
    pub branch_id: Option<Id>,
    pub status: String,
    pub acquired_at: i64,
    pub expires_at: Option<i64>,
    pub released_at: Option<i64>,
    pub revision: Revision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: Id,
    pub session_id: Id,
    pub project_revision: Revision,
    pub context_hash: String,
    pub digest: String,
    pub next_action: String,
    pub open_loops: Vec<String>,
    pub changed_entities: Vec<String>,
    pub revision: Revision,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: Id,
    pub project_id: Id,
    pub name: String,
    pub parent_branch_id: Option<Id>,
    pub git_ref: Option<String>,
    pub fork_project_revision: Revision,
    pub status: String,
    pub revision: Revision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: Id,
    pub project_id: Id,
    pub artifact_type: String,
    pub locator: String,
    pub sha256: String,
    pub size: u64,
    pub mime: String,
    pub source_event_id: Option<Id>,
    pub revision: Revision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Draft,
    Ready,
    Approved,
    Applied,
    Conflict,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationProposal {
    pub id: Id,
    pub project_id: Id,
    pub work_item_id: Option<Id>,
    pub source_id: Id,
    pub base_fingerprint: String,
    pub expected_revision: Revision,
    pub mutation_type: String,
    pub patch: serde_json::Value,
    pub status: ProposalStatus,
    pub created_by_session: Option<Id>,
    pub revision: Revision,
}
