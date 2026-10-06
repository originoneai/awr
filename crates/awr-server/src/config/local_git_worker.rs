//! Explicit optional repository workers; configuration never grants authority.
use super::*;
use crate::delivery_adapter::LocalGitConfig;

pub const LOCAL_GIT_WORKER_CONFIG_ENV: &str = "AWR_TEAM_LOCAL_GIT_WORKER_CONFIG";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitWorkerConfig {
    pub version: u32,
    #[serde(default)]
    pub workers: Vec<LocalGitWorkerSpec>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitWorkerSpec {
    pub project: String,
    pub worker_id: String,
    /// Name only. The value is privately resolved once; rotation requires restart.
    pub credential_env: String,
    pub integration_enabled: bool,
    pub repository: LocalGitConfig,
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

impl LocalGitWorkerConfig {
    pub fn from_environment(service: &ServiceConfig) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os(LOCAL_GIT_WORKER_CONFIG_ENV) else {
            return Ok(None);
        };
        Self::read(Path::new(&path), service).map(Some)
    }

    pub fn read(path: &Path, service: &ServiceConfig) -> Result<Self, String> {
        let mut text = String::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(65537).read_to_string(&mut text))
            .map_err(|_| "could not read local Git worker configuration".to_string())?;
        if text.len() > 65536 {
            return Err("local Git worker configuration is too large".into());
        }
        let config: Self = toml::from_str(&text)
            .map_err(|_| "invalid local Git worker configuration".to_string())?;
        config.validate(service)?;
        Ok(config)
    }

    pub fn validate(&self, service: &ServiceConfig) -> Result<(), String> {
        service.validate()?;
        if self.version != 1 || self.workers.len() > MAX_DELIVERY_WORKERS {
            return Err("unsupported local Git worker version or count".into());
        }
        let mut names = BTreeSet::new();
        let mut scopes = BTreeSet::new();
        for worker in &self.workers {
            let repo = &worker.repository;
            let binding = service
                .projects
                .iter()
                .find(|p| p.key == worker.project)
                .ok_or_else(|| "local Git worker project is unavailable".to_string())?;
            if !identifier(&worker.worker_id)
                || !names.insert(&worker.worker_id)
                || repo.tenant_id != binding.tenant_id
                || repo.project_id != binding.project_id
                || repo.workstream_id.parse::<Id>().is_err()
                || !scopes.insert((&worker.project, &repo.work_id, &repo.connector_id))
            {
                return Err("invalid local Git worker identity or project scope".into());
            }
            let variable = worker.credential_env.as_bytes();
            if !(1..=128).contains(&variable.len())
                || !variable[0].is_ascii_uppercase() && variable[0] != b'_'
                || !variable
                    .iter()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
            {
                return Err(
                    "local Git worker credentials require an environment-variable name".into(),
                );
            }
            if !(100..=60000).contains(&worker.poll_interval_ms)
                || !(5..=300).contains(&worker.lease_seconds)
                || !(100..=60000).contains(&worker.operation_timeout_ms)
                || worker.operation_timeout_ms >= worker.lease_seconds as u64 * 1000
                || repo.inspection_timeout_ms > worker.operation_timeout_ms
                || !(worker.poll_interval_ms..=300000).contains(&worker.max_backoff_ms)
                || !(1..=32).contains(&worker.page_size)
                || !(1..=16).contains(&worker.max_pages_per_poll)
                || !(1..=64).contains(&worker.max_jobs_per_poll)
            {
                return Err("local Git worker limits are outside supported bounds".into());
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
                key: "team".into(),
                tenant_id: "example-tenant".into(),
                project_id: "example-project".into(),
            }],
        }
    }

    const EXAMPLE: &str = include_str!("../../../../examples/local-git-delivery/worker.toml");

    #[test]
    fn published_example_has_explicit_scope_and_observation_only_mode() {
        let config: LocalGitWorkerConfig = toml::from_str(EXAMPLE).unwrap();
        config.validate(&service()).unwrap();
        assert_eq!(config.workers.len(), 1);
        assert!(!config.workers[0].integration_enabled);
        assert_eq!(
            config.workers[0].credential_env,
            "AWR_LOCAL_GIT_WORKER_CREDENTIAL"
        );
    }

    #[test]
    fn strict_documents_reject_implicit_effect_mode_and_unknown_fields() {
        for text in [
            EXAMPLE.replace("integration_enabled = false\n", ""),
            format!("token = 'synthetic'\n{EXAMPLE}"),
            EXAMPLE.replace("project = \"team\"", "project = \"team\"\nrole = \"admin\""),
            EXAMPLE.replace(
                "[workers.repository]",
                "[workers.repository]\nremote_url = \"https://example.invalid/repo\"",
            ),
        ] {
            assert!(toml::from_str::<LocalGitWorkerConfig>(&text).is_err());
        }
    }

    #[test]
    fn bounded_reader_redacts_invalid_source_and_refuses_oversized_files() {
        let path = std::env::temp_dir().join(format!("awr-git-worker-config-{}", Id::new()));
        for bytes in [vec![b'x'; 65537], b"raw_synthetic_credential".to_vec()] {
            std::fs::write(&path, bytes).unwrap();
            let error = LocalGitWorkerConfig::read(&path, &service()).err().unwrap();
            assert!(!error.contains("raw_synthetic_credential"));
            assert!(!error.contains(path.to_str().unwrap()));
        }
        std::fs::write(&path, EXAMPLE).unwrap();
        LocalGitWorkerConfig::read(&path, &service()).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
