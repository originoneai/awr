//! Explicit optional repository workers; configuration never grants authority.
use super::*;
use crate::delivery_adapter::{GitHubAdapter, GitHubConfig};

pub const GITHUB_WORKER_CONFIG_ENV: &str = "AWR_TEAM_GITHUB_WORKER_CONFIG";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubWorkerConfig {
    pub version: u32,
    #[serde(default)]
    pub workers: Vec<GitHubWorkerSpec>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubWorkerSpec {
    pub project: String,
    pub worker_id: String,
    /// Name only. The value is privately resolved once; rotation requires restart.
    pub credential_env: String,
    /// Explicit effect opt-in. A provider credential alone grants no authority.
    pub integration_enabled: bool,
    pub provider_credential_env: String,
    pub repository: GitHubConfig,
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

impl GitHubWorkerConfig {
    pub fn from_environment(service: &ServiceConfig) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os(GITHUB_WORKER_CONFIG_ENV) else {
            return Ok(None);
        };
        Self::read(Path::new(&path), service).map(Some)
    }

    pub fn read(path: &Path, service: &ServiceConfig) -> Result<Self, String> {
        let mut text = String::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(65537).read_to_string(&mut text))
            .map_err(|_| "could not read GitHub worker configuration".to_string())?;
        if text.len() > 65536 {
            return Err("GitHub worker configuration is too large".into());
        }
        let config: Self =
            toml::from_str(&text).map_err(|_| "invalid GitHub worker configuration".to_string())?;
        config.validate(service)?;
        Ok(config)
    }

    pub fn validate(&self, service: &ServiceConfig) -> Result<(), String> {
        service.validate()?;
        if self.version != 1 || self.workers.len() > MAX_DELIVERY_WORKERS {
            return Err("unsupported GitHub worker version or count".into());
        }
        let mut names = BTreeSet::new();
        let mut scopes = BTreeSet::new();
        for worker in &self.workers {
            let repo = &worker.repository;
            let binding = service
                .projects
                .iter()
                .find(|p| p.key == worker.project)
                .ok_or_else(|| "GitHub worker project is unavailable".to_string())?;
            if !identifier(&worker.worker_id)
                || !names.insert(&worker.worker_id)
                || repo.tenant_id != binding.tenant_id
                || repo.project_id != binding.project_id
                || repo.workstream_id.parse::<Id>().is_err()
                || !scopes.insert((&worker.project, &repo.work_id, &repo.connector_id))
            {
                return Err("invalid GitHub worker identity or project scope".into());
            }
            if worker.credential_env == worker.provider_credential_env {
                return Err(
                    "GitHub and AWR credentials require separate environment-variable names".into(),
                );
            }
            for name in [&worker.credential_env, &worker.provider_credential_env] {
                let variable = name.as_bytes();
                if !(1..=128).contains(&variable.len())
                    || !variable[0].is_ascii_uppercase() && variable[0] != b'_'
                    || !variable
                        .iter()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
                {
                    return Err(
                        "GitHub worker credentials require environment-variable names".into(),
                    );
                }
            }
            // Opens operator-owned configuration without credentials or provider access.
            GitHubAdapter::open(repo.clone(), None)
                .map_err(|_| "invalid GitHub worker adapter configuration".to_string())?;
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
                return Err("GitHub worker limits are outside supported bounds".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ProjectBinding;
    const EXAMPLE: &str = include_str!("../../../../examples/github-delivery/worker.toml");

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
    fn config() -> GitHubWorkerConfig {
        let mut c: GitHubWorkerConfig = toml::from_str(EXAMPLE).unwrap();
        c.workers[0].repository.report_directory = std::env::temp_dir();
        c
    }
    #[test]
    fn published_example_declares_separate_credentials_scope_and_observation_mode() {
        let c = config();
        c.validate(&service()).unwrap();
        assert!(!c.workers[0].integration_enabled);
        assert_ne!(
            c.workers[0].credential_env,
            c.workers[0].provider_credential_env
        );
        for empty in [
            GitHubWorkerConfig {
                version: 1,
                workers: vec![],
            },
            toml::from_str("version=1").unwrap(),
        ] {
            empty.validate(&service()).unwrap();
        }
    }
    #[test]
    fn strict_configuration_refuses_implicit_effect_mode_and_injected_transports() {
        let normalized = EXAMPLE.replace("\r\n", "\n");
        for newline in ["\n", "\r\n"] {
            let example = normalized.replace('\n', newline);
            assert!(toml::from_str::<GitHubWorkerConfig>(&example).is_ok());
            for changed in [
                example.replace("integration_enabled = false", ""),
                example.replace(
                    "provider_credential_env = \"GITHUB_DELIVERY_CREDENTIAL\"",
                    "",
                ),
                format!("token='synthetic'{newline}{example}"),
                example.replace(
                    "project = \"team\"",
                    &format!("project = \"team\"{newline}transport = \"fixture\""),
                ),
                example.replace(
                    "[workers.repository]",
                    &format!("[workers.repository]{newline}principal_actor_id = \"other\""),
                ),
            ] {
                assert_ne!(changed, example);
                assert!(toml::from_str::<GitHubWorkerConfig>(&changed).is_err());
            }
        }
    }
    #[test]
    fn invalid_scopes_credentials_provider_config_and_limits_fail_validation() {
        let original = config();
        for mutate in [
            |c: &mut GitHubWorkerConfig| c.version = 2,
            |c: &mut GitHubWorkerConfig| c.workers[0].project = "missing".into(),
            |c: &mut GitHubWorkerConfig| c.workers[0].worker_id = "private/name".into(),
            |c: &mut GitHubWorkerConfig| {
                c.workers[0].credential_env = "raw-synthetic-credential".into()
            },
            |c: &mut GitHubWorkerConfig| {
                c.workers[0].provider_credential_env = c.workers[0].credential_env.clone()
            },
            |c: &mut GitHubWorkerConfig| c.workers[0].provider_credential_env = "1INVALID".into(),
            |c: &mut GitHubWorkerConfig| c.workers[0].repository.tenant_id = "other".into(),
            |c: &mut GitHubWorkerConfig| c.workers[0].repository.workstream_id = "wrong".into(),
            |c: &mut GitHubWorkerConfig| c.workers[0].repository.repository_id = 0,
            |c: &mut GitHubWorkerConfig| {
                c.workers[0].repository.api_base_url = "http://example.invalid".into()
            },
            |c: &mut GitHubWorkerConfig| c.workers[0].repository.owner = "../other".into(),
            |c: &mut GitHubWorkerConfig| {
                c.workers[0].repository.report_directory = "relative".into()
            },
            |c: &mut GitHubWorkerConfig| c.workers[0].poll_interval_ms = 99,
            |c: &mut GitHubWorkerConfig| c.workers[0].lease_seconds = 301,
            |c: &mut GitHubWorkerConfig| c.workers[0].operation_timeout_ms = 60000,
            |c: &mut GitHubWorkerConfig| c.workers[0].operation_timeout_ms = 11000,
            |c: &mut GitHubWorkerConfig| c.workers[0].max_backoff_ms = 999,
            |c: &mut GitHubWorkerConfig| c.workers[0].page_size = 33,
            |c: &mut GitHubWorkerConfig| c.workers[0].max_pages_per_poll = 17,
            |c: &mut GitHubWorkerConfig| c.workers[0].max_jobs_per_poll = 0,
            |c: &mut GitHubWorkerConfig| c.workers.push(c.workers[0].clone()),
        ] {
            let mut c = original.clone();
            mutate(&mut c);
            assert!(c.validate(&service()).is_err());
        }
        let mut c = original;
        c.workers = vec![c.workers[0].clone(); 17];
        assert!(c.validate(&service()).is_err());
    }
    #[test]
    fn bounded_reader_does_not_echo_raw_invalid_source_or_private_paths() {
        let path = std::env::temp_dir().join(format!("awr-github-worker-config-{}", Id::new()));
        for bytes in [vec![b'x'; 65537], b"raw_synthetic_credential".to_vec()] {
            std::fs::write(&path, bytes).unwrap();
            let error = GitHubWorkerConfig::read(&path, &service()).err().unwrap();
            assert!(!error.contains("raw_synthetic_credential"));
            assert!(!error.contains(path.to_str().unwrap()));
        }
        let directory = std::env::temp_dir().to_string_lossy().replace('\\', "/");
        std::fs::write(
            &path,
            EXAMPLE.replace("/absolute/private/github-reports", &directory),
        )
        .unwrap();
        GitHubWorkerConfig::read(&path, &service()).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
