use crate::{Checkpoint, Claim, Id, Session};
use serde::{Deserialize, Serialize};

pub const CHECKPOINT_LIMITATION: &str = "Progress record only: context hash and caller identity are not independently verified. Saving a checkpoint does not update source-backed work or prove tests passed or work completed.";

pub const INCOMPLETE_CONTEXT_GUIDANCE: &str = "Required context has gaps. Do not claim complete understanding, passing tests or completed work. You may save an incomplete checkpoint with the actual last-used context hash, explicit gaps and open loops; do not invent a hash. A checkpoint does not change the source-backed next action.";

/// Stable host conversation identity, independent of any MCP transport connection.
/// `client` is an MCP namespace, distinct from the hook dialect in `ClientBinding`.
/// HTTP supplies its authenticated registry client; stdio supplies `stdio`.
/// CLI values are caller-supplied and do not themselves prove authentication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpSessionBinding {
    pub client: String,
    pub conversation: String,
}
impl McpSessionBinding {
    pub fn validate(&self) -> crate::Result<()> {
        crate::ensure_public_data(self)?;
        if self.client.trim().is_empty()
            || self.client.len() > 128
            || self.conversation.trim().is_empty()
            || self.conversation.len() > 512
            || self
                .client
                .chars()
                .chain(self.conversation.chars())
                .any(char::is_control)
        {
            return Err(crate::Error::InvalidInput(
                "invalid MCP client/conversation identity".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOperation {
    pub id: Id,
    pub client: String,
    pub request_id: String,
    pub tool: String,
    pub fingerprint: String,
    pub expected_revision: crate::Revision,
    pub started_revision: crate::Revision,
    pub status: String,
    pub result: Option<serde_json::Value>,
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpWait {
    pub id: Id,
    pub client: String,
    pub session_id: Id,
    pub checkpoint_id: Id,
    pub question: String,
    pub status: String,
    pub reply: Option<String>,
    pub created_at: i64,
    pub revision: crate::Revision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointDraft {
    pub context_hash: String,
    pub digest: String,
    pub next_action: String,
    pub open_loops: Vec<String>,
    pub changed_entities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactDraft {
    pub artifact_type: String,
    pub locator: String,
    pub sha256: String,
    pub size: u64,
    pub mime: String,
    pub source_event_id: Id,
}

pub fn is_sha256_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDraft {
    pub work_item_key: Option<String>,
    pub agent_id: String,
    pub provider: String,
    pub model: String,
    pub branch_id: Option<Id>,
    pub claim: bool,
    pub claim_ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStarted {
    pub session: Session,
    pub claim: Option<Claim>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumeClaim {
    #[default]
    Inherit,
    Acquire,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResumeDraft {
    pub from_session_id: Id,
    pub checkpoint_id: Option<Id>,
    pub agent_id: String,
    pub provider: String,
    pub model: String,
    pub claim: ResumeClaim,
    pub claim_ttl_ms: Option<u64>,
    pub prepared_context_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResumed {
    pub from_session: Session,
    pub session: Session,
    pub checkpoint: Option<Checkpoint>,
    pub claim: Option<Claim>,
    pub closed_claim_ids: Vec<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub from_session: Session,
    pub to_session: Option<Session>,
    pub checkpoint: Checkpoint,
    pub closed_claim_ids: Vec<Id>,
    pub transferred_claim: Option<Claim>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    Ended,
    Interrupted,
    Incomplete,
}
impl SessionOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ended => "ended",
            Self::Interrupted => "interrupted",
            Self::Incomplete => "incomplete",
        }
    }
}
impl Claim {
    pub fn active_at(&self, at: i64) -> bool {
        self.status == "active"
            && self.released_at.is_none()
            && self.expires_at.is_none_or(|end| end > at)
    }
}
