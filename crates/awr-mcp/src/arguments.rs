use awr_core::*;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct StatusArgs {
    pub view: Option<String>,
    #[serde(default)]
    pub work: Vec<String>,
    pub goal: Option<String>,
    pub milestone: Option<String>,
    pub branch: Option<String>,
    pub source_sha: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadyArgs {
    pub branch: Option<String>,
    #[serde(default = "ten")]
    pub limit: usize,
}
fn ten() -> usize {
    10
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkArgs {
    pub work: String,
    pub branch: Option<String>,
    pub source_sha: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchArgs {
    pub text: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub work: Option<String>,
    #[serde(default = "ten")]
    pub limit: usize,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ContextArgs {
    pub work: Option<String>,
    pub session: Option<Id>,
    pub detached: bool,
    pub agent: Option<String>,
    pub branch: Option<String>,
    pub goals: Vec<String>,
    pub paths: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub source_sha: Option<String>,
    pub intent: Option<String>,
    pub budget: Option<usize>,
    pub checkpoint: Option<Id>,
    pub after_revision: Option<Revision>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransitionArgs {
    pub work: String,
    pub action: WorkAction,
    pub session: Id,
    pub expected_revision: Revision,
    pub reason: String,
    pub next_action: Option<String>,
    pub summary: Option<String>,
    pub blocker: Option<String>,
    pub completion: Option<CompletionInput>,
    /// Only for `complete`: end the session in the same request.
    pub end_session: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventArgs {
    pub expected_revision: Revision,
    pub work: Option<String>,
    pub session: Option<Id>,
    pub branch: Option<String>,
    pub event_type: String,
    pub importance: Option<String>,
    pub summary: String,
    pub payload: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceArgs {
    pub expected_revision: Revision,
    pub external_key: String,
    pub work: Option<String>,
    pub evidence_type: String,
    pub level: EvidenceLevel,
    pub summary: String,
    pub locator: String,
    pub sha256: Option<String>,
    pub source_sha: Option<String>,
    pub command: Option<String>,
    pub scope: Vec<String>,
    pub branch: Option<String>,
    pub verified_at: Option<i64>,
}
