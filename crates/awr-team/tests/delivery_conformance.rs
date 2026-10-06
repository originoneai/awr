use awr_team::delivery::*;
use awr_team::{ProjectId, RequestId, ScopeId, TenantId, WorkId};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    codec: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    format: Option<RevisionFormat>,
    value: Option<String>,
    pr: bool,
    change: Option<String>,
    reference: String,
    legacy: String,
}

fn candidate(case: &Case) -> DeliveryCandidate {
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: "source".into(),
            sha256: "d".repeat(64),
            byte_length: "42".into(),
            locator: "artifact:example/source".into(),
        }],
    };
    DeliveryCandidate {
        binding: CandidateBinding {
            tenant_id: TenantId::new("example-tenant").unwrap(),
            project_id: ProjectId::new("example-project").unwrap(),
            scope_id: ScopeId::new("main").unwrap(),
            workstream_id: "example-stream".into(),
            work_id: WorkId::new("example-work").unwrap(),
            candidate_id: RequestId::new("example-candidate").unwrap(),
            candidate_version: "1".into(),
            contract_hash: "c".repeat(64),
            manifest_digest: manifest.digest().unwrap(),
            source_revision: case.format.clone().map(|format| RevisionRef {
                resource: "example/repository".into(),
                format,
                value: case.value.clone().unwrap(),
            }),
            required_checks: vec!["conformance".into()],
            target: DeliveryTarget {
                resource: "example/target".into(),
                reference: Some("main".into()),
                precondition: TargetPrecondition::Missing,
            },
        },
        manifest,
    }
}

fn snapshot() -> LegacyPrSnapshot {
    LegacyPrSnapshot {
        delivery_id: "example-delivery".into(),
        repository: "example/repository".into(),
        pr_number: 42,
        pr_url: "https://github.com/example/repository/pull/42".into(),
        head_sha: "a".repeat(40),
        merge_sha: Some("b".repeat(40)),
        submitted: true,
        approved: true,
        merged: true,
        fact_source: "authorized_human_github_verification".into(),
        observed_at: "2026-01-01T12:00:00+08:00".into(),
        contract_hash: "c".repeat(64),
        state: "active".into(),
        test_evidence_id: Some("legacy-evidence".into()),
    }
}

fn provenance() -> FactProvenance {
    FactProvenance {
        source: FactSource::CallerDeclared,
        reference: "observation:example".into(),
        observed_at_unix_ms: None,
        recorded_at_unix_ms: 1_800_000_000_000,
    }
}

fn wire(record: DeliveryRecord) -> DeliveryEnvelope {
    parse_delivery_record(
        &serde_json::to_vec(&DeliveryEnvelope {
            protocol: DELIVERY_PROTOCOL.into(),
            protocol_version: DELIVERY_PROTOCOL_VERSION,
            record,
        })
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn shared_public_corpus_preserves_candidate_meaning_across_two_boundaries() {
    let corpus: Corpus =
        serde_json::from_str(include_str!("fixtures/delivery-facts-v1.json")).unwrap();
    assert_eq!(corpus.codec, "awr-delivery-conformance-v1");
    assert_eq!(corpus.cases.len(), 10);
    for case in corpus.cases {
        let candidate = candidate(&case);
        let mut current = candidate.binding.clone();
        match case.change.as_deref() {
            Some("version") => current.candidate_version = "2".into(),
            Some("target") => current.target.reference = Some("release".into()),
            Some("source") => current.source_revision.as_mut().unwrap().value = "f".repeat(40),
            Some("scope") => current.scope_id = ScopeId::new("other").unwrap(),
            Some("checks") => current.required_checks.push("additional".into()),
            None => (),
            other => panic!("unknown corpus mutation: {other:?}"),
        }
        let reference = wire(DeliveryRecord::Candidate(candidate.clone()));
        assert_eq!(
            reference.record.validate_against(&current).is_ok(),
            case.reference == "accepted",
            "{}: reference boundary",
            case.name
        );

        // The historical hosted shape is a real separate JSON boundary. It
        // cannot express SHA-256, or manufacture a PR for artifact-only work.
        if !case.pr {
            let hosted: Option<LegacyPrSnapshot> = serde_json::from_str("null").unwrap();
            assert!(hosted.is_none());
            assert_eq!(case.legacy, "absent");
            continue;
        }
        let mut hosted = snapshot();
        hosted.head_sha = case.value.clone().unwrap();
        let hosted: LegacyPrSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&hosted).unwrap()).unwrap();
        let observation = hosted.incomplete_observation();
        assert!(!observation.acceptance_ready);
        assert!(
            observation
                .missing_facts
                .contains(&MissingDeliveryFact::AwrReviewDecision)
        );
        assert!(
            observation
                .missing_facts
                .contains(&MissingDeliveryFact::IntegrationContentProof)
        );
        let normalized = hosted.change_request(&candidate, &current, provenance());
        match case.legacy.as_str() {
            "accepted" => {
                let normalized = normalized.unwrap();
                assert_eq!(Some(&normalized.binding), reference.record.binding());
                assert_eq!(normalized.provenance, provenance());
                assert_eq!(normalized.provenance.observed_at_unix_ms, None);
                DeliveryRecord::ChangeRequest(normalized)
                    .validate_against(&current)
                    .unwrap();
            }
            "rejected" => assert!(normalized.is_err(), "{}", case.name),
            "unsupported" => {
                assert!(normalized.is_err());
                assert_eq!(observation.reported_source_revision, None);
                assert_eq!(
                    observation.issues,
                    vec![LegacyDeliveryIssue::InvalidSourceRevision]
                );
            }
            other => panic!("unknown expected legacy outcome: {other}"),
        }

        // Identical explicit unknown facts survive both shapes. Approved/merged
        // flags do not supply these records, observation times or content proof.
        if case.reference == "accepted" && case.legacy == "accepted" {
            let integration = IntegrationObservation {
                binding: candidate.binding.clone(),
                request_id: None,
                external_reference: "integration:example".into(),
                outcome: IntegrationOutcome::Unknown,
                result_revision: None,
                contains_manifest_digest: None,
                provenance: provenance(),
            };
            let local = wire(DeliveryRecord::IntegrationObservation(integration.clone()));
            let hosted: IntegrationObservation =
                serde_json::from_value(serde_json::to_value(&integration).unwrap()).unwrap();
            assert_eq!(local.record, DeliveryRecord::IntegrationObservation(hosted));
            let verification = VerificationRun {
                binding: candidate.binding.clone(),
                run_id: "example-run".into(),
                check: "conformance".into(),
                outcome: VerificationOutcome::Unknown,
                result_artifact: None,
                provenance: provenance(),
            };
            assert_eq!(
                wire(DeliveryRecord::Verification(verification.clone())).record,
                DeliveryRecord::Verification(verification)
            );
        }
    }
}

#[test]
fn old_names_flags_and_evidence_never_fill_missing_facts() {
    let mut legacy = snapshot();
    legacy.fact_source = "human_trusted_adapter".into();
    legacy.submitted = false;
    legacy.merge_sha = None;
    let observed = legacy.incomplete_observation();
    assert_eq!(observed.fact_source, "human_trusted_adapter");
    assert_eq!(observed.observed_at, legacy.observed_at);
    assert!(!observed.external_status.submitted);
    assert!(observed.external_status.approved && observed.external_status.merged);
    assert!(observed.reported_merge_revision.is_none());
    assert!(!observed.acceptance_ready);
    assert_eq!(observed.missing_facts.len(), 9);
    assert!(observed.issues.is_empty());
    let value = serde_json::to_value(observed).unwrap();
    assert!(value.get("pr_url").is_none());
    assert!(value.get("test_evidence_id").is_none());
    assert!(parse_delivery_record(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn malformed_legacy_data_stays_readable_without_echoing_values_in_diagnostics() {
    let mut legacy = snapshot();
    legacy.head_sha = "sensitive-looking-test-value".into();
    legacy.merge_sha = Some("unsupported-revision".into());
    let observed = legacy.incomplete_observation();
    assert!(observed.reported_source_revision.is_none());
    assert!(observed.reported_merge_revision.is_none());
    assert!(
        observed
            .missing_facts
            .contains(&MissingDeliveryFact::SourceRevision)
    );
    let issues = serde_json::to_string(&observed.issues).unwrap();
    assert_eq!(
        issues,
        "[\"invalid_source_revision\",\"invalid_merge_revision\"]"
    );
    assert!(!issues.contains(&legacy.head_sha));
    let raw = serde_json::to_value(&legacy).unwrap();
    assert_eq!(raw["head_sha"], legacy.head_sha);
    assert_eq!(
        serde_json::from_value::<LegacyPrSnapshot>(raw).unwrap(),
        legacy
    );
}

#[test]
fn explicit_candidate_conversion_rejects_stale_source_and_invalid_bound_facts() {
    let corpus: Corpus =
        serde_json::from_str(include_str!("fixtures/delivery-facts-v1.json")).unwrap();
    let mut candidate = candidate(&corpus.cases[0]);
    let current = candidate.binding.clone();
    let mut legacy = snapshot();
    legacy.head_sha = "f".repeat(40);
    let error = legacy
        .change_request(&candidate, &current, provenance())
        .unwrap_err();
    assert!(!error.to_string().contains(&legacy.head_sha));
    legacy = snapshot();
    legacy.contract_hash = "f".repeat(64);
    assert!(
        legacy
            .change_request(&candidate, &current, provenance())
            .is_err()
    );
    legacy = snapshot();
    legacy.state = "invalidated".into();
    assert!(
        legacy
            .change_request(&candidate, &current, provenance())
            .is_err()
    );
    legacy = snapshot();
    candidate.manifest.entries[0].sha256 = "f".repeat(64);
    assert!(
        legacy
            .change_request(&candidate, &current, provenance())
            .is_err()
    );
}

#[test]
fn explicit_provenance_and_strict_legacy_fields_are_required() {
    let corpus: Corpus =
        serde_json::from_str(include_str!("fixtures/delivery-facts-v1.json")).unwrap();
    let candidate = candidate(&corpus.cases[0]);
    let mut invalid = provenance();
    invalid.recorded_at_unix_ms = 0;
    assert!(
        snapshot()
            .change_request(&candidate, &candidate.binding, invalid)
            .is_err()
    );
    let mut value: Value = serde_json::to_value(snapshot()).unwrap();
    value["human_approval"] = true.into();
    assert!(serde_json::from_value::<LegacyPrSnapshot>(value).is_err());
}
