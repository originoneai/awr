use awr_core::{Error, Result};
use clap::Args;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeSet;

const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Args)]
pub struct CapabilitiesArgs {
    /// Version of the capability negotiation contract, not the program version.
    #[arg(long, default_value_t = PROTOCOL_VERSION)]
    protocol_version: u32,
    /// Require an available capability. Repeat for each prerequisite.
    #[arg(long = "require")]
    required: Vec<String>,
}

#[derive(Serialize)]
struct Capability {
    id: &'static str,
    available: bool,
    commands: &'static [&'static str],
    limitations: &'static [&'static str],
}

fn catalog() -> Vec<Capability> {
    [
        (
            "runtime.matched_snapshot",
            true,
            &[
                "runtime binding",
                "runtime backup",
                "runtime check",
                "runtime restore-preview",
                "runtime restore",
                "runtime restore-status",
                "runtime restore-recover",
            ][..],
            &[
                "same_root",
                "same_executable_bytes",
                "matching_sources_and_known_runtime_files",
                "database_only_restore",
                "explicit_offline",
                "new_files_preserved",
                "external_host_state_excluded",
            ][..],
        ),
        (
            "query.summary",
            true,
            &["status --view summary"][..],
            &[
                "explicit_scope",
                "source_counts_are_not_acceptance",
                "details_on_demand",
            ][..],
        ),
        (
            "organization.mapped_metadata",
            true,
            &[
                "organization preview",
                "organization change",
                "organization status",
                "organization recover",
            ][..],
            &[
                "primary_yaml",
                "explicit_mapping",
                "no_entity_or_acceptance_changes",
                "exact_preview",
            ][..],
        ),
        (
            "source.read",
            true,
            &["source list", "source show"][..],
            &["registered_sources_only", "bounded_body_reads"][..],
        ),
        (
            "source.refresh",
            true,
            &["source scan", "source reindex", "source history"],
            &["may_write_runtime", "partial_failure_requires_retry"],
        ),
        (
            "source.changes",
            true,
            &["source changes"],
            &[
                "read_only_event_window",
                "consumer_owns_success_cursor",
                "current_failures_are_not_retirements",
            ],
        ),
        (
            "intake.reviewed_draft",
            true,
            &["init", "intake inspect"],
            &[
                "explicit_accept",
                "draft_fingerprint_checked",
                "effect_binding_requires_expected_preview",
            ],
        ),
        (
            "intake.exact_preview",
            true,
            &["init"],
            &["requires_expected_preview", "individual_files_only"],
        ),
        (
            "intake.semantic_preflight",
            true,
            &["init", "source configure"],
            &[
                "captured_bytes",
                "no_project_writes",
                "execution_readiness_separate",
                "bounded_temporary_snapshot",
            ],
        ),
        (
            "source.relocate",
            true,
            &[
                "source relocate",
                "source relocate-status",
                "source relocate-recover",
            ],
            &[
                "single_relative_file",
                "unchanged_contents",
                "stable_object_keys",
                "explicit_preview",
                "recoverable_not_filesystem_atomic",
            ],
        ),
        (
            "source.configure",
            true,
            &["source configure", "source configure-status"],
            &[
                "existing_project_identity_preserved",
                "requires_expected_preview",
                "projection_can_partially_fail",
            ],
        ),
        (
            "query.coherent_snapshot",
            true,
            &[
                "status",
                "ready",
                "work show",
                "work graph",
                "object list",
                "search",
                "context compile",
            ],
            &[
                "source_transition_wait_bounded",
                "query_revision_is_a_snapshot",
                "source_fingerprint_excludes_runtime_events",
                "cached_mode_does_not_verify_current_sources",
            ],
        ),
        (
            "work.read",
            true,
            &["work show", "ready", "status"],
            &["summaries_are_not_a_complete_catalog"],
        ),
        (
            "object.read",
            true,
            &["object show", "decision show"],
            &["explicit_object_reference", "bounded_body_reads"],
        ),
        (
            "project.catalog",
            true,
            &["object list"],
            &[
                "revision_bound_pages",
                "retired_is_not_archived",
                "summaries_only_bodies_are_opt_in",
            ],
        ),
        (
            "history.cursor",
            true,
            &["event history", "source history", "work history"],
            &["revision_bound_cursor"],
        ),
        (
            "context.compile",
            true,
            &["context compile"],
            &["may_write_runtime", "incomplete_or_over_budget_is_failure"],
        ),
        (
            "session.checkpoint",
            true,
            &["session checkpoint"],
            &["explicit_session", "caller_supplied_digest"],
        ),
        (
            "session.resume",
            true,
            &["session resume", "recovery inspect"],
            &[
                "inspect_is_separate_from_resume",
                "not_native_client_resume",
            ],
        ),
        (
            "client.lifecycle.generic",
            true,
            &["client bind", "client progress", "client hook"],
            &[
                "documented_lifecycle_events_only",
                "no_required_hook_installation",
            ],
        ),
        (
            "execution.external.register",
            true,
            &["execution register", "execution show"],
            &["external_reference_unverified", "not_managed_execution"],
        ),
        (
            "execution.managed",
            true,
            &["execution run", "execution inspect"],
            &["local_supervisor", "project_scoped_operation_key"],
        ),
        (
            "execution.external.report",
            true,
            &[
                "execution report",
                "execution report-status",
                "execution inspect",
            ],
            &[
                "host_supplied_provenance",
                "never_promotes_managed_or_verified",
                "request_key_idempotent",
            ],
        ),
        (
            "artifact.managed",
            true,
            &["artifact add", "artifact show", "artifact cat"],
            &["add_copies_content", "bounded_authorized_reads"],
        ),
        (
            "evidence.read_write",
            true,
            &["evidence add", "evidence show"],
            &[
                "registration_is_not_verification",
                "command_field_is_not_executed",
            ],
        ),
        (
            "mutation.yaml.record",
            true,
            &["proposal create", "proposal apply"],
            &[
                "supported_fields_only",
                "field_spans_preserve_unrelated_bytes",
                "state_requires_domain_action",
                "single_file",
            ],
        ),
        (
            "mutation.yaml.lossless_fields",
            true,
            &["proposal apply"],
            &[
                "supported_yaml_shapes_only",
                "scalar_style_when_representable",
                "domain_actions_still_required",
            ],
        ),
        (
            "mutation.work.create",
            true,
            &["work create", "work create-status", "work create-recover"],
            &[
                "registered_yaml_or_supported_markdown_ledger",
                "exact_preview_and_revision",
                "stable_request_key",
                "draft_state_not_executable",
            ],
        ),
        (
            "mutation.markdown",
            true,
            &[
                "document change",
                "host save",
                "work create",
                "work progress",
            ],
            &[
                "finite_stable_id_ledger_records",
                "registered_document_bodies",
            ],
        ),
        (
            "mutation.markdown.document",
            true,
            &["document change", "document status", "document recover"],
            &[
                "single_file",
                "registered_sources_only",
                "exact_preview_and_revision",
                "preserve_identity_and_lifecycle",
                "draft_no_clobber",
            ],
        ),
        (
            "mutation.human_save",
            true,
            &["host save", "host status", "host recover"],
            &[
                "registered_sources_only",
                "provenance_is_not_authentication",
                "domain_guards_retained",
                "single_file",
            ],
        ),
        (
            "mutation.ai_apply",
            true,
            &["host preview", "host save"],
            &["exact_reviewed_preview_required", "stable_request_identity"],
        ),
        (
            "mutation.work.activate_draft",
            true,
            &["host save"],
            &[
                "draft_only",
                "declared_structure_and_dependencies_required",
                "not_completion",
            ],
        ),
        (
            "mutation.batch.ledger",
            true,
            &["batch change", "batch status", "batch recover"],
            &[
                "single_source",
                "all_operations_preflight",
                "exact_preview_and_revision",
                "draft_import_explicit_duplicates",
            ],
        ),
        (
            "mutation.work.archive",
            true,
            &["batch change"],
            &[
                "separate_from_lifecycle",
                "dependency_and_claim_guards",
                "identity_and_history_preserved",
            ],
        ),
        (
            "mutation.multi_file",
            true,
            &["batch change", "batch status", "batch recover"],
            &[
                "all_targets_preflight",
                "per_file_durable_steps",
                "not_filesystem_atomic",
                "existing_registered_documents_and_one_ledger",
            ],
        ),
        (
            "mutation.decision.adopt",
            true,
            &["batch change"],
            &[
                "explicit_version_bound_decisions",
                "finite_yaml_frontmatter",
                "preserve_old_document",
                "changed_content_invalidates_approval",
            ],
        ),
        (
            "completion.engineering",
            true,
            &["work complete"],
            &[
                "session_and_claim_required",
                "source_sha_and_acceptance_evidence_required",
            ],
        ),
        (
            "workflow.prepare",
            true,
            &["work prepare"],
            &[
                "required_context_preserved",
                "explicit_consumption",
                "no_automatic_claim",
            ],
        ),
        (
            "workflow.response_summary",
            true,
            &[
                "work prepare --response-view summary",
                "work progress --response-view summary",
                "work complete --response-view summary",
            ],
            &[
                "optional_presentation",
                "full_required_context",
                "errors_unchanged",
                "stored_receipts_unchanged",
            ],
        ),
        (
            "workflow.action_guidance",
            true,
            &["work prepare --response-view action"],
            &[
                "one_conditional_action",
                "guidance_max_1024_bytes",
                "required_context_preserved",
                "full_and_summary_compatible",
            ],
        ),
        (
            "client.compaction",
            true,
            &[
                "session compaction observe",
                "session compaction inspect",
                "session compaction defer",
            ],
            &[
                "host_reported_completed_compaction",
                "unknown_metrics_preserved",
                "native_compaction_unchanged",
                "no_automatic_session_switch",
            ],
        ),
        (
            "work.management",
            true,
            &["work assess", "work manage"],
            &[
                "attributed_observations",
                "no_automatic_downgrade",
                "completion_policy_unchanged",
            ],
        ),
        (
            "assessment.replay",
            true,
            &["assessment replay"][..],
            &[
                "offline_fixed_inputs_only",
                "missing_snapshot_not_replayable",
                "no_production_reread_no_tools_no_model",
            ][..],
        ),
        (
            "assessment.shadow_compare",
            true,
            &["assessment compare"][..],
            &[
                "same_input_baseline_candidate",
                "diffs_explained_by_rule_version",
                "failed_samples_retained",
                "no_execution_or_context_adoption",
                "no_background_daemon",
            ][..],
        ),
        (
            "assessment.advice_mode",
            true,
            &["assessment advice-mode"][..],
            &[
                "disable_restores_prior_advice_only",
                "hard_protections_remain",
                "shadow_non_adopting",
                "no_background_daemon",
            ][..],
        ),
        (
            "assessment.explain",
            true,
            &["work prepare", "work assess"],
            &[
                "negotiated_field_only",
                "default_off_preserves_legacy_views",
                "read_only_no_claim_no_complete_no_auto_tools",
                "reuses_prepare_explanation_chain",
            ],
        ),
        (
            "work.graph",
            true,
            &["work graph"],
            &[
                "complete_dependency_closure",
                "source_bound_impact",
                "no_execution_admission",
            ],
        ),
        (
            "completion.prepare",
            true,
            &["work prepare-completion"],
            &[
                "actual_local_report",
                "caller_asserted_level",
                "no_execution_or_completion",
            ],
        ),
        (
            "completion.user_confirmation",
            true,
            &["host save"],
            &[
                "explicit_scoped_policy_v1",
                "human_provenance_required",
                "separate_from_engineering_verification",
            ],
        ),
    ]
    .into_iter()
    .map(|(id, available, commands, limitations)| Capability {
        id,
        available,
        commands,
        limitations,
    })
    .collect()
}

pub fn run(args: &CapabilitiesArgs, json_output: bool) -> Result<()> {
    if args.protocol_version != PROTOCOL_VERSION {
        return Err(Error::ProtocolUnsupported {
            requested: args.protocol_version,
            supported: vec![PROTOCOL_VERSION],
        });
    }
    if args.required.len() > 100
        || args.required.iter().any(|id| {
            id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err(Error::InvalidInput(
            "require accepts at most 100 capability IDs of 1..128 ASCII identifier characters"
                .into(),
        ));
    }
    let capabilities = catalog();
    let mut unknown = Vec::new();
    let mut unsupported = Vec::new();
    for id in args.required.iter().collect::<BTreeSet<_>>() {
        match capabilities.iter().find(|c| c.id == id) {
            None => unknown.push(id.clone()),
            Some(c) if !c.available => unsupported.push(id.clone()),
            Some(_) => (),
        }
    }
    if !unknown.is_empty() || !unsupported.is_empty() {
        return Err(Error::CapabilityUnavailable {
            unknown,
            unsupported,
        });
    }
    // Deliberately no project path, environment lookup, database open or source scan.
    let value = json!({
        "ok": true,
        "protocol": {"name": "awr.host", "version": PROTOCOL_VERSION},
        "program": {"name": "awr", "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "database": {"schema_version": awr_store::SCHEMA_VERSION,
            "read_only_schema_versions": [awr_store::SCHEMA_VERSION],
            "migrate_from_schema_versions": [1, 2, 3],
            "newer_schema_policy": "reject_without_migration",
            "schema_validation_required": true},
        "source_adapters": awr_source::SOURCE_ADAPTERS.iter().map(|id| json!({
            "id": id, "read": true,
            "write_mode": match *id { "yaml-ledger-v1" => "lossless_supported_fields", "markdown-ledger-v1" => "stable_id_table_or_checklist", _ => "document_body_only" },
            "lossless_field_write": matches!(*id, "yaml-ledger-v1" | "markdown-ledger-v1")
        })).collect::<Vec<_>>(),
        "capabilities": capabilities,
        "source_write_performed": false,
        "runtime_write_performed": false,
        "scope": "build_capabilities_not_project_authorization",
        "transport": {"invocation": "argv", "json_flag": "--json",
            "success_stream": "stdout", "error_stream": "stderr",
            "exit_codes": {"success": 0, "runtime_error": 1, "usage_error": 2},
            "partial_result_policy": "stdout_may_accompany_nonzero_exit",
            "timeout_policy": "outcome_unknown_inspect_before_retry"}
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "awr {} — host protocol {}",
            env!("CARGO_PKG_VERSION"),
            PROTOCOL_VERSION
        );
        for capability in capabilities {
            println!(
                "{}: {}",
                capability.id,
                if capability.available {
                    "available"
                } else {
                    "unavailable"
                }
            );
        }
    }
    Ok(())
}
