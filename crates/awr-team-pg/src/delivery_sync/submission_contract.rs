//! Lazy, descriptive input discovery. Material facts and authority are never invented.
use crate::{PgError, PgResult};
use awr_core::Id;
use awr_team::delivery::{
    DeliveryCandidate, SOURCE_FETCH_REF_FORBIDDEN_PATTERNS, SOURCE_FETCH_REF_PATTERN,
};
use serde_json::{Value, json};

pub(super) fn context(
    tenant: &str,
    project: &str,
    workstream: Id,
    work: &str,
    contract: &str,
    checks: Value,
) -> Value {
    json!({"tenant_id":tenant,"project_id":project,"scope_id":"main",
        "workstream_id":workstream.to_string(),"work_id":work,
        "contract_hash":contract,"required_checks":checks,
        "candidate_command":"delivery.candidate.select","artifact_command":"delivery.evidence.submit",
        "review_command":"delivery.submit_and_request_review",
        "material_facts":"Observe the actual published source revision, target precondition and each manifest entry's bytes; never infer them from a progress summary.",
        "manifest_hash_codec":{"algorithm":"sha256","digest_encoding":"lowercase hexadecimal",
            "json":"UTF-8 compact JSON; recursively sort object keys, preserve array order, emit non-ASCII characters directly",
            "manifest_pointer":"/fields/manifest",
            "instruction":"Replace envelope.fields.manifest with the actual manifest object and hash the complete envelope once; do not add another wrapper.",
            "envelope":{"codec":awr_team::HASH_CODEC,"kind":"request",
                "fields":{"codec":"awr-delivery-manifest-v1","manifest":"the actual manifest object"}}}})
}

fn text(limit: usize) -> Value {
    json!({"type":"string","minLength":1,"maxLength":limit,
        "description":"Nonblank text without control characters; the domain enforces the UTF-8 byte limit."})
}

fn version(positive: bool) -> Value {
    json!({"type":"string","maxLength":20,
        "pattern":if positive {"^[1-9][0-9]*$"} else {"^(0|[1-9][0-9]*)$"},
        "description":"Canonical decimal u64 string, not a JSON number."})
}

fn digest() -> Value {
    json!({"type":"string","pattern":"^[0-9a-f]{64}$"})
}

fn revision() -> Value {
    json!({"type":"object","additionalProperties":false,
        "required":["resource","format","value"],"properties":{
            "resource":text(1024),"format":{"enum":["git_sha1","git_sha256","artifact"]},
            "value":text(128)},
        "allOf":[
            {"if":{"properties":{"format":{"const":"git_sha1"}}},
                "then":{"properties":{"value":{"pattern":"^[0-9a-f]{40}$"}}}},
            {"if":{"properties":{"format":{"const":"git_sha256"}}},
                "then":{"properties":{"value":{"pattern":"^[0-9a-f]{64}$"}}}}]})
}

fn source_fetch_ref() -> Value {
    let exclusions = SOURCE_FETCH_REF_FORBIDDEN_PATTERNS
        .iter()
        .map(|pattern| json!({"not":{"pattern":pattern}}))
        .collect::<Vec<_>>();
    json!({"type":"string","maxLength":1024,"pattern":SOURCE_FETCH_REF_PATTERN,
        "allOf":exclusions,
        "description":"Optional ordinary advertised Git ref using only portable ASCII letters, digits, '.', '_', '/', '-', '+', and '@'. Other Git-legal names are outside this contract. The ref is mutable and only locates an object; pass it as one process argument without a shell, fetch it exactly, and verify the resulting commit against source_revision."})
}

fn candidate_schema() -> Value {
    let revision = revision();
    json!({"type":"object","additionalProperties":false,"required":["binding","manifest"],
        "properties":{
            "binding":{"type":"object","additionalProperties":false,
                "required":["tenant_id","project_id","scope_id","workstream_id","work_id",
                    "candidate_id","candidate_version","contract_hash","manifest_digest","required_checks","target"],
                "properties":{
                    "tenant_id":text(128),"project_id":text(128),"scope_id":{"const":"main"},
                    "workstream_id":text(128),"work_id":text(128),"candidate_id":text(128),
                    "candidate_version":version(true),"contract_hash":digest(),"manifest_digest":digest(),
                    "source_revision":{"anyOf":[revision,{"type":"null"}]},
                    "source_fetch_ref":{"anyOf":[source_fetch_ref(),{"type":"null"}]},
                    "required_checks":{"type":"array","maxItems":64,"uniqueItems":true,"items":text(128)},
                    "target":{"type":"object","additionalProperties":false,"required":["resource","precondition"],
                        "properties":{"resource":text(1024),"reference":{"anyOf":[text(1024),{"type":"null"}]},
                            "precondition":{"oneOf":[
                                {"type":"object","additionalProperties":false,"required":["kind"],
                                    "properties":{"kind":{"const":"missing"}}},
                                {"type":"object","additionalProperties":false,"required":["kind","revision"],
                                    "properties":{"kind":{"const":"exact"},"revision":revision}}]}}}},
                "allOf":[
                    {"if":{"required":["source_fetch_ref"],
                        "properties":{"source_fetch_ref":{"type":"string"}}},
                     "then":{"required":["source_revision"],
                        "properties":{"source_revision":{"type":"object","required":["format"],
                            "properties":{"format":{"enum":["git_sha1","git_sha256"]}}}}}}]},
            "manifest":{"type":"object","additionalProperties":false,"required":["entries"],
                "properties":{"entries":{"type":"array","minItems":1,"maxItems":128,
                    "description":"Each artifact_id must be unique. Hash and length refer to the actual immutable bytes.",
                    "items":{"type":"object","additionalProperties":false,
                        "required":["artifact_id","sha256","byte_length","locator"],
                        "properties":{"artifact_id":text(128),"sha256":digest(),
                            "byte_length":version(false),"locator":text(4096)}}}}}}})
}

pub(super) fn describe(mut inspection: Value) -> PgResult<Value> {
    let context = inspection["submission"]["candidate_context"].take();
    if !context.is_object() {
        return Err(PgError::SourceDivergence);
    }
    let binding_values = json!({
        "tenant_id":context["tenant_id"],"project_id":context["project_id"],
        "scope_id":context["scope_id"],"workstream_id":context["workstream_id"],
        "work_id":context["work_id"],"contract_hash":context["contract_hash"],
        "required_checks":context["required_checks"]});
    let selection = if inspection["candidate"].is_null() {
        Value::Null
    } else {
        let candidate: DeliveryCandidate = serde_json::from_value(inspection["candidate"].clone())
            .map_err(|_| PgError::SourceDivergence)?;
        json!({"binding_digest":candidate.binding.digest().map_err(|_| PgError::SourceDivergence)?,
            "selection_version":inspection["selection_version"],
            "current":inspection["selected_current"]})
    };
    let mut result = json!({
        "contract":"awr-delivery-submission-v1","work_id":inspection["work_id"],
        "workstream_id":inspection["workstream_id"],"source_snapshot_id":inspection["source_snapshot_id"],
        "coordinator_epoch":inspection["coordinator_epoch"],"project_revision":inspection["project_revision"],
        "binding_values":binding_values,"stored_selection":selection,
        "connectors":inspection["connectors"],"connectors_truncated":inspection["connectors_truncated"],
        "command":{"op":"delivery.candidate.select",
            "header_from":"Fresh work.prepare: protocol_version=1, a stable request_id, workstream_id, work_id, coordinator_epoch, expected_project_revision=project_revision, expected_authority_version=authority_version, expected_ownership_version=data.ownership_version, expected_contract_hash=data.contract_hash.",
            "args_schema":{"type":"object","additionalProperties":false,
                "required":["source_snapshot_id","session_id","claim_id","fence","lease_version","candidate"],
                "properties":{"source_snapshot_id":text(128),"session_id":text(128),"claim_id":text(128),
                    "fence":version(false),"lease_version":version(false),
                    "expected_selected_digest":{"anyOf":[digest(),{"type":"null"}]},
                    "candidate":candidate_schema()}},
            "runtime_inputs":"Use your actual active session and claim.inspect's current claim_id, fence and lease_version. args.source_snapshot_id comes from fresh work.prepare. expected_selected_digest is the stored selection binding_digest, or null when none. Do not send read_set or request_id inside args; the transport derives them."},
        "material_inputs":{
            "candidate_identity":"Choose a stable candidate_id and a positive decimal candidate_version for this immutable submission; never silently reuse its identity for changed facts.",
            "manifest":"Measure every actual artifact's SHA-256 and decimal byte_length; use its inspectable locator. Compute binding.manifest_digest over the full manifest using manifest_hash_codec.",
            "source_revision":"Observe the actual published immutable revision using its opaque connector resource and git_sha1, git_sha256 or artifact format. Null is only a structural option; the configured adapter may require an exact revision.",
            "source_fetch_ref":"For a Git source revision, optionally declare the full ordinary advertised refs/heads/... or refs/tags/... ref from which that exact commit can be fetched. Only the documented portable ASCII subset is accepted; other Git-legal names are outside this contract. Pass the ref as one argv value without shell interpolation or eval. This ref is candidate-bound but mutable; it is not target.reference and never replaces verification of source_revision after fetch.",
            "target":"Use the authorized connector resource and project target reference. Observe its exact current revision, or confirmed absence; errors and inaccessible targets never mean missing. An exact precondition's revision.resource must equal target.resource."},
        "manifest_hash_codec":context["manifest_hash_codec"],
        "evidence":{"command":"delivery.evidence.submit",
            "required_args":["session_id","expected_session_version","delivery_payload","dirty_tree"],
            "policy_required_args":{"execution_id":"Required for every simulated-member evidence item; obtain the actual execution ID from work.observe."},
            "candidate_binding":"Set args.delivery_payload.delivery_candidate_digest to data.candidate_digest in the successful selection receipt (receipt.data.candidate_digest over MCP/HTTP). delivery_payload is a required JSON object; optional passed (boolean) and output_digest (64 lowercase hex) share the same object and other keys are preserved unchanged.",
            "artifacts":"Submit each manifest entry's exact bytes separately with artifact_text (UTF-8) or artifact_hex in delivery.evidence.submit. The server assigns stored artifact IDs and measures digests; the manifest's logical IDs do not replace them. Measured SHA-256 and length must match. A local path or report alone is insufficient.",
            "next_query":{"protocol_version":1,"op":"delivery.neutral.inspect","work_id":inspection["work_id"]},
            "review_command":"delivery.submit_and_request_review",
            "review_required_args":["session_id","expected_session_version","evidence_id"],
            "review_binding":"Use the primary report's submitted evidence_id; do not reuse the evidence submission payload. Refresh the session version before opening review."},
        "guidance":{"when":"Preparing a version-bound submission for neutral delivery",
            "because":"The input contract describes structure; only authorized project publication and actual bytes establish material facts",
            "action":"Publish through the shared channel and target disclosed in this project's working agreement, then select the measured candidate and submit readable bound evidence. If the channel or target is unavailable, record the missing prerequisite for the supervisor; do not guess or search server configuration, peer workspaces or implementation.",
            "recheck_on":"Source, contract, ownership, selection, claim, authority or publication changes; refresh work.prepare and claim.inspect before any command"},
        "read_only":true,"execution_authorized":false,"acceptance_ready":false,"source_synchronized":false});
    if inspection["connectors"]
        .as_array()
        .is_some_and(|connectors| {
            connectors.iter().any(|connector| {
                connector["provider"] == "local_git"
                    && connector["enabled"] == true
                    && connector["current_epoch"] == true
            })
        })
    {
        result["local_git"] = json!({
            "when":"The authorized shared project channel is a Git remote observed by a local_git connector",
            "action":"Use this repository's existing authorized remote to publish an immutable development branch, not the protected integration target. Read source revision and file bytes from that commit; use git-blob:relative/path locators. Observe the actual project target reference through that same authorized remote.",
            "basis":"local_git.manifest verifies committed bytes; it does not run tests, approve work or grant repository write permission",
            "recheck_on":"Published commit, authorized remote, target revision or connector configuration changes"});
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct SourceFetchRefCorpus {
        codec: String,
        cases: Vec<SourceFetchRefCase>,
    }

    #[derive(Deserialize)]
    struct SourceFetchRefCase {
        name: String,
        value: String,
        accepted: bool,
    }

    fn source_fetch_ref_corpus() -> SourceFetchRefCorpus {
        serde_json::from_str(include_str!(
            "../../../awr-team/tests/fixtures/source-fetch-refs-v1.json"
        ))
        .unwrap()
    }

    fn source_condition_allows(binding: &Value, condition: &Value) -> bool {
        let Some(binding) = binding.as_object() else {
            return false;
        };
        let applies = binding
            .get("source_fetch_ref")
            .is_some_and(Value::is_string);
        if !applies {
            return true;
        }
        let Some(format) = binding
            .get("source_revision")
            .and_then(Value::as_object)
            .and_then(|revision| revision.get("format"))
        else {
            return false;
        };
        condition["then"]["properties"]["source_revision"]["properties"]["format"]["enum"]
            .as_array()
            .unwrap()
            .contains(format)
    }

    #[test]
    fn submission_description_exposes_evidence_and_review_inputs() {
        let workstream = Id::new();
        let inspection = json!({
            "work_id":"work", "workstream_id":workstream.to_string(),
            "source_snapshot_id":"snapshot", "coordinator_epoch":"epoch",
            "project_revision":"1", "candidate":null, "connectors":[],
            "connectors_truncated":false,
            "submission":{"candidate_context":context("tenant", "project", workstream, "work", &"a".repeat(64), json!([]))}
        });
        let result = describe(inspection).unwrap();
        assert_eq!(
            result["evidence"]["required_args"],
            json!([
                "session_id",
                "expected_session_version",
                "delivery_payload",
                "dirty_tree"
            ])
        );
        assert!(
            result["evidence"]["policy_required_args"]["execution_id"]
                .as_str()
                .unwrap()
                .contains("simulated-member")
        );
        assert_eq!(
            result["evidence"]["review_required_args"],
            json!(["session_id", "expected_session_version", "evidence_id"])
        );
        assert_eq!(result["evidence"]["next_query"]["work_id"], "work");
        assert_eq!(result["execution_authorized"], false);
    }

    #[test]
    fn source_fetch_ref_schema_matches_the_domain_corpus_and_git_revision_condition() {
        let candidate = candidate_schema();
        let binding = &candidate["properties"]["binding"];
        let reference = &binding["properties"]["source_fetch_ref"]["anyOf"][0];
        assert_eq!(reference["pattern"], SOURCE_FETCH_REF_PATTERN);
        assert_eq!(reference["maxLength"], 1024);
        let shape = Regex::new(reference["pattern"].as_str().unwrap()).unwrap();
        let exclusions = reference["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| Regex::new(rule["not"]["pattern"].as_str().unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            reference["allOf"],
            json!(
                SOURCE_FETCH_REF_FORBIDDEN_PATTERNS
                    .iter()
                    .map(|pattern| json!({"not":{"pattern":pattern}}))
                    .collect::<Vec<_>>()
            )
        );
        let corpus = source_fetch_ref_corpus();
        assert_eq!(corpus.codec, "awr-source-fetch-ref-corpus-v1");
        for case in corpus.cases {
            let accepted = shape.is_match(&case.value)
                && exclusions
                    .iter()
                    .all(|pattern| !pattern.is_match(&case.value));
            assert_eq!(
                accepted, case.accepted,
                "source fetch ref schema mismatch for {} ({:?})",
                case.name, case.value
            );
        }

        let condition = &binding["allOf"][0];
        assert_eq!(condition["if"]["required"], json!(["source_fetch_ref"]));
        assert_eq!(
            condition["if"]["properties"]["source_fetch_ref"]["type"],
            "string"
        );
        assert_eq!(condition["then"]["required"], json!(["source_revision"]));
        for (value, accepted) in [
            (json!({}), true),
            (json!({"source_fetch_ref":null}), true),
            (json!({"source_fetch_ref":"refs/heads/candidate"}), false),
            (
                json!({"source_fetch_ref":"refs/heads/candidate","source_revision":null}),
                false,
            ),
            (
                json!({"source_fetch_ref":"refs/tags/v1","source_revision":{"format":"artifact"}}),
                false,
            ),
            (
                json!({"source_fetch_ref":"refs/heads/candidate","source_revision":{"format":"git_sha1"}}),
                true,
            ),
            (
                json!({"source_fetch_ref":"refs/heads/candidate","source_revision":{"format":"git_sha256"}}),
                true,
            ),
        ] {
            assert_eq!(
                source_condition_allows(&value, condition),
                accepted,
                "{value}"
            );
        }
    }
}
