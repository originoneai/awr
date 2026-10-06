//! Optional server-owned delivery workers. Credentials and source paths are not configuration data.
use crate::service::ServiceConfig;
use awr_core::Id;
use serde::Deserialize;
use std::{collections::BTreeSet, io::Read, path::Path};

pub const DELIVERY_WORKER_CONFIG_ENV: &str = "AWR_TEAM_DELIVERY_WORKER_CONFIG";
pub const MAX_DELIVERY_WORKERS: usize = 16;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryWorkerConfig {
    pub version: u32,
    #[serde(default)]
    pub workers: Vec<DeliveryWorkerSpec>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryWorkerSpec {
    pub project: String,
    pub worker_id: String,
    pub workstreams: Vec<Id>,
    /// Environment-variable name, never its value. Rotation requires an explicit restart.
    pub credential_env: String,
    #[serde(default = "poll_ms")]
    pub poll_interval_ms: u64,
    #[serde(default = "lease_seconds")]
    pub lease_seconds: i32,
    #[serde(default = "operation_ms")]
    pub operation_timeout_ms: u64,
    #[serde(default = "backoff_ms")]
    pub max_backoff_ms: u64,
    #[serde(default = "page_size")]
    pub page_size: u16,
    #[serde(default = "pages")]
    pub max_pages_per_poll: u16,
    #[serde(default = "jobs")]
    pub max_jobs_per_poll: u16,
}

fn poll_ms() -> u64 {
    1000
}
fn lease_seconds() -> i32 {
    60
}
fn operation_ms() -> u64 {
    15000
}
fn backoff_ms() -> u64 {
    60000
}
fn page_size() -> u16 {
    16
}
fn pages() -> u16 {
    4
}
fn jobs() -> u16 {
    16
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

impl DeliveryWorkerConfig {
    pub fn from_environment(service: &ServiceConfig) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os(DELIVERY_WORKER_CONFIG_ENV) else {
            return Ok(None);
        };
        Self::read(Path::new(&path), service).map(Some)
    }

    pub fn read(path: &Path, service: &ServiceConfig) -> Result<Self, String> {
        let mut text = String::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(65537).read_to_string(&mut text))
            .map_err(|_| "could not read delivery worker configuration".to_string())?;
        if text.len() > 65536 {
            return Err("delivery worker configuration is too large".into());
        }
        let config: Self = toml::from_str(&text)
            .map_err(|_| "invalid delivery worker configuration".to_string())?;
        config.validate(service)?;
        Ok(config)
    }

    pub fn validate(&self, service: &ServiceConfig) -> Result<(), String> {
        service.validate()?;
        if self.version != 1 || self.workers.len() > MAX_DELIVERY_WORKERS {
            return Err("unsupported delivery worker version or count".into());
        }
        let mut workers = BTreeSet::new();
        for worker in &self.workers {
            if !identifier(&worker.worker_id)
                || !workers.insert(&worker.worker_id)
                || !service.projects.iter().any(|p| p.key == worker.project)
                || worker.workstreams.is_empty()
                || worker.workstreams.len() > 32
                || worker.workstreams.iter().collect::<BTreeSet<_>>().len()
                    != worker.workstreams.len()
            {
                return Err("invalid worker identity, project or workstream scopes".into());
            }
            let variable = worker.credential_env.as_bytes();
            if !(1..=128).contains(&variable.len())
                || !variable[0].is_ascii_uppercase() && variable[0] != b'_'
                || !variable
                    .iter()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
            {
                return Err("worker credentials require an environment-variable name".into());
            }
            if !(100..=60000).contains(&worker.poll_interval_ms)
                || !(5..=300).contains(&worker.lease_seconds)
                || !(100..=60000).contains(&worker.operation_timeout_ms)
                || worker.operation_timeout_ms >= worker.lease_seconds as u64 * 1000
                || !(worker.poll_interval_ms..=300000).contains(&worker.max_backoff_ms)
                || !(1..=64).contains(&worker.page_size)
                || !(1..=16).contains(&worker.max_pages_per_poll)
                || !(1..=64).contains(&worker.max_jobs_per_poll)
            {
                return Err("delivery worker limits are outside supported bounds".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ProjectBinding;

    fn service() -> ServiceConfig {
        ServiceConfig {
            version: 1,
            listen: "127.0.0.1:0".parse().unwrap(),
            allowed_hosts: vec![],
            allowed_web_origins: vec![],
            oauth: None,
            projects: vec![ProjectBinding {
                key: "one".into(),
                tenant_id: "tenant".into(),
                project_id: "project".into(),
            }],
        }
    }

    fn text() -> &'static str {
        r#"version = 1
[[workers]]
project = "one"
worker_id = "delivery-one"
workstreams = ["00000000000000000000000001"]
credential_env = "AWR_WORKER_CREDENTIAL"
"#
    }

    #[test]
    fn optional_file_and_defaults_do_not_change_service_config() {
        let empty: DeliveryWorkerConfig = toml::from_str("version=1").unwrap();
        empty.validate(&service()).unwrap();
        let config: DeliveryWorkerConfig = toml::from_str(text()).unwrap();
        config.validate(&service()).unwrap();
        assert_eq!(config.workers[0].poll_interval_ms, 1000);
        assert_eq!(config.workers[0].lease_seconds, 60);
        assert_eq!(config.workers[0].max_pages_per_poll, 4);
    }

    #[test]
    fn identity_scope_credentials_and_limits_are_explicit() {
        let original: DeliveryWorkerConfig = toml::from_str(text()).unwrap();
        for mutate in [
            |c: &mut DeliveryWorkerConfig| c.version = 2,
            |c: &mut DeliveryWorkerConfig| c.workers[0].project = "missing".into(),
            |c: &mut DeliveryWorkerConfig| c.workers[0].worker_id = "worker/private".into(),
            |c: &mut DeliveryWorkerConfig| c.workers[0].workstreams.clear(),
            |c: &mut DeliveryWorkerConfig| c.workers[0].workstreams.push(Id::from(1)),
            |c: &mut DeliveryWorkerConfig| c.workers.push(c.workers[0].clone()),
            |c: &mut DeliveryWorkerConfig| c.workers[0].credential_env = "raw-secret-value".into(),
            |c: &mut DeliveryWorkerConfig| c.workers[0].credential_env = "1INVALID".into(),
            |c: &mut DeliveryWorkerConfig| c.workers[0].lease_seconds = 301,
            |c: &mut DeliveryWorkerConfig| c.workers[0].operation_timeout_ms = 60000,
            |c: &mut DeliveryWorkerConfig| c.workers[0].poll_interval_ms = 99,
            |c: &mut DeliveryWorkerConfig| c.workers[0].max_backoff_ms = 999,
            |c: &mut DeliveryWorkerConfig| c.workers[0].page_size = 65,
            |c: &mut DeliveryWorkerConfig| c.workers[0].max_pages_per_poll = 17,
            |c: &mut DeliveryWorkerConfig| c.workers[0].max_jobs_per_poll = 0,
            |c: &mut DeliveryWorkerConfig| c.workers = vec![c.workers[0].clone(); 17],
        ] {
            let mut config = original.clone();
            mutate(&mut config);
            assert!(config.validate(&service()).is_err());
        }
        for extra in [
            "token='synthetic'",
            "tenant_id='other'",
            "role='admin'",
            "source_path='/tmp/x'",
        ] {
            assert!(
                toml::from_str::<DeliveryWorkerConfig>(&format!("{}{extra}\n", text())).is_err()
            );
        }
    }
}
