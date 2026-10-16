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
            read_limited(std::io::Cursor::new(b"abcd"), 4, "", "read_stdout")
                .await
                .unwrap(),
            b"abcd"
        );
        assert!(matches!(
            read_limited(std::io::Cursor::new(b"abcde"), 4, "", "read_stdout").await,
            Err(LocalGitError::OutputLimit)
        ));
        assert_eq!(
            read_limited(std::io::Cursor::new(b""), 0, "", "read_stdout")
                .await
                .unwrap(),
            b""
        );
    }

    #[test]
    fn process_diagnostics_retain_stage_and_os_code_without_private_values() {
        let error = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private repository path and credential",
        );
        assert_eq!(
            process_diagnostic("private argument", "spawn", Some(&error)),
            "local Git process failed: command=other, stage=spawn, kind=Some(PermissionDenied), os_code=None"
        );
        let error = std::io::Error::from_raw_os_error(2);
        let diagnostic = process_diagnostic("--version", "spawn", Some(&error));
        assert!(diagnostic.contains("command=version, stage=spawn"));
        assert!(diagnostic.contains("os_code=Some(2)"));
        assert_eq!(
            process_diagnostic("cat-file", "decode_utf8", None),
            "local Git process failed: command=cat-file, stage=decode_utf8, kind=None, os_code=None"
        );
    }

    #[tokio::test]
    async fn failed_spawn_remains_repository_unavailable() {
        let git = GitProcess {
            executable: std::env::temp_dir().join(format!("missing-git-{}", awr_core::Id::new())),
            repository: std::env::temp_dir(),
            timeout_ms: 100,
        };
        assert!(matches!(
            git.text(&["--version"]).await,
            Err(LocalGitError::RepositoryUnavailable)
        ));
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
        std::fs::write(&executable, "#!/bin/sh\nprintf '\\377'\n").unwrap();
        assert!(matches!(
            git.text(&[]).await,
            Err(LocalGitError::RepositoryUnavailable)
        ));
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

// Keep the public finite error contract while retaining safe operational evidence.
// Display/Debug on io::Error can include configured paths; never print it or Git stderr.
fn process_diagnostic(command: &str, stage: &str, error: Option<&std::io::Error>) -> String {
    // Do not emit arbitrary argv, paths, refs, object IDs, or provider output.
    let command = match command {
        "--version" => "version",
        "rev-parse" => "rev-parse",
        "show-ref" => "show-ref",
        "symbolic-ref" => "symbolic-ref",
        "cat-file" => "cat-file",
        "ls-tree" => "ls-tree",
        "merge-base" => "merge-base",
        "update-ref" => "update-ref",
        _ => "other",
    };
    format!(
        "local Git process failed: command={command}, stage={stage}, kind={:?}, os_code={:?}",
        error.map(std::io::Error::kind),
        error.and_then(std::io::Error::raw_os_error)
    )
}

fn process_error(command: &str, stage: &str, error: Option<&std::io::Error>) -> LocalGitError {
    eprintln!("{}", process_diagnostic(command, stage, error));
    LocalGitError::RepositoryUnavailable
}

async fn read_limited(
    pipe: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
    command: &str,
    stage: &str,
) -> Result<Vec<u8>, LocalGitError> {
    let mut bytes = Vec::new();
    pipe.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| process_error(command, stage, Some(&error)))?;
    if bytes.len() > limit {
        return Err(LocalGitError::OutputLimit);
    }
    Ok(bytes)
}

impl GitProcess {
    /// Only fixed Git subcommands with separately validated object/ref/path arguments.
    /// No shell, hooks, working-tree filters, replacement objects or lazy network fetches.
    pub async fn run(&self, args: &[&str], limit: usize) -> Result<Output, LocalGitError> {
        let subcommand = args.first().copied().unwrap_or("");
        let mut command = Command::new(&self.executable);
        command
            .arg("--no-replace-objects")
            .arg("--literal-pathspecs")
            // The process cwd is the already canonicalized, operator-bound bare
            // repository. Windows canonical paths use a verbatim prefix that
            // Git refuses as an argument; a fixed relative Git directory keeps
            // the same scope without rewriting or weakening the validated path.
            .arg("--git-dir=.")
            .arg("-c")
            .arg(if cfg!(windows) {
                "core.hooksPath=NUL"
            } else {
                "core.hooksPath=/dev/null"
            })
            .arg("-c")
            .arg("core.fsync=reference")
            .arg("-c")
            .arg("core.fsyncMethod=fsync")
            .args(args)
            .current_dir(&self.repository)
            .env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env(
                "GIT_GRAFT_FILE",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
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
            .map_err(|error| process_error(subcommand, "spawn", Some(&error)))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| process_error(subcommand, "missing_stdout", None))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| process_error(subcommand, "missing_stderr", None))?;
        let operation = async {
            let (bytes, _, status) = tokio::try_join!(
                read_limited(stdout, limit, subcommand, "read_stdout"),
                read_limited(stderr, 16384, subcommand, "read_stderr"),
                async {
                    child
                        .wait()
                        .await
                        .map_err(|error| process_error(subcommand, "wait", Some(&error)))
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
            .map_err(|_| process_error(args.first().copied().unwrap_or(""), "decode_utf8", None))
    }
}
