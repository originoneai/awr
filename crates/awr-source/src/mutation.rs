use crate::{
    Locator, Manifest, MarkdownDirectoryAdapter, SourceSnapshot, SourceSpec, source_adapter,
};
use awr_core::{Error, Freshness, MutationPatch, Result, Source};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct MutationSourceCheck {
    pub locator: String,
    pub fingerprint: String,
    pub adapter: String,
    pub checked_at: i64,
}

/// Read-only observation of the actual current manifest mapping and source bytes. This is
/// not a file lock: a future writer must repeat the comparison immediately before writing.
pub fn verify_mutation_source(
    root: &Path,
    source: &Source,
    patch: &MutationPatch,
) -> Result<MutationSourceCheck> {
    patch.validate()?;
    let reference = &patch.target.meta.source_ref;
    if source.id != reference.source_id
        || source.fingerprint != reference.source_fingerprint
        || source.revision != reference.source_revision
        || source.config != patch.source_config
    {
        return Err(Error::SourceConflict(
            "proposal binding differs from the indexed source; create a new proposal".into(),
        ));
    }
    match source.freshness {
        Freshness::Fresh => (),
        Freshness::Stale => return Err(Error::SourceStale(
            "proposal source is stale; reindex before creating a replacement proposal".into(),
        )),
        Freshness::Unavailable => return Err(Error::SourceUnavailable(
            "proposal source is unavailable; restore and reindex before creating a replacement proposal".into(),
        )),
    }
    let (_, _, snapshot) = inspect_mutation_source(root, source, patch)?;
    if snapshot.fingerprint != reference.source_fingerprint {
        return Err(Error::SourceConflict(
            "proposal source bytes changed; create a new proposal from current facts".into(),
        ));
    }
    Ok(MutationSourceCheck {
        locator: snapshot.locator,
        fingerprint: snapshot.fingerprint,
        adapter: source.adapter.clone(),
        checked_at: awr_core::now_millis()?,
    })
}

/// Resolve the original mapping but observe bytes independently of cached freshness. Recovery
/// uses this to recognize the exact pre-write or post-write state of a recorded attempt.
pub fn inspect_mutation_source(
    root: &Path,
    source: &Source,
    patch: &MutationPatch,
) -> Result<(Locator, SourceSpec, SourceSnapshot)> {
    patch.validate()?;
    if source.id != patch.target.meta.source_ref.source_id || source.config != patch.source_config {
        return Err(Error::SourceConflict(
            "mutation source identity/configuration differs from its binding".into(),
        ));
    }
    let (locator, spec, snapshot) = inspect_registered_source(root, source)?;
    if snapshot.locator != patch.target.meta.source_ref.locator {
        return Err(Error::SourceConflict("proposal source bytes or immutable locator changed; keep the proposal for review and create a new one from current facts".into()));
    }
    Ok((locator, spec, snapshot))
}

/// Resolve exactly one currently registered source, retaining its bound configuration.
/// This is a read-only observation; writers must hold their source lock and recheck bytes.
pub fn inspect_registered_source(
    root: &Path,
    source: &Source,
) -> Result<(Locator, SourceSpec, SourceSnapshot)> {
    let root = root.canonicalize()?;
    let manifest = Manifest::load(&root)?;
    let mut matches = Vec::new();
    // Resolve every mapping in this domain, so a newly added alias cannot silently acquire
    // the proposal's source. A failed scan cannot establish unique ownership.
    for spec in manifest
        .sources
        .iter()
        .filter(|s| s.domain == source.domain)
    {
        let adapter = source_adapter(&spec.adapter)?;
        for locator in adapter.discover(&root, &manifest, spec)? {
            let identity = if spec.adapter == "markdown-directory-v1" {
                MarkdownDirectoryAdapter.source_identity(&root, &manifest, spec, &locator)?
            } else {
                locator.identity()?
            };
            if identity != source.locator {
                continue;
            }
            let config = crate::indexer::source_configuration(
                spec,
                manifest.project.context_profile == crate::ContextProfile::Minimal,
            );
            if source.adapter != spec.adapter || source.role != spec.role || config != source.config
            {
                return Err(Error::SourceConflict(
                    "proposal source mapping or adapter configuration has changed".into(),
                ));
            }
            matches.push((locator, spec.clone()));
        }
    }
    if matches.len() != 1 {
        return Err(Error::SourceConflict(format!(
            "proposal source must have exactly one current manifest mapping; found {}",
            matches.len()
        )));
    }
    let (locator, spec) = matches.pop().unwrap();
    let snapshot = locator.read(&root, crate::source_read_cap(&spec.adapter)?)?;
    // Mutation recovery archives exact before/after bytes. Warn admission must
    // not copy an unredacted source into that archive or a proposed patch.
    if awr_core::secrets_mode() == awr_core::SecretsMode::Warn && snapshot.content_review.is_none()
    {
        awr_core::ensure_public_bytes(&snapshot.bytes)?;
    }
    Ok((locator, spec, snapshot))
}
