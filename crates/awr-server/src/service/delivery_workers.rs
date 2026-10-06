//! One admission and cancellation boundary for optional source and Git workers.
use super::{
    ServiceConfig,
    delivery_sync::{DeliveryWorkerMonitor, DeliveryWorkerRuntime},
    local_git_worker::{LocalGitWorkerMonitor, LocalGitWorkerRuntime},
};
use crate::config::{DeliveryWorkerConfig, LocalGitWorkerConfig};
use awr_team_pg::DeliverySyncStore;
use std::{sync::Arc, time::Duration};

pub struct DeliveryWorkers {
    source: DeliveryWorkerRuntime,
    git: LocalGitWorkerRuntime,
}

impl DeliveryWorkers {
    /// Both sealed admissions must succeed before either task group can spawn.
    pub async fn start(
        service: &ServiceConfig,
        source: Option<DeliveryWorkerConfig>,
        git: Option<LocalGitWorkerConfig>,
        store: DeliverySyncStore,
        mut credential_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, String> {
        service.validate()?;
        if let Some(config) = &source {
            config.validate(service)?;
        }
        if let Some(config) = &git {
            config.validate(service)?;
        }
        let store = Arc::new(store);
        let (source, git) = tokio::time::timeout(Duration::from_secs(30), async {
            let source = DeliveryWorkerRuntime::admit(
                service,
                source,
                store.clone(),
                &mut credential_lookup,
            )
            .await?;
            let git =
                LocalGitWorkerRuntime::admit(service, git, store, &mut credential_lookup).await?;
            Ok::<_, String>((source, git))
        })
        .await
        .map_err(|_| "delivery worker startup checks timed out".to_string())??;
        Ok(Self {
            source: source.spawn(),
            git: git.spawn(),
        })
    }
    pub fn source_monitor(&self) -> DeliveryWorkerMonitor {
        self.source.monitor()
    }
    pub fn git_monitor(&self) -> LocalGitWorkerMonitor {
        self.git.monitor()
    }
    pub fn request_stop(&self) {
        self.source.request_stop();
        self.git.request_stop();
    }
    pub async fn shutdown(self) {
        self.request_stop();
        tokio::join!(self.source.shutdown(), self.git.shutdown());
    }
}

impl From<DeliveryWorkerRuntime> for DeliveryWorkers {
    fn from(source: DeliveryWorkerRuntime) -> Self {
        Self {
            source,
            git: LocalGitWorkerRuntime::empty(),
        }
    }
}
