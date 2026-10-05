use awr_team::delivery::*;
use awr_team::{ActorId, ProjectId, RequestId, TeamError, TenantId, WorkId};
use serde_json::{Value, json};

fn artifact() -> ArtifactEntry {
    ArtifactEntry {
        artifact_id: "public-source".into(),
        sha256: "a".repeat(64),
        byte_length: "42".into(),
        locator: "artifact:public-source".into(),
    }
}

fn candidate() -> DeliveryCandidate {
    let manifest = ArtifactManifest {
        entries: vec![artifact()],
    };
    DeliveryCandidate {
        binding: CandidateBinding {
            tenant_id: TenantId::new("example-tenant").unwrap(),
            project_id: ProjectId::new("example-project").unwrap(),
            work_id: WorkId::new("example-work").unwrap(),
            candidate_id: RequestId::new("example-candidate").unwrap(),
            candidate_version: "1".into(),
            contract_hash: "b".repeat(64),
            manifest_digest: manifest.digest().unwrap(),
            source_revision: Some(RevisionRef {
                resource: "repository:example".into(),
                format: RevisionFormat::GitSha256,
                value: "c".repeat(64),
            }),
            required_checks: vec!["conformance".into()],
            target: DeliveryTarget {
                resource: "repository:example".into(),
                reference: Some("refs/heads/main".into()),
                precondition: TargetPrecondition::Exact(RevisionRef {
                    resource: "repository:example".into(),
                    format: RevisionFormat::GitSha256,
                    value: "d".repeat(64),
                }),
            },
        },
        manifest,
    }
}

fn provenance() -> FactProvenance {
    FactProvenance {
        source: FactSource::CallerDeclared,
        reference: "report:example".into(),
        observed_at_unix_ms: Some(1000),
        recorded_at_unix_ms: 2000,
    }
}

fn envelope(record: DeliveryRecord) -> DeliveryEnvelope {
    DeliveryEnvelope {
        protocol: DELIVERY_PROTOCOL.into(),
        protocol_version: DELIVERY_PROTOCOL_VERSION,
        record,
    }
}

fn records() -> Vec<DeliveryRecord> {
    let candidate = candidate();
    let binding = candidate.binding.clone();
    vec![
        DeliveryRecord::Candidate(candidate),
        DeliveryRecord::ChangeRequest(ChangeRequest {
            binding: binding.clone(),
            provider: "example-forge".into(),
            resource_id: "change:opaque-id".into(),
            locator: None,
            provenance: provenance(),
        }),
        DeliveryRecord::Verification(VerificationRun {
            binding: binding.clone(),
            run_id: "example-run".into(),
            check: "conformance".into(),
            outcome: VerificationOutcome::Passed,
            result_artifact: Some(artifact()),
            provenance: provenance(),
        }),
        DeliveryRecord::ReviewDecision(ReviewDecision {
            binding: binding.clone(),
            round_id: "example-round".into(),
            decision_id: "example-decision".into(),
            evidence_bundle_digest: "e".repeat(64),
            reviewer_actor_id: ActorId::new("example-reviewer").unwrap(),
            outcome: ReviewOutcome::Approved,
        }),
        DeliveryRecord::IntegrationRequest(IntegrationRequest {
            binding: binding.clone(),
            request_id: RequestId::new("example-integration").unwrap(),
            operation: IntegrationOperation::Merge,
            review_round_id: "example-round".into(),
            review_decision_id: "example-decision".into(),
            verified_checks: vec![VerificationRef {
                check: "conformance".into(),
                run_id: "example-run".into(),
            }],
        }),
        DeliveryRecord::IntegrationObservation(IntegrationObservation {
            binding: binding.clone(),
            request_id: Some(RequestId::new("example-integration").unwrap()),
            external_reference: "result:example".into(),
            outcome: IntegrationOutcome::Applied,
            result_revision: Some(RevisionRef {
                resource: binding.target.resource.clone(),
                format: RevisionFormat::GitSha256,
                value: "f".repeat(64),
            }),
            contains_manifest_digest: Some(binding.manifest_digest.clone()),
            provenance: provenance(),
        }),
        DeliveryRecord::AdapterCapabilities(AdapterCapabilities {
            adapter_id: "example-reference".into(),
            inspection: true,
            change_requests: false,
            verification: true,
            integration_requests: true,
            integration_observations: true,
            notifications: false,
            polling: true,
        }),
    ]
}

#[test]
fn neutral_records_round_trip_without_a_github_url_or_pr_number() {
    for record in records() {
        let envelope = envelope(record);
        envelope.validate().unwrap();
        let bytes = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(parse_delivery_record(&bytes).unwrap(), envelope);
        assert!(!String::from_utf8(bytes).unwrap().contains("github.com"));
    }
}

#[test]
fn artifact_only_delivery_needs_neither_a_repository_nor_change_request() {
    let mut candidate = candidate();
    candidate.binding.source_revision = None;
    candidate.binding.required_checks.clear();
    candidate.binding.target = DeliveryTarget {
        resource: "artifact-collection:example".into(),
        reference: None,
        precondition: TargetPrecondition::Missing,
    };
    DeliveryRecord::Candidate(candidate.clone())
        .validate()
        .unwrap();
    let request = IntegrationRequest {
        binding: candidate.binding.clone(),
        request_id: RequestId::new("artifact-apply").unwrap(),
        operation: IntegrationOperation::ApplyArtifacts,
        review_round_id: "round".into(),
        review_decision_id: "decision".into(),
        verified_checks: vec![],
    };
    DeliveryRecord::IntegrationRequest(request.clone())
        .validate()
        .unwrap();
    let git_request = IntegrationRequest {
        operation: IntegrationOperation::Merge,
        ..request
    };
    assert!(
        DeliveryRecord::IntegrationRequest(git_request)
            .validate()
            .is_err()
    );
}

#[test]
fn revision_formats_are_explicit_and_sha256_git_is_not_forced_to_sha1() {
    for (format, value) in [
        (RevisionFormat::GitSha1, "a".repeat(40)),
        (RevisionFormat::GitSha256, "b".repeat(64)),
        (RevisionFormat::Artifact, "package-release-v2".into()),
    ] {
        RevisionRef {
            resource: "resource".into(),
            format,
            value,
        }
        .validate()
        .unwrap();
    }
    for (format, value) in [
        (RevisionFormat::GitSha1, "a".repeat(64)),
        (RevisionFormat::GitSha256, "b".repeat(40)),
        (RevisionFormat::GitSha256, "B".repeat(64)),
        (RevisionFormat::Artifact, "".into()),
    ] {
        assert!(
            RevisionRef {
                resource: "resource".into(),
                format,
                value
            }
            .validate()
            .is_err()
        );
    }
}

#[test]
fn every_identity_version_contract_artifact_requirement_and_target_change_requires_reevaluation() {
    let original = candidate().binding;
    let changes: Vec<(&str, Value)> = vec![
        ("tenant_id", json!("other-tenant")),
        ("project_id", json!("other-project")),
        ("work_id", json!("other-work")),
        ("candidate_id", json!("other-candidate")),
        ("candidate_version", json!("2")),
        ("contract_hash", json!("f".repeat(64))),
        ("manifest_digest", json!("e".repeat(64))),
        ("required_checks", json!(["conformance", "integration"])),
        ("source_revision", Value::Null),
        (
            "target",
            json!({"resource":"repository:other","reference":"refs/heads/release","precondition":{"kind":"missing"}}),
        ),
    ];
    for (key, value) in changes {
        let mut changed = serde_json::to_value(&original).unwrap();
        changed[key] = value;
        let changed: CandidateBinding = serde_json::from_value(changed).unwrap();
        changed.validate().unwrap();
        assert!(
            require_same_candidate(&original, &changed).is_err(),
            "accepted changed {key}"
        );
        assert_ne!(original.digest().unwrap(), changed.digest().unwrap());
    }
    for mut record in records() {
        if let DeliveryRecord::AdapterCapabilities(_) = record {
            continue;
        }
        record.validate_against(&original).unwrap();
        match &mut record {
            DeliveryRecord::Verification(r) => r.binding.candidate_version = "2".into(),
            DeliveryRecord::ReviewDecision(r) => r.binding.contract_hash = "f".repeat(64),
            DeliveryRecord::IntegrationRequest(r) => {
                r.binding.target.reference = Some("refs/heads/other".into())
            }
            DeliveryRecord::IntegrationObservation(r) => {
                r.binding.work_id = WorkId::new("other-work").unwrap()
            }
            _ => continue,
        }
        assert!(record.validate_against(&original).is_err());
    }
}

#[test]
fn manifests_reject_tampering_duplicate_identities_and_noncanonical_counters() {
    let original = candidate();
    let mut changed = original.clone();
    changed.manifest.entries[0].locator = "artifact:changed".into();
    assert!(DeliveryRecord::Candidate(changed).validate().is_err());
    let mut manifest = original.manifest;
    manifest.entries.push(artifact());
    assert!(manifest.validate().is_err());
    for value in ["01", "+1", "18446744073709551616"] {
        let entry = ArtifactEntry {
            byte_length: value.into(),
            ..artifact()
        };
        assert!(entry.validate().is_err());
    }
    let large = ArtifactEntry {
        byte_length: "9007199254740993".into(),
        ..artifact()
    };
    large.validate().unwrap();
    let mut binding = candidate().binding;
    binding.candidate_version = "9007199254740993".into();
    binding.validate().unwrap();
    binding.candidate_version = "0".into();
    assert!(binding.validate().is_err());
}

#[test]
fn declared_verification_requires_an_inspectable_result_but_never_becomes_trusted_by_shape() {
    let mut run = records()
        .into_iter()
        .find_map(|r| {
            if let DeliveryRecord::Verification(v) = r {
                Some(v)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(run.provenance.source, FactSource::CallerDeclared);
    run.result_artifact = None;
    assert!(
        DeliveryRecord::Verification(run.clone())
            .validate()
            .is_err()
    );
    run.outcome = VerificationOutcome::Unknown;
    run.provenance.observed_at_unix_ms = None;
    let parsed = parse_delivery_record(
        &serde_json::to_vec(&envelope(DeliveryRecord::Verification(run))).unwrap(),
    )
    .unwrap();
    let DeliveryRecord::Verification(run) = parsed.record else {
        panic!("wrong record");
    };
    assert_eq!(run.outcome, VerificationOutcome::Unknown);
    assert_eq!(run.provenance.observed_at_unix_ms, None);
    assert_eq!(run.provenance.recorded_at_unix_ms, 2000);
    assert_eq!(run.provenance.source, FactSource::CallerDeclared);
}

#[test]
fn integration_intent_requires_the_exact_declared_check_set() {
    let request = records()
        .into_iter()
        .find_map(|r| {
            if let DeliveryRecord::IntegrationRequest(v) = r {
                Some(v)
            } else {
                None
            }
        })
        .unwrap();
    for checks in [
        vec![],
        vec![VerificationRef {
            check: "other-check".into(),
            run_id: "run".into(),
        }],
        vec![
            request.verified_checks[0].clone(),
            request.verified_checks[0].clone(),
        ],
    ] {
        let changed = IntegrationRequest {
            verified_checks: checks,
            ..request.clone()
        };
        assert!(
            DeliveryRecord::IntegrationRequest(changed)
                .validate()
                .is_err()
        );
    }
    DeliveryRecord::IntegrationRequest(request)
        .validate()
        .unwrap();
}

#[test]
fn applied_observation_needs_an_exact_manifest_and_target_revision_while_unknown_stays_unknown() {
    let observation = records()
        .into_iter()
        .find_map(|r| {
            if let DeliveryRecord::IntegrationObservation(v) = r {
                Some(v)
            } else {
                None
            }
        })
        .unwrap();
    let mut changed = observation.clone();
    changed.contains_manifest_digest = None;
    assert!(
        DeliveryRecord::IntegrationObservation(changed)
            .validate()
            .is_err()
    );
    let mut changed = observation.clone();
    changed.result_revision = None;
    assert!(
        DeliveryRecord::IntegrationObservation(changed)
            .validate()
            .is_err()
    );
    let mut changed = observation.clone();
    changed.contains_manifest_digest = Some("a".repeat(64));
    assert!(
        DeliveryRecord::IntegrationObservation(changed)
            .validate()
            .is_err()
    );
    let mut changed = observation.clone();
    changed.result_revision.as_mut().unwrap().resource = "other-target".into();
    assert!(
        DeliveryRecord::IntegrationObservation(changed)
            .validate()
            .is_err()
    );
    let unknown = IntegrationObservation {
        outcome: IntegrationOutcome::Unknown,
        result_revision: None,
        contains_manifest_digest: None,
        provenance: FactProvenance {
            observed_at_unix_ms: None,
            ..provenance()
        },
        ..observation
    };
    let parsed = parse_delivery_record(
        &serde_json::to_vec(&envelope(DeliveryRecord::IntegrationObservation(unknown))).unwrap(),
    )
    .unwrap();
    let DeliveryRecord::IntegrationObservation(record) = parsed.record else {
        panic!("wrong record");
    };
    assert_eq!(record.outcome, IntegrationOutcome::Unknown);
    assert!(record.result_revision.is_none());
}

#[test]
fn capability_support_is_descriptive_and_uncertain_integration_must_be_inspectable() {
    let mut capability = records()
        .into_iter()
        .find_map(|r| {
            if let DeliveryRecord::AdapterCapabilities(v) = r {
                Some(v)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        DeliveryRecord::AdapterCapabilities(capability.clone())
            .validate_against(&candidate().binding)
            .is_err()
    );
    capability.notifications = true;
    capability.polling = false;
    DeliveryRecord::AdapterCapabilities(capability.clone())
        .validate()
        .unwrap();
    capability.integration_observations = false;
    assert!(
        DeliveryRecord::AdapterCapabilities(capability)
            .validate()
            .is_err()
    );
}

#[test]
fn protocol_rejects_unknown_fields_bad_versions_and_oversize_input_without_echoing_values() {
    let original = serde_json::to_value(envelope(DeliveryRecord::Candidate(candidate()))).unwrap();
    for pointer in [
        "",
        "/record",
        "/record/data",
        "/record/data/binding",
        "/record/data/manifest",
        "/record/data/manifest/entries/0",
        "/record/data/binding/target/precondition",
    ] {
        let mut value = original.clone();
        value.pointer_mut(pointer).unwrap()["unexpected"] = json!("do-not-echo-test-value");
        let error = parse_delivery_record(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("do-not-echo-test-value"));
    }
    for (field, value) in [("protocol", json!("other")), ("protocol_version", json!(2))] {
        let mut changed = original.clone();
        changed[field] = value;
        assert_eq!(
            parse_delivery_record(&serde_json::to_vec(&changed).unwrap()).unwrap_err(),
            TeamError::ProtocolUnsupported
        );
    }
    assert!(parse_delivery_record(&vec![b' '; MAX_DELIVERY_RECORD_BYTES + 1]).is_err());
    let mut bad = original.clone();
    bad["record"]["data"]["binding"]["candidate_version"] = json!(1);
    assert!(parse_delivery_record(&serde_json::to_vec(&bad).unwrap()).is_err());
}

#[test]
fn invalid_counters_do_not_echo_input_and_target_preconditions_are_exact() {
    let original = serde_json::to_value(envelope(DeliveryRecord::Candidate(candidate()))).unwrap();
    for pointer in [
        "/record/data/binding/candidate_version",
        "/record/data/manifest/entries/0/byte_length",
    ] {
        let mut bad = original.clone();
        *bad.pointer_mut(pointer).unwrap() = json!("do-not-echo-test-value");
        let error = parse_delivery_record(&serde_json::to_vec(&bad).unwrap())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("do-not-echo-test-value"));
    }
    let current = candidate().binding;
    let mut changed = current.clone();
    let TargetPrecondition::Exact(revision) = &mut changed.target.precondition else {
        panic!("expected exact target");
    };
    revision.value = "f".repeat(64);
    assert!(require_same_candidate(&current, &changed).is_err());
    changed.target.precondition = TargetPrecondition::Missing;
    assert!(require_same_candidate(&current, &changed).is_err());
}
