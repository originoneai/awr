//! Bounded provider reads establish content identity, never integration authority.
use super::{GitHubError, Query, sha};
use awr_team::delivery::*;
use serde_json::Value;
use std::{collections::BTreeSet, time::Instant};
use url::Url;

fn unavailable(reason: ContentProofUnavailableReason) -> IntegrationContentWitness {
    IntegrationContentWitness::Unavailable { reason }
}

impl Query<'_> {
    /// REST comparison has no `head_commit` field. Bind its response URL to the
    /// exact requested revision pair, independently of commit-list pagination.
    /// This checks identity only and never follows a provider-supplied URL.
    pub(super) fn comparison(&mut self, base: &str, head: &str) -> Result<Value, GitHubError> {
        let pair = format!("{base}...{head}");
        let comparison = self.required(&["compare", &pair])?;
        let mut expected = Url::parse(&self.adapter.config.api_base_url).unwrap();
        expected
            .path_segments_mut()
            .map_err(|_| GitHubError::InvalidConfiguration)?
            .pop_if_empty()
            .extend([
                "repos",
                &self.adapter.config.owner,
                &self.adapter.config.repository,
                "compare",
                &pair,
            ]);
        let actual = comparison["url"]
            .as_str()
            .and_then(|value| Url::parse(value).ok())
            .ok_or(GitHubError::BindingMismatch)?;
        let expected_parts: Vec<_> = expected.path_segments().unwrap().collect();
        let actual_parts: Vec<_> = actual
            .path_segments()
            .ok_or(GitHubError::BindingMismatch)?
            .collect();
        let repository_part = expected_parts.len() - 4;
        let same_path = actual_parts.len() == expected_parts.len()
            && actual_parts
                .iter()
                .zip(&expected_parts)
                .enumerate()
                .all(|(index, (a, e))| {
                    if index == repository_part || index == repository_part + 1 {
                        a.eq_ignore_ascii_case(e)
                    } else {
                        a == e
                    }
                });
        if actual.origin() != expected.origin()
            || !actual.username().is_empty()
            || actual.password().is_some()
            || actual.query().is_some()
            || actual.fragment().is_some()
            || !same_path
            || sha(&comparison["base_commit"]["sha"])? != base
        {
            return Err(GitHubError::BindingMismatch);
        }
        Ok(comparison)
    }

    /// A status or merge base from another requested pair cannot prove ancestry.
    pub(super) fn contains(&mut self, base: &str, head: &str) -> Result<bool, GitHubError> {
        if base == head {
            return Ok(true);
        }
        let comparison = self.comparison(base, head)?;
        let ancestor = sha(&comparison["merge_base_commit"]["sha"])? == base;
        match comparison["status"].as_str() {
            Some("ahead" | "identical") if ancestor => Ok(true),
            Some("behind" | "diverged") if !ancestor => Ok(false),
            _ => Err(GitHubError::InvalidResponse),
        }
    }

    /// Read every subtree, including paths outside the selected manifest. Git's
    /// root tree identity includes all paths, modes and object identities. These
    /// observations trust the configured provider transport, not caller URLs or
    /// a caller-supplied tree. Every response is nonrecursive and nontruncated,
    /// and uses the existing aggregate request, response and deadline budgets.
    fn complete_tree(&mut self, root: &str) -> Result<(), GitHubError> {
        let mut pending = vec![(root.to_owned(), false)];
        let mut visiting = BTreeSet::new();
        let mut complete = BTreeSet::new();
        while let Some((id, exiting)) = pending.pop() {
            if Instant::now() >= self.deadline {
                return Err(GitHubError::TimedOut);
            }
            if exiting {
                visiting.remove(&id);
                complete.insert(id);
                continue;
            }
            if complete.contains(&id) {
                continue;
            }
            if !visiting.insert(id.clone()) {
                return Err(GitHubError::InvalidResponse);
            }
            let tree = self.tree(&id)?;
            pending.push((id, true));
            let mut paths = BTreeSet::new();
            for entry in tree["tree"].as_array().unwrap() {
                let path = entry["path"].as_str().ok_or(GitHubError::InvalidResponse)?;
                if path.is_empty()
                    || matches!(path, "." | "..")
                    || path.contains(['/', '\0'])
                    || !paths.insert(path)
                {
                    return Err(GitHubError::InvalidResponse);
                }
                let object = sha(&entry["sha"])?;
                match (entry["mode"].as_str(), entry["type"].as_str()) {
                    (Some("040000"), Some("tree")) => pending.push((object, false)),
                    (Some("100644" | "100755" | "120000"), Some("blob")) => {}
                    // A gitlink belongs to the snapshot, not to this repository's
                    // reachable tree. Do not follow a different repository.
                    (Some("160000"), Some("commit")) => {}
                    _ => return Err(GitHubError::InvalidResponse),
                }
            }
        }
        Ok(())
    }

    pub(super) fn content_witness(
        &mut self,
        candidate: &DeliveryCandidate,
        source_tree: &str,
        result: Option<&RevisionRef>,
    ) -> Result<IntegrationContentWitness, GitHubError> {
        let source = candidate.binding.source_revision.as_ref().unwrap();
        let Some(result) = result else {
            return Ok(unavailable(ContentProofUnavailableReason::NotObserved));
        };
        if source == result {
            self.complete_tree(source_tree)?;
            return Ok(IntegrationContentWitness::ExactRevision);
        }
        let TargetPrecondition::Exact(base) = &candidate.binding.target.precondition else {
            return Ok(unavailable(ContentProofUnavailableReason::BaseChanged));
        };
        let result_tree = self.commit(&result.value)?;
        self.commit(&base.value)?;
        if !self.contains(&base.value, &source.value)?
            || !self.contains(&base.value, &result.value)?
        {
            return Ok(unavailable(ContentProofUnavailableReason::BaseChanged));
        }
        if source_tree != result_tree {
            return Ok(unavailable(ContentProofUnavailableReason::ContentChanged));
        }
        self.complete_tree(source_tree)?;
        let snapshot = CompleteSnapshotIdentity {
            resource: self.adapter.config.resource.clone(),
            format: SnapshotIdentityFormat::GitTreeSha1,
            value: source_tree.into(),
        };
        Ok(IntegrationContentWitness::MatchingCompleteSnapshots {
            source_snapshot: snapshot.clone(),
            result_snapshot: snapshot,
            retained_base: base.clone(),
        })
    }
}
