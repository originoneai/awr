//! Bounded caller provenance for a Git baseline transition. These statements
//! do not verify Git ancestry, filesystem completeness, or target approval.
use super::*;
use awr_team::delivery::{RevisionFormat, RevisionRef};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    CallerAssertedGitBaselineV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    FastForwardOnly,
    MergeNoFf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BaselineTransition {
    kind: Kind,
    operation: Operation,
    workspace_before_revision: RevisionRef,
    imported_revision: RevisionRef,
    workspace_after_revision: RevisionRef,
    contribution_paths: Vec<String>,
}

impl BaselineTransition {
    pub(super) fn validate(&self) -> PgResult<()> {
        let revisions = [
            &self.workspace_before_revision,
            &self.imported_revision,
            &self.workspace_after_revision,
        ];
        if !matches!(self.kind, Kind::CallerAssertedGitBaselineV1)
            || !matches!(
                self.operation,
                Operation::FastForwardOnly | Operation::MergeNoFf
            )
            || revisions.iter().any(|revision| {
                revision.validate().is_err()
                    || !matches!(
                        &revision.format,
                        RevisionFormat::GitSha1 | RevisionFormat::GitSha256
                    )
            })
            || revisions.iter().skip(1).any(|revision| {
                revision.resource != revisions[0].resource || revision.format != revisions[0].format
            })
            || self.contribution_paths.len() > 128
            || self
                .contribution_paths
                .iter()
                .any(|path| !canonical_path(path))
            || self
                .contribution_paths
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.contribution_paths.len()
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Origin {
    CallerAsserted,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    ProvenanceOnly,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProvenanceDescriptor {
    origin: Origin,
    purpose: Purpose,
}

pub(super) fn provenance_descriptor() -> Value {
    json!({"origin":"caller_asserted","purpose":"provenance_only"})
}

/// Validate the full immutable caller statement and its exact server descriptor.
/// Both fields must be absent on a legacy receipt.
pub(in crate::workstream_command) fn validate_stored_receipt(
    payload: &Value,
) -> PgResult<Option<Value>> {
    match (
        payload.get("baseline_transition"),
        payload.get("baseline_transition_provenance"),
    ) {
        (None, None) => Ok(None),
        (Some(transition), Some(provenance)) => {
            let transition: BaselineTransition =
                serde_json::from_value(transition.clone()).map_err(|_| PgError::EvidenceInvalid)?;
            transition
                .validate()
                .map_err(|_| PgError::EvidenceInvalid)?;
            let provenance: ProvenanceDescriptor =
                serde_json::from_value(provenance.clone()).map_err(|_| PgError::EvidenceInvalid)?;
            if provenance
                != (ProvenanceDescriptor {
                    origin: Origin::CallerAsserted,
                    purpose: Purpose::ProvenanceOnly,
                })
            {
                return Err(PgError::EvidenceInvalid);
            }
            Ok(Some(provenance_descriptor()))
        }
        _ => Err(PgError::EvidenceInvalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transition() -> Value {
        json!({
            "kind":"caller_asserted_git_baseline_v1",
            "operation":"merge_no_ff",
            "workspace_before_revision":{
                "resource":"local-repository:stored-receipt-test",
                "format":"git_sha1",
                "value":"1".repeat(40)
            },
            "imported_revision":{
                "resource":"local-repository:stored-receipt-test",
                "format":"git_sha1",
                "value":"2".repeat(40)
            },
            "workspace_after_revision":{
                "resource":"local-repository:stored-receipt-test",
                "format":"git_sha1",
                "value":"3".repeat(40)
            },
            "contribution_paths":["src/api/result.rs"]
        })
    }

    fn descriptor() -> Value {
        json!({"origin":"caller_asserted","purpose":"provenance_only"})
    }

    fn assert_invalid(payload: Value) {
        assert!(matches!(
            validate_stored_receipt(&payload),
            Err(PgError::EvidenceInvalid)
        ));
    }

    #[test]
    fn stored_receipt_accepts_legacy_absence_and_exact_typed_metadata() {
        assert_eq!(validate_stored_receipt(&json!({})).unwrap(), None);
        assert_eq!(
            validate_stored_receipt(&json!({
                "baseline_transition":transition(),
                "baseline_transition_provenance":descriptor()
            }))
            .unwrap(),
            Some(descriptor())
        );
    }

    #[test]
    fn stored_receipt_rejects_null_one_sided_and_incomplete_metadata() {
        for payload in [
            json!({
                "baseline_transition":null,
                "baseline_transition_provenance":descriptor()
            }),
            json!({
                "baseline_transition":transition(),
                "baseline_transition_provenance":null
            }),
            json!({"baseline_transition":transition()}),
            json!({"baseline_transition_provenance":descriptor()}),
        ] {
            assert_invalid(payload);
        }

        let mut incomplete = transition();
        incomplete.as_object_mut().unwrap().remove("operation");
        assert_invalid(json!({
            "baseline_transition":incomplete,
            "baseline_transition_provenance":descriptor()
        }));
    }

    #[test]
    fn stored_receipt_rejects_forged_or_open_provenance_descriptors() {
        for provenance in [
            json!({"origin":"adapter_verified","purpose":"provenance_only"}),
            json!({"origin":"caller_asserted","purpose":"scope_authority"}),
            json!({
                "origin":"caller_asserted",
                "purpose":"provenance_only",
                "verified":true
            }),
        ] {
            assert_invalid(json!({
                "baseline_transition":transition(),
                "baseline_transition_provenance":provenance
            }));
        }
    }
}
