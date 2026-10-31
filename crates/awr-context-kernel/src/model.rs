//! The data the kernel takes and gives. Plain values only: no handle to a store, a file, a clock or a model.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
/// The one packing policy format 1 names: required chunks are indivisible, optional ones are tried in stable rank
/// order, a chunk that does not fit is skipped so that a later, smaller one may fit, and the whole text is measured.
pub const POLICY_WHOLE_CHUNKS_V1: &str = "awr.whole_chunks.v1";
pub const SUPPORTED_POLICIES: [&str; 1] = [POLICY_WHOLE_CHUNKS_V1];
pub const ENCODING_UTF8: &str = "utf-8";
pub const MEDIA_PLAIN: &str = "text/plain";
pub const MEDIA_MARKDOWN: &str = "text/markdown";
pub const SUPPORTED_MEDIA: [&str; 2] = [MEDIA_PLAIN, MEDIA_MARKDOWN];
/// What a result says about itself, always.
pub const SCOPE_NOTE: &str = "A structural packing result. It grants no authority, says nothing of readiness, completeness or acceptance, and is not an execution admission.";

fn utf8() -> String {
    ENCODING_UTF8.into()
}

fn plain() -> String {
    MEDIA_PLAIN.into()
}

/// What the host's identity pins and what a chunk is about. Ids are the host's own strings, never database keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    pub kind: String,
    pub id: String,
    pub revision: u64,
}

/// A piece of text with a stable name. `(section, key)` is unique in a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub key: String,
    pub section: String,
    /// Where the section stands in the rendered text; sections render in this order, chunks in key order.
    #[serde(default)]
    pub section_rank: u16,
    #[serde(default = "utf8")]
    pub encoding: String,
    #[serde(default = "plain")]
    pub media: String,
    pub text: String,
    #[serde(default)]
    pub entities: Vec<Entity>,
    /// What the host measured for this text in the budget's unit. Without it the kernel can judge a byte budget by the
    /// length of the text, and a token budget only by the measurer of the packing step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<u64>,
}

/// An optional chunk and its place in the order of trial: lower priority first, then newer, then section and key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ranked {
    pub priority: u16,
    pub recency: u64,
    pub chunk: Chunk,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkRef {
    pub section: String,
    pub key: String,
}

/// Chunks that are selected together or not at all. A group is one unit of trial for the packing step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtomicGroup {
    pub id: String,
    pub members: Vec<ChunkRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetUnit {
    ModelTokens,
    BoundedBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exactness {
    Exact,
    Estimate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub unit: BudgetUnit,
    pub limit: u64,
    pub exactness: Exactness,
    /// What the count covers and what it leaves out, in words.
    pub scope: String,
    /// The tokenizer a token budget is counted with; required for `model_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokenizer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackRequest {
    pub protocol_version: u32,
    pub policy: String,
    /// The host's own identity binding for this request. Opaque here; it goes into the digest of the request.
    pub identity: String,
    /// Entities the identity pins; a chunk that names one of them at another revision conflicts with it.
    #[serde(default)]
    pub pinned_entities: Vec<Entity>,
    /// The host's request binding. Opaque JSON, bounded.
    #[serde(default)]
    pub request_binding: Value,
    pub budget: Budget,
    #[serde(default)]
    pub required: Vec<Chunk>,
    #[serde(default)]
    pub optional: Vec<Ranked>,
    #[serde(default)]
    pub groups: Vec<AtomicGroup>,
}

impl PackRequest {
    /// Every chunk of the request with whether it is required, required ones first, in the order given.
    pub fn chunks(&self) -> impl Iterator<Item = (&Chunk, bool)> {
        self.required
            .iter()
            .map(|chunk| (chunk, true))
            .chain(self.optional.iter().map(|ranked| (&ranked.chunk, false)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub section: String,
    pub key: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// Why an optional chunk is not in the text: it did not fit, or its group did not fit as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmissionReason {
    Budget,
    GroupBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Omission {
    pub section: String,
    pub key: String,
    pub reason: OmissionReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// What was counted. `required_used` is the count of the text holding only the required chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub unit: BudgetUnit,
    pub exactness: Exactness,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokenizer: Option<String>,
    pub limit: u64,
    pub required_used: u64,
    pub total_used: u64,
}

/// The outcome of packing, as the contract fixes it. Every chunk of the request is in `selected` or in `omitted`, once.
/// It carries no word about authority, readiness, completeness or acceptance: those are not the kernel's to give.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackResult {
    pub protocol_version: u32,
    pub policy: String,
    pub request_digest: String,
    pub selected: Vec<Selection>,
    pub omitted: Vec<Omission>,
    pub measurement: Measurement,
    /// `sha256:` and 64 hex digits of the text that was rendered.
    pub rendered_hash: String,
    pub scope_note: String,
}
