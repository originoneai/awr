//! One admission and cancellation boundary for optional source and repository workers.
use super::{
    ServiceConfig,
    delivery_sync::{DeliveryWorkerMonitor, DeliveryWorkerRuntime},
    github_worker::{GitHubWorkerMonitor, GitHubWorkerRuntime, GitHubWorkerTransports},
    local_git_worker::{LocalGitWorkerMonitor, LocalGitWorkerRuntime},
};
use crate::{
    config::{DeliveryWorkerConfig, GitHubWorkerConfig, GitHubWorkerSpec, LocalGitWorkerConfig},
    delivery_adapter::GitHubError,
};
use awr_team_pg::DeliverySyncStore;
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;

pub struct DeliveryWorkers {
    source: DeliveryWorkerRuntime,
    git: LocalGitWorkerRuntime,
    github: GitHubWorkerRuntime,
}

impl DeliveryWorkers {
    /// Both sealed admissions must succeed before either task group can spawn.
    pub async fn start(
        service: &ServiceConfig,
        source: Option<DeliveryWorkerConfig>,
        git: Option<LocalGitWorkerConfig>,
        store: DeliverySyncStore,
        credential_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, String> {
        Self::start_with_github(service, source, git, None, store, credential_lookup).await
    }

    /// Every supplied group is admitted before any worker task is spawned.
    pub async fn start_with_github(
        service: &ServiceConfig,
        source: Option<DeliveryWorkerConfig>,
        git: Option<LocalGitWorkerConfig>,
        github: Option<GitHubWorkerConfig>,
        store: DeliverySyncStore,
        credential_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, String> {
        Self::start_with_github_transports(
            service,
            source,
            git,
            github,
            store,
            credential_lookup,
            |_, _| Ok(None),
        )
        .await
    }

    /// Trusted library extension only; no route or configuration can inject a transport.
    pub async fn start_with_github_transports(
        service: &ServiceConfig,
        source: Option<DeliveryWorkerConfig>,
        git: Option<LocalGitWorkerConfig>,
        github: Option<GitHubWorkerConfig>,
        store: DeliverySyncStore,
        mut credential_lookup: impl FnMut(&str) -> Option<String>,
        mut transports: impl FnMut(
            &GitHubWorkerSpec,
            Zeroizing<String>,
        ) -> Result<Option<GitHubWorkerTransports>, GitHubError>,
    ) -> Result<Self, String> {
        service.validate()?;
        if let Some(config) = &source {
            config.validate(service)?;
        }
        if let Some(config) = &git {
            config.validate(service)?;
        }
        if let Some(config) = &github {
            config.validate(service)?;
        }
        // A connector has one observer scope, irrespective of provider/group or project alias.
        let mut scopes = std::collections::BTreeSet::new();
        for repo in git
            .iter()
            .flat_map(|c| &c.workers)
            .map(|w| {
                (
                    &w.repository.tenant_id,
                    &w.repository.project_id,
                    &w.repository.work_id,
                    &w.repository.connector_id,
                )
            })
            .chain(github.iter().flat_map(|c| &c.workers).map(|w| {
                (
                    &w.repository.tenant_id,
                    &w.repository.project_id,
                    &w.repository.work_id,
                    &w.repository.connector_id,
                )
            }))
        {
            if !scopes.insert(repo) {
                return Err("duplicate delivery observer scope".into());
            }
        }
        let store = Arc::new(store);
        let (source, git, github) = tokio::time::timeout(Duration::from_secs(30), async {
            let source = DeliveryWorkerRuntime::admit(
                service,
                source,
                store.clone(),
                &mut credential_lookup,
            )
            .await?;
            let git =
                LocalGitWorkerRuntime::admit(service, git, store.clone(), &mut credential_lookup)
                    .await?;
            let github = GitHubWorkerRuntime::admit(
                service,
                github,
                store,
                &mut credential_lookup,
                &mut transports,
            )
            .await?;
            Ok::<_, String>((source, git, github))
        })
        .await
        .map_err(|_| "delivery worker startup checks timed out".to_string())??;
        Ok(Self {
            source: source.spawn(),
            git: git.spawn(),
            github: github.spawn(),
        })
    }
    pub fn source_monitor(&self) -> DeliveryWorkerMonitor {
        self.source.monitor()
    }
    pub fn git_monitor(&self) -> LocalGitWorkerMonitor {
        self.git.monitor()
    }
    pub fn github_monitor(&self) -> GitHubWorkerMonitor {
        self.github.monitor()
    }
    pub fn request_stop(&self) {
        self.source.request_stop();
        self.git.request_stop();
        self.github.request_stop();
    }
    pub async fn shutdown(self) {
        self.request_stop();
        tokio::join!(
            self.source.shutdown(),
            self.git.shutdown(),
            self.github.shutdown()
        );
    }
}

impl From<DeliveryWorkerRuntime> for DeliveryWorkers {
    fn from(source: DeliveryWorkerRuntime) -> Self {
        Self {
            source,
            git: LocalGitWorkerRuntime::empty(),
            github: GitHubWorkerRuntime::empty(),
        }
    }
}
