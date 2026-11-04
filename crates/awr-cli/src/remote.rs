use awr_core::{Error, Result};
use awr_team::{RemoteProfile, TeamError};
use clap::Subcommand;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Store endpoint, project key and credential env reference. Does not store secrets.
    Add {
        name: String,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        project_key: String,
        #[arg(long)]
        credential_env: String,
    },
    Inspect {
        name: String,
    },
    Remove {
        name: String,
    },
}

pub fn run(project: &Path, command: &RemoteCommand, json: bool) -> Result<()> {
    match command {
        RemoteCommand::Add {
            name,
            endpoint,
            project_key,
            credential_env,
        } => {
            let profile = RemoteProfile {
                name: name.clone(),
                endpoint: endpoint.clone(),
                project_key: project_key.clone(),
                credential_env: credential_env.clone(),
                protocol_version: 1,
            };
            profile.validate().map_err(team_err)?;
            let path = profile_path(project, name)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let toml = toml::to_string(&profile)
                .map_err(|_| Error::InvalidInput("remote serialization failed".into()))?;
            fs::write(&path, toml)?;
            if json {
                println!("{}", profile.redacted());
            }
            Ok(())
        }
        RemoteCommand::Inspect { name } => {
            let profile = load_profile(project, name)?;
            println!("{}", profile.redacted());
            let _ = json;
            Ok(())
        }
        RemoteCommand::Remove { name } => {
            let path = profile_path(project, name)?;
            if path.exists() {
                fs::remove_file(path)?;
            }
            Ok(())
        }
    }
}

pub fn load_profile(project: &Path, name: &str) -> Result<RemoteProfile> {
    let path = profile_path(project, name)?;
    let raw =
        fs::read_to_string(&path).map_err(|_| Error::NotFound(format!("team remote {name}")))?;
    let table = raw
        .parse::<toml::Table>()
        .map_err(|_| Error::InvalidInput("invalid remote TOML".into()))?;
    let required = |key: &str| {
        table
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .ok_or_else(|| Error::InvalidInput(format!("{key} required")))
    };
    let profile = RemoteProfile {
        name: name.into(),
        endpoint: required("endpoint")?,
        project_key: required("project_key")?,
        credential_env: required("credential_env")?,
        protocol_version: table
            .get("protocol_version")
            .and_then(|v| v.as_integer())
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| team_err(TeamError::ProtocolUnsupported))?,
    };
    profile.validate().map_err(team_err)?;
    Ok(profile)
}

pub fn team_err(error: TeamError) -> Error {
    match error {
        TeamError::Unsupported => Error::Unsupported(
            "Team command transport/dispatch is not implemented; nothing was submitted".into(),
        ),
        TeamError::InvalidInput(message) => Error::InvalidInput(message),
        TeamError::ProtocolUnsupported => Error::ProtocolUnsupported {
            requested: 0,
            supported: vec![1],
        },
        TeamError::OfflineWriteForbidden => Error::Unsupported(
            "team remote required; local coordination cannot take over team authority".into(),
        ),
        TeamError::ProjectRequired => {
            Error::InvalidInput("explicit team project key required".into())
        }
        TeamError::AuthProjectMismatch => {
            Error::RuleViolation("request body cannot override the authorized team project".into())
        }
        TeamError::SecretRefInvalid => Error::RuleViolation(
            "team remotes store credential environment names, not secrets or database URLs".into(),
        ),
        other => Error::InvalidInput(other.to_string()),
    }
}

pub(crate) fn remote_name(value: &str) -> std::result::Result<String, String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        Err(
            "remote name must be nonempty and contain only ASCII letters, digits, '-' or '_'"
                .into(),
        )
    } else {
        Ok(value.to_owned())
    }
}

fn profile_path(project: &Path, name: &str) -> Result<PathBuf> {
    remote_name(name).map_err(Error::InvalidInput)?;
    Ok(project
        .join(".awr/team-remotes")
        .join(format!("{name}.toml")))
}
