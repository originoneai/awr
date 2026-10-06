//! Authoritative project source configuration and adapter contracts.
mod adapter;
mod delivery_writeback;
mod directory;
mod document;
mod file_inventory;
mod freshness;
mod indexer;
mod ledger_mapping;
mod limits;
mod locator;
mod manifest;
mod markdown;
mod markdown_ledger;
mod mutation;
mod planning_writeback;
mod publish_prep;
mod query_snapshot;
mod source_concurrency;
pub use query_snapshot::{
    QuerySnapshot, recorded_snapshot, refresh_snapshot, source_state_fingerprint,
};
mod yaml_create;
mod yaml_edit;
mod yaml_ledger;
mod yaml_mutation;

pub use adapter::{ParseContext, ProjectionBatch, SourceAdapter};
pub use awr_core::{Error, Result};
pub use delivery_writeback::{
    DeliveryCompletionReference, DeliverySourceNote, LockedSourceFile, SourceFileIdentity,
    prepare_delivery_source_note,
};
pub use directory::{DirectoryDelta, DirectoryInventory, MarkdownDirectoryAdapter};
pub use document::{
    DocumentAction, DocumentEdit, PreparedDocument, document_path_registration,
    prepare_document_draft, prepare_document_edit,
};
pub use file_inventory::{
    FileInventory, InventoryChange, InventoryDiff, compare_file_inventories, inventory_files,
};
pub use freshness::{SourceObservation, observe_source};
pub use indexer::{
    IndexIssue, IndexReport, IndexedSource, PreviewSources, index_project, index_project_locked,
    preview_index_project, scan_project, source_adapter, source_configuration,
};
pub use ledger_mapping::LedgerMapping;
pub use limits::{MARKDOWN_READ_CAP, YAML_READ_CAP, source_read_cap};
pub use locator::{Locator, SourceSnapshot, fingerprint, read_capped, read_source_capped};
pub use publish_prep::{
    DEFAULT_COMPLETION_POLICY, FieldDiff, PARSER_VERSION, PublishPackageFile, PublishPrepOptions,
    PublishPreview, ReferencedSpec, SOURCE_BINDING_FILE, SOURCE_PROVENANCE_FILE,
    SUPPORTED_LEDGER_ADAPTER, SoleSourceKind, SoleSourceLocation, SourceProvenance,
    SourceStatusNote, TeamPublishPackage, WORKSTREAMS_FILE, prepare_publish_from_ledger_bytes,
    prepare_publish_from_server_directory, source_status_notes_are_completion_receipts,
};
mod safe_fs;
pub use manifest::{
    ContextProfile, Manifest, ProjectConfig, SOURCE_ADAPTERS, SourceSpec, minimal_context,
};
pub use markdown::{
    MarkdownHeadingAdapter, MarkdownRulesAdapter, MarkdownSection, markdown_sections,
};
pub use markdown_ledger::MarkdownLedgerAdapter;
pub use mutation::{
    MutationSourceCheck, inspect_mutation_source, inspect_registered_source, verify_mutation_source,
};
pub use planning_writeback::{
    CompatibleStatusWriteback, FieldWriteAuthority, LedgerWritebackPatch, RUNTIME_ONLY_FIELDS,
    SOURCE_WRITABLE_FIELDS, VerifiedDomainStatus, apply_planning_changes_to_ledger,
    derive_compatible_status_writeback, refuse_external_overwrite,
    refuse_runtime_field_in_source_write, runtime_field_authority, source_field_authority,
};
pub use safe_fs::{open_dir_exact, open_file_exact, read_under_root};
pub use source_concurrency::{
    ShardCandidate, ShardObservation, ShardWrite, SourceWriteMode, form_shard_candidate,
    observe_candidate, observe_shard, refuse_stale_proposal_base, refuse_stale_whole_file,
    require_write_mode, source_write_mode,
};
pub use yaml_create::{
    PreparedWorkCreation, prepare_work_creation, prepare_work_creation_with_fields,
};
pub use yaml_ledger::{YamlLedgerAdapter, YamlWorkstreamLedgerAdapter};
pub use yaml_mutation::{
    PreparedYamlMutation, parse_mutation_projection, prepare_yaml_mutation,
    read_yaml_mutation_record, yaml_field_writable,
};
mod markdown_records;

mod markdown_mutation;
pub use markdown_mutation::*;

mod ledger_batch;
pub use ledger_batch::*;

mod adoption;
pub use adoption::*;

mod organization_edit;
pub use organization_edit::{edit_organization_fields, organization_fields};

mod content_review;
pub use content_review::{
    ProjectContentReview, archive_content_review, read_project_document, scan_content_review,
};
pub use locator::read_source_for_review;
