use super::LocalGitError;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

pub(super) struct GitProcess {
    pub executable: PathBuf,
    pub repository: PathBuf,
    pub timeout_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_reader_enforces_exact_byte_bound() {
        assert_eq!(
            read_limited(std::io::Cursor::new(b"abcd"), 4)
                .await
                .unwrap(),
            b"abcd"
        );
        assert!(matches!(
            read_limited(std::io::Cursor::new(b"abcde"), 4).await,
            Err(LocalGitError::OutputLimit)
        ));
        assert_eq!(
            read_limited(std::io::Cursor::new(b""), 0).await.unwrap(),
            b""
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn configured_process_timeout_and_stderr_never_become_success_or_diagnostics() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("awr-git-process-{}", awr_core::Id::new()));
        std::fs::create_dir(&root).unwrap();
        let executable = root.join("synthetic-git");
        std::fs::write(&executable, "#!/bin/sh\nprintf 0123456789\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut git = GitProcess {
            executable: executable.clone(),
            repository: root.clone(),
            timeout_ms: 3000,
        };
        let result = git.run(&[], 4).await;
        assert!(
            matches!(result, Err(LocalGitError::OutputLimit)),
            "unexpected finite error: {:?}",
            result.err()
        );
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf synthetic-private-stderr >&2\nexit 1\n",
        )
        .unwrap();
        assert_eq!(git.text(&[]).await.unwrap(), None);
        std::fs::write(&executable, "#!/bin/sh\nexec sleep 2\n").unwrap();
        git.timeout_ms = 100;
        let started = std::time::Instant::now();
        assert!(matches!(git.text(&[]).await, Err(LocalGitError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(1));
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub(super) struct Output {
    pub code: i32,
    pub bytes: Vec<u8>,
}

async fn read_limited(
    pipe: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, LocalGitError> {
    let mut bytes = Vec::new();
    pipe.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| LocalGitError::RepositoryUnavailable)?;
    if bytes.len() > limit {
        return Err(LocalGitError::OutputLimit);
    }
    Ok(bytes)
}

impl GitProcess {
    /// Only fixed Git subcommands with separately validated object/ref/path arguments.
    /// No shell, hooks, working-tree filters, replacement objects or lazy network fetches.
    pub async fn run(&self, args: &[&str], limit: usize) -> Result<Output, LocalGitError> {
        let mut command = Command::new(&self.executable);
        command
            .arg("--no-replace-objects")
            .arg("--literal-pathspecs")
            // The process cwd is the already canonicalized, operator-bound bare
            // repository. Windows canonical paths use a verbatim prefix that
            // Git refuses as an argument; a fixed relative Git directory keeps
            // the same scope without rewriting or weakening the validated path.
            .arg("--git-dir=.")
            .args(args)
            .current_dir(&self.repository)
            .env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Git's platform launcher may require PATH, but none of the supported
        // commands dispatch helpers. All ambient GIT_* and credential values are cleared.
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let mut child = command
            .spawn()
            .map_err(|_| LocalGitError::RepositoryUnavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(LocalGitError::RepositoryUnavailable)?;
        let stderr = child
            .stderr
            .take()
            .ok_or(LocalGitError::RepositoryUnavailable)?;
        let operation = async {
            let (bytes, _, status) = tokio::try_join!(
                read_limited(stdout, limit),
                read_limited(stderr, 16384),
                async {
                    child
                        .wait()
                        .await
                        .map_err(|_| LocalGitError::RepositoryUnavailable)
                }
            )?;
            Ok(Output {
                code: status.code().unwrap_or(-1),
                bytes,
            })
        };
        // Raw Git stderr is never emitted or included in an error/report.
        tokio::time::timeout(Duration::from_millis(self.timeout_ms), operation)
            .await
            .map_err(|_| LocalGitError::TimedOut)?
    }

    pub async fn text(&self, args: &[&str]) -> Result<Option<String>, LocalGitError> {
        let output = self.run(args, 4096).await?;
        if output.code != 0 {
            return Ok(None);
        }
        String::from_utf8(output.bytes)
            .map(|s| Some(s.trim_end().to_owned()))
            .map_err(|_| LocalGitError::RepositoryUnavailable)
    }
}
