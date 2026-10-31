//! The checks of the contract: what a request must be before it is packed, and what a result must be to be believed.
//!
//! Both are structural. A request that passes is well formed, not authorized, complete or ready; a result that passes
//! is a consistent partition of its request, not a statement about the work.
use crate::model::*;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The bounds a request is checked against. A host may tighten them, never read a request above them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_chunks: usize,
    pub max_groups: usize,
    pub max_group_members: usize,
    pub max_entities_per_chunk: usize,
    pub max_pinned_entities: usize,
    pub max_chunk_bytes: usize,
    pub max_total_bytes: usize,
    pub max_name_bytes: usize,
    pub max_identity_bytes: usize,
    pub max_binding_bytes: usize,
    pub max_model_tokens: u64,
    pub max_bounded_bytes: u64,
    pub max_issues: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_chunks: 4_096,
            max_groups: 1_024,
            max_group_members: 256,
            max_entities_per_chunk: 64,
            max_pinned_entities: 1_024,
            max_chunk_bytes: 256 * 1024,
            max_total_bytes: 8 * 1024 * 1024,
            max_name_bytes: 256,
            max_identity_bytes: 1_024,
            max_binding_bytes: 16 * 1024,
            max_model_tokens: 1_000_000,
            max_bounded_bytes: 16 * 1024 * 1024,
            max_issues: 64,
        }
    }
}

/// Why a request or a result is refused. The codes are stable. None of them carries chunk text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackIssueCode {
    UnsupportedProtocol,
    UnsupportedPolicy,
    InvalidIdentity,
    BindingTooLarge,
    BudgetOutOfRange,
    InvalidBudgetScope,
    TokenizerMissing,
    TooManyChunks,
    TooManyGroups,
    TooManyPinnedEntities,
    TooManyEntities,
    ChunkTooLarge,
    InputTooLarge,
    InvalidName,
    UnsupportedEncoding,
    UnsupportedMedia,
    InvalidEntity,
    DuplicateChunk,
    EntityRevisionConflict,
    DuplicateGroup,
    GroupEmpty,
    GroupTooLarge,
    GroupMemberMissing,
    GroupMemberRepeated,
    GroupOverlap,
    GroupMixesRequiredAndOptional,
    MandatoryOverflow,
    ResultProtocolMismatch,
    ResultDigestMismatch,
    ResultChunkMissing,
    ResultChunkUnknown,
    ResultChunkTwice,
    ResultRequiredOmitted,
    ResultRequiredFlagMismatch,
    ResultGroupSplit,
    ResultGroupMismatch,
    ResultMeasurementMismatch,
    ResultOverBudget,
    ResultHashMalformed,
    ResultScopeNote,
}

impl PackIssueCode {
    /// Every code, so that a corpus can be held to exercise each one.
    pub const ALL: [PackIssueCode; 40] = [
        Self::UnsupportedProtocol,
        Self::UnsupportedPolicy,
        Self::InvalidIdentity,
        Self::BindingTooLarge,
        Self::BudgetOutOfRange,
        Self::InvalidBudgetScope,
        Self::TokenizerMissing,
        Self::TooManyChunks,
        Self::TooManyGroups,
        Self::TooManyPinnedEntities,
        Self::TooManyEntities,
        Self::ChunkTooLarge,
        Self::InputTooLarge,
        Self::InvalidName,
        Self::UnsupportedEncoding,
        Self::UnsupportedMedia,
        Self::InvalidEntity,
        Self::DuplicateChunk,
        Self::EntityRevisionConflict,
        Self::DuplicateGroup,
        Self::GroupEmpty,
        Self::GroupTooLarge,
        Self::GroupMemberMissing,
        Self::GroupMemberRepeated,
        Self::GroupOverlap,
        Self::GroupMixesRequiredAndOptional,
        Self::MandatoryOverflow,
        Self::ResultProtocolMismatch,
        Self::ResultDigestMismatch,
        Self::ResultChunkMissing,
        Self::ResultChunkUnknown,
        Self::ResultChunkTwice,
        Self::ResultRequiredOmitted,
        Self::ResultRequiredFlagMismatch,
        Self::ResultGroupSplit,
        Self::ResultGroupMismatch,
        Self::ResultMeasurementMismatch,
        Self::ResultOverBudget,
        Self::ResultHashMalformed,
        Self::ResultScopeNote,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::UnsupportedPolicy => "unsupported_policy",
            Self::InvalidIdentity => "invalid_identity",
            Self::BindingTooLarge => "binding_too_large",
            Self::BudgetOutOfRange => "budget_out_of_range",
            Self::InvalidBudgetScope => "invalid_budget_scope",
            Self::TokenizerMissing => "tokenizer_missing",
            Self::TooManyChunks => "too_many_chunks",
            Self::TooManyGroups => "too_many_groups",
            Self::TooManyPinnedEntities => "too_many_pinned_entities",
            Self::TooManyEntities => "too_many_entities",
            Self::ChunkTooLarge => "chunk_too_large",
            Self::InputTooLarge => "input_too_large",
            Self::InvalidName => "invalid_name",
            Self::UnsupportedEncoding => "unsupported_encoding",
            Self::UnsupportedMedia => "unsupported_media",
            Self::InvalidEntity => "invalid_entity",
            Self::DuplicateChunk => "duplicate_chunk",
            Self::EntityRevisionConflict => "entity_revision_conflict",
            Self::DuplicateGroup => "duplicate_group",
            Self::GroupEmpty => "group_empty",
            Self::GroupTooLarge => "group_too_large",
            Self::GroupMemberMissing => "group_member_missing",
            Self::GroupMemberRepeated => "group_member_repeated",
            Self::GroupOverlap => "group_overlap",
            Self::GroupMixesRequiredAndOptional => "group_mixes_required_and_optional",
            Self::MandatoryOverflow => "mandatory_overflow",
            Self::ResultProtocolMismatch => "result_protocol_mismatch",
            Self::ResultDigestMismatch => "result_digest_mismatch",
            Self::ResultChunkMissing => "result_chunk_missing",
            Self::ResultChunkUnknown => "result_chunk_unknown",
            Self::ResultChunkTwice => "result_chunk_twice",
            Self::ResultRequiredOmitted => "result_required_omitted",
            Self::ResultRequiredFlagMismatch => "result_required_flag_mismatch",
            Self::ResultGroupSplit => "result_group_split",
            Self::ResultGroupMismatch => "result_group_mismatch",
            Self::ResultMeasurementMismatch => "result_measurement_mismatch",
            Self::ResultOverBudget => "result_over_budget",
            Self::ResultHashMalformed => "result_hash_malformed",
            Self::ResultScopeNote => "result_scope_note",
        }
    }
}

/// One refusal. `section` and `key` name the chunk it is about; they are the host's own identifiers, cut at 80 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackIssue {
    pub code: PackIssueCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl PackIssue {
    fn new(code: PackIssueCode) -> Self {
        Self {
            code,
            section: None,
            key: None,
        }
    }

    fn chunk(code: PackIssueCode, section: &str, key: &str) -> Self {
        Self {
            code,
            section: Some(cut(section)),
            key: Some(cut(key)),
        }
    }

    fn named(code: PackIssueCode, name: &str) -> Self {
        Self {
            code,
            section: None,
            key: Some(cut(name)),
        }
    }
}

impl std::fmt::Display for PackIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code.as_str())?;
        match (&self.section, &self.key) {
            (Some(section), Some(key)) => write!(f, " ({section}/{key})"),
            (None, Some(key)) => write!(f, " ({key})"),
            _ => Ok(()),
        }
    }
}

fn cut(text: &str) -> String {
    let mut end = text.len().min(80);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].chars().filter(|c| !c.is_control()).collect()
}

fn plain_name(text: &str, limits: &Limits) -> bool {
    !text.is_empty() && text.len() <= limits.max_name_bytes && !text.chars().any(char::is_control)
}

struct Collector<'a> {
    issues: Vec<PackIssue>,
    limits: &'a Limits,
}

impl Collector<'_> {
    fn add(&mut self, issue: PackIssue) {
        if self.issues.len() < self.limits.max_issues && !self.issues.contains(&issue) {
            self.issues.push(issue);
        }
    }
}

/// The effective cost of a chunk in the budget's unit, when there is one: the host's own measure, or for a byte budget the
/// length of the text.
fn lower_bound(chunk: &Chunk, unit: BudgetUnit) -> Option<u64> {
    chunk.cost.or(match unit {
        BudgetUnit::BoundedBytes => Some(chunk.text.len() as u64),
        BudgetUnit::ModelTokens => None,
    })
}

/// Every reason the request cannot be packed, in the order they are found, at most `limits.max_issues`. An empty list is a
/// well-formed request, nothing more.
pub fn validate_request(request: &PackRequest, limits: &Limits) -> Vec<PackIssue> {
    let mut c = Collector {
        issues: Vec::new(),
        limits,
    };
    if request.protocol_version != PROTOCOL_VERSION {
        c.add(PackIssue::new(PackIssueCode::UnsupportedProtocol));
        return c.issues;
    }
    if !SUPPORTED_POLICIES.contains(&request.policy.as_str()) {
        c.add(PackIssue::new(PackIssueCode::UnsupportedPolicy));
    }
    if request.identity.is_empty()
        || request.identity.len() > limits.max_identity_bytes
        || request.identity.chars().any(char::is_control)
    {
        c.add(PackIssue::new(PackIssueCode::InvalidIdentity));
    }
    let binding =
        serde_json::to_vec(&request.request_binding).map_or(usize::MAX, |bytes| bytes.len());
    if binding > limits.max_binding_bytes {
        c.add(PackIssue::new(PackIssueCode::BindingTooLarge));
    }
    let budget = &request.budget;
    let ceiling = match budget.unit {
        BudgetUnit::ModelTokens => limits.max_model_tokens,
        BudgetUnit::BoundedBytes => limits.max_bounded_bytes,
    };
    if budget.limit == 0 || budget.limit > ceiling {
        c.add(PackIssue::new(PackIssueCode::BudgetOutOfRange));
    }
    if budget.scope.trim().is_empty()
        || budget.scope.len() > 512
        || budget.scope.chars().any(char::is_control)
    {
        c.add(PackIssue::new(PackIssueCode::InvalidBudgetScope));
    }
    if budget.unit == BudgetUnit::ModelTokens
        && budget
            .tokenizer
            .as_deref()
            .is_none_or(|name| !plain_name(name, limits) || name.len() > 64)
    {
        c.add(PackIssue::new(PackIssueCode::TokenizerMissing));
    }
    let chunk_count = request.required.len() + request.optional.len();
    if chunk_count > limits.max_chunks {
        c.add(PackIssue::new(PackIssueCode::TooManyChunks));
        return c.issues;
    }
    if request.groups.len() > limits.max_groups {
        c.add(PackIssue::new(PackIssueCode::TooManyGroups));
        return c.issues;
    }
    if request.pinned_entities.len() > limits.max_pinned_entities {
        c.add(PackIssue::new(PackIssueCode::TooManyPinnedEntities));
        return c.issues;
    }
    let mut total_bytes = 0usize;
    let mut seen: BTreeMap<(&str, &str), bool> = BTreeMap::new();
    let mut revisions: BTreeMap<(&str, &str), u64> = BTreeMap::new();
    let mut conflicted: BTreeSet<(&str, &str)> = BTreeSet::new();
    for entity in &request.pinned_entities {
        if !plain_name(&entity.kind, limits)
            || !plain_name(&entity.id, limits)
            || entity.revision == 0
        {
            c.add(PackIssue::new(PackIssueCode::InvalidEntity));
            continue;
        }
        note_revision(&mut c, &mut revisions, &mut conflicted, entity);
    }
    for (chunk, required) in request.chunks() {
        total_bytes = total_bytes.saturating_add(chunk.text.len());
        if !plain_name(&chunk.key, limits) || !plain_name(&chunk.section, limits) {
            c.add(PackIssue::chunk(
                PackIssueCode::InvalidName,
                &chunk.section,
                &chunk.key,
            ));
            continue;
        }
        if chunk.encoding != ENCODING_UTF8 {
            c.add(PackIssue::chunk(
                PackIssueCode::UnsupportedEncoding,
                &chunk.section,
                &chunk.key,
            ));
        }
        if !SUPPORTED_MEDIA.contains(&chunk.media.as_str()) {
            c.add(PackIssue::chunk(
                PackIssueCode::UnsupportedMedia,
                &chunk.section,
                &chunk.key,
            ));
        }
        if chunk.text.len() > limits.max_chunk_bytes {
            c.add(PackIssue::chunk(
                PackIssueCode::ChunkTooLarge,
                &chunk.section,
                &chunk.key,
            ));
        }
        if seen
            .insert((chunk.section.as_str(), chunk.key.as_str()), required)
            .is_some()
        {
            c.add(PackIssue::chunk(
                PackIssueCode::DuplicateChunk,
                &chunk.section,
                &chunk.key,
            ));
        }
        if chunk.entities.len() > limits.max_entities_per_chunk {
            c.add(PackIssue::chunk(
                PackIssueCode::TooManyEntities,
                &chunk.section,
                &chunk.key,
            ));
            continue;
        }
        for entity in &chunk.entities {
            if !plain_name(&entity.kind, limits)
                || !plain_name(&entity.id, limits)
                || entity.revision == 0
            {
                c.add(PackIssue::chunk(
                    PackIssueCode::InvalidEntity,
                    &chunk.section,
                    &chunk.key,
                ));
                continue;
            }
            note_revision(&mut c, &mut revisions, &mut conflicted, entity);
        }
    }
    if total_bytes > limits.max_total_bytes {
        c.add(PackIssue::new(PackIssueCode::InputTooLarge));
    }
    check_groups(&mut c, request, &seen);
    // A necessary condition, from what can be counted without the renderer: the required chunks alone are over the budget.
    let mandatory: u64 = request
        .required
        .iter()
        .filter_map(|chunk| lower_bound(chunk, budget.unit))
        .fold(0u64, u64::saturating_add);
    if budget.limit > 0 && mandatory > budget.limit {
        c.add(PackIssue::new(PackIssueCode::MandatoryOverflow));
    }
    c.issues
}

fn note_revision<'r>(
    c: &mut Collector<'_>,
    revisions: &mut BTreeMap<(&'r str, &'r str), u64>,
    conflicted: &mut BTreeSet<(&'r str, &'r str)>,
    entity: &'r Entity,
) {
    let key = (entity.kind.as_str(), entity.id.as_str());
    match revisions.get(&key) {
        None => {
            revisions.insert(key, entity.revision);
        }
        Some(earlier) if *earlier != entity.revision => {
            if conflicted.insert(key) {
                c.add(PackIssue::named(
                    PackIssueCode::EntityRevisionConflict,
                    &format!("{}:{}", entity.kind, entity.id),
                ));
            }
        }
        Some(_) => {}
    }
}

fn check_groups(c: &mut Collector<'_>, request: &PackRequest, seen: &BTreeMap<(&str, &str), bool>) {
    let mut group_ids = BTreeSet::new();
    let mut owner: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    for group in &request.groups {
        if !plain_name(&group.id, c.limits) {
            c.add(PackIssue::named(PackIssueCode::InvalidName, &group.id));
            continue;
        }
        if !group_ids.insert(group.id.as_str()) {
            c.add(PackIssue::named(PackIssueCode::DuplicateGroup, &group.id));
        }
        if group.members.is_empty() {
            c.add(PackIssue::named(PackIssueCode::GroupEmpty, &group.id));
            continue;
        }
        if group.members.len() > c.limits.max_group_members {
            c.add(PackIssue::named(PackIssueCode::GroupTooLarge, &group.id));
            continue;
        }
        let mut within = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        for member in &group.members {
            let key = (member.section.as_str(), member.key.as_str());
            if !within.insert(key) {
                c.add(PackIssue::chunk(
                    PackIssueCode::GroupMemberRepeated,
                    &member.section,
                    &member.key,
                ));
                continue;
            }
            match seen.get(&key) {
                None => c.add(PackIssue::chunk(
                    PackIssueCode::GroupMemberMissing,
                    &member.section,
                    &member.key,
                )),
                Some(required) => {
                    kinds.insert(*required);
                }
            }
            if let Some(first) = owner.insert(key, group.id.as_str()) {
                if first != group.id {
                    c.add(PackIssue::chunk(
                        PackIssueCode::GroupOverlap,
                        &member.section,
                        &member.key,
                    ));
                }
            }
        }
        if kinds.len() > 1 {
            c.add(PackIssue::named(
                PackIssueCode::GroupMixesRequiredAndOptional,
                &group.id,
            ));
        }
    }
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> =
                map.iter().map(|(k, v)| (k, canonical(v))).collect();
            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn canonical_text(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            let parts: Vec<String> = sorted
                .into_iter()
                .map(|(key, value)| {
                    format!("{}:{}", Value::String(key.clone()), canonical_text(value))
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_text)
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

/// A digest that names the request, whatever the order its JSON keys came in: the same request gives the same digest,
/// a changed chunk, group, budget or binding gives another.
pub fn request_digest(request: &PackRequest) -> String {
    let value = canonical(&serde_json::to_value(request).expect("a request serializes"));
    let mut hasher = Sha256::new();
    hasher.update(b"awr-context-kernel-request-v1\0");
    hasher.update(canonical_text(&value).as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn is_sha256_label(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Whether `result` is a consistent answer to `request`, whoever produced it. This is the oracle the packing step is held
/// to: it is written from the contract, not from any packing code. A request that is itself refused has no answer.
pub fn check_result(request: &PackRequest, result: &PackResult, limits: &Limits) -> Vec<PackIssue> {
    let mut c = Collector {
        issues: Vec::new(),
        limits,
    };
    if result.protocol_version != request.protocol_version || result.policy != request.policy {
        c.add(PackIssue::new(PackIssueCode::ResultProtocolMismatch));
    }
    if result.request_digest != request_digest(request) {
        c.add(PackIssue::new(PackIssueCode::ResultDigestMismatch));
    }
    if !is_sha256_label(&result.rendered_hash) {
        c.add(PackIssue::new(PackIssueCode::ResultHashMalformed));
    }
    if result.scope_note != SCOPE_NOTE {
        c.add(PackIssue::new(PackIssueCode::ResultScopeNote));
    }
    let mut known: BTreeMap<(&str, &str), bool> = BTreeMap::new();
    for (chunk, required) in request.chunks() {
        known.insert((chunk.section.as_str(), chunk.key.as_str()), required);
    }
    let mut state: BTreeMap<(&str, &str), (bool, Option<&str>)> = BTreeMap::new();
    for selection in &result.selected {
        let key = (selection.section.as_str(), selection.key.as_str());
        match known.get(&key) {
            None => c.add(PackIssue::chunk(
                PackIssueCode::ResultChunkUnknown,
                &selection.section,
                &selection.key,
            )),
            Some(required) => {
                if *required != selection.required {
                    c.add(PackIssue::chunk(
                        PackIssueCode::ResultRequiredFlagMismatch,
                        &selection.section,
                        &selection.key,
                    ));
                }
                if state
                    .insert(key, (true, selection.group.as_deref()))
                    .is_some()
                {
                    c.add(PackIssue::chunk(
                        PackIssueCode::ResultChunkTwice,
                        &selection.section,
                        &selection.key,
                    ));
                }
            }
        }
    }
    for omission in &result.omitted {
        let key = (omission.section.as_str(), omission.key.as_str());
        match known.get(&key) {
            None => c.add(PackIssue::chunk(
                PackIssueCode::ResultChunkUnknown,
                &omission.section,
                &omission.key,
            )),
            Some(required) => {
                if *required {
                    c.add(PackIssue::chunk(
                        PackIssueCode::ResultRequiredOmitted,
                        &omission.section,
                        &omission.key,
                    ));
                }
                if state
                    .insert(key, (false, omission.group.as_deref()))
                    .is_some()
                {
                    c.add(PackIssue::chunk(
                        PackIssueCode::ResultChunkTwice,
                        &omission.section,
                        &omission.key,
                    ));
                }
            }
        }
    }
    for (section, key) in known.keys() {
        if !state.contains_key(&(*section, *key)) {
            c.add(PackIssue::chunk(
                PackIssueCode::ResultChunkMissing,
                section,
                key,
            ));
        }
    }
    let mut group_of: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    for group in &request.groups {
        for member in &group.members {
            group_of.insert(
                (member.section.as_str(), member.key.as_str()),
                group.id.as_str(),
            );
        }
    }
    for (key, (_, said)) in &state {
        if said != &group_of.get(key).copied() {
            c.add(PackIssue::chunk(
                PackIssueCode::ResultGroupMismatch,
                key.0,
                key.1,
            ));
        }
    }
    for group in &request.groups {
        let verdicts: BTreeSet<bool> = group
            .members
            .iter()
            .filter_map(|member| {
                state
                    .get(&(member.section.as_str(), member.key.as_str()))
                    .map(|(selected, _)| *selected)
            })
            .collect();
        if verdicts.len() > 1 {
            c.add(PackIssue::named(PackIssueCode::ResultGroupSplit, &group.id));
        }
    }
    let m = &result.measurement;
    let budget = &request.budget;
    if m.unit != budget.unit
        || m.exactness != budget.exactness
        || m.scope != budget.scope
        || m.tokenizer != budget.tokenizer
        || m.limit != budget.limit
        || m.required_used > m.total_used
    {
        c.add(PackIssue::new(PackIssueCode::ResultMeasurementMismatch));
    }
    if m.total_used > budget.limit {
        c.add(PackIssue::new(PackIssueCode::ResultOverBudget));
    }
    c.issues
}
