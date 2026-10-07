//! Bounded complete-tree reads. Rewritten history is observed, never performed.
use super::{LocalGitAdapter, LocalGitError};
use awr_team::delivery::*;

fn unavailable(reason: ContentProofUnavailableReason) -> IntegrationContentWitness {
    IntegrationContentWitness::Unavailable { reason }
}

impl LocalGitAdapter {
    pub(super) async fn content_witness(
        &self,
        candidate: &DeliveryCandidate,
        result: Option<&RevisionRef>,
        source_available: bool,
    ) -> IntegrationContentWitness {
        self.resolve_content(candidate, result, source_available)
            .await
            .unwrap_or_else(|_| unavailable(ContentProofUnavailableReason::HistoryUnavailable))
    }

    async fn tree(
        &self,
        revision: &RevisionRef,
    ) -> Result<Option<CompleteSnapshotIdentity>, LocalGitError> {
        if revision.resource != self.config.resource
            || revision.format != self.format
            || revision.validate().is_err()
            || self
                .git
                .text(&["cat-file", "-t", &revision.value])
                .await?
                .as_deref()
                != Some("commit")
        {
            return Ok(None);
        }
        let spec = format!("{}^{{tree}}", revision.value);
        let Some(value) = self.git.text(&["rev-parse", "--verify", &spec]).await? else {
            return Ok(None);
        };
        let tree = CompleteSnapshotIdentity {
            resource: self.config.resource.clone(),
            format: if self.format == RevisionFormat::GitSha1 {
                SnapshotIdentityFormat::GitTreeSha1
            } else {
                SnapshotIdentityFormat::GitTreeSha256
            },
            value,
        };
        if tree.validate().is_err()
            || self
                .git
                .text(&["cat-file", "-t", &tree.value])
                .await?
                .as_deref()
                != Some("tree")
        {
            return Ok(None);
        }
        Ok(Some(tree))
    }

    async fn resolve_content(
        &self,
        candidate: &DeliveryCandidate,
        result: Option<&RevisionRef>,
        source_available: bool,
    ) -> Result<IntegrationContentWitness, LocalGitError> {
        let (Some(source), Some(result)) = (candidate.binding.source_revision.as_ref(), result)
        else {
            return Ok(unavailable(ContentProofUnavailableReason::NotObserved));
        };
        if source.format != self.format || result.format != self.format {
            return Ok(unavailable(ContentProofUnavailableReason::Unsupported));
        }
        if !source_available {
            return Ok(unavailable(
                ContentProofUnavailableReason::HistoryUnavailable,
            ));
        }
        if source == result {
            return Ok(IntegrationContentWitness::ExactRevision);
        }
        let TargetPrecondition::Exact(base) = &candidate.binding.target.precondition else {
            return Ok(unavailable(ContentProofUnavailableReason::BaseChanged));
        };
        if base.resource != self.config.resource || base.format != self.format {
            return Ok(unavailable(ContentProofUnavailableReason::Unsupported));
        }
        if self
            .git
            .text(&["cat-file", "-t", &base.value])
            .await?
            .as_deref()
            != Some("commit")
        {
            return Ok(unavailable(
                ContentProofUnavailableReason::HistoryUnavailable,
            ));
        }
        for revision in [source, result] {
            match self
                .git
                .run(
                    &["merge-base", "--is-ancestor", &base.value, &revision.value],
                    4096,
                )
                .await?
                .code
            {
                0 => {}
                1 => return Ok(unavailable(ContentProofUnavailableReason::BaseChanged)),
                _ => {
                    return Ok(unavailable(
                        ContentProofUnavailableReason::HistoryUnavailable,
                    ));
                }
            }
        }
        let (Some(source_snapshot), Some(result_snapshot)) =
            (self.tree(source).await?, self.tree(result).await?)
        else {
            return Ok(unavailable(
                ContentProofUnavailableReason::HistoryUnavailable,
            ));
        };
        if source_snapshot != result_snapshot {
            return Ok(unavailable(ContentProofUnavailableReason::ContentChanged));
        }
        // A tree hash covers every path and mode. Check its whole reachable
        // object graph as well, including blobs outside the selected manifest.
        // Quiet traversal is bounded by the existing command/inspection limits
        // and cannot fetch missing objects or execute a helper.
        if self
            .git
            .run(
                &[
                    "rev-list",
                    "--objects",
                    "--quiet",
                    "--missing=error",
                    &source_snapshot.value,
                ],
                4096,
            )
            .await?
            .code
            != 0
        {
            return Ok(unavailable(
                ContentProofUnavailableReason::HistoryUnavailable,
            ));
        }
        Ok(IntegrationContentWitness::MatchingCompleteSnapshots {
            source_snapshot,
            result_snapshot,
            retained_base: base.clone(),
        })
    }
}
