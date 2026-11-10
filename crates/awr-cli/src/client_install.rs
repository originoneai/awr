use awr_core::{Error, Id, Result};
use awr_runtime::host_adapter::installation::{self, ADAPTER_VERSION, CLAUDE_WRITE_MATCHER};
use awr_source::{GatePolicy, WriteGate};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

struct Configuration {
    path: PathBuf,
    original: Option<Vec<u8>>,
    value: Value,
}

fn remove_managed(config: &mut Value, client: &str) {
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    let mut changed = false;
    hooks.retain(|_, groups| {
        let groups = groups.as_array_mut().unwrap();
        let before_groups = groups.len();
        groups.retain_mut(|group| {
            let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = hooks.len();
            hooks.retain(|hook| installation::managed_hook(hook, client).is_none());
            changed |= before != hooks.len();
            // Retain pre-existing empty user groups; remove only groups emptied by us.
            before == hooks.len() || !hooks.is_empty()
        });
        before_groups == groups.len() || !groups.is_empty()
    });
    if changed && hooks.is_empty() {
        config.as_object_mut().unwrap().remove("hooks");
    }
}

fn quote(value: &str, powershell: bool) -> String {
    format!(
        "'{}'",
        if powershell {
            value.replace('\'', "''")
        } else {
            value.replace('\'', "'\"'\"'")
        }
    )
}

fn add_managed(root: &Path, config: &mut Value, client: &str, shared: bool) -> Result<()> {
    let powershell = cfg!(windows) && client == "codex";
    let command = if shared {
        format!(
            "awr --project \"$CLAUDE_PROJECT_DIR\" client hook --client {client} --adapter-version {ADAPTER_VERSION}"
        )
    } else {
        let exe = std::env::current_exe()?.canonicalize()?;
        format!(
            "{}{} --project {} client hook --client {client} --adapter-version {ADAPTER_VERSION}",
            if powershell { "& " } else { "" },
            quote(&exe.to_string_lossy(), powershell),
            quote(&root.to_string_lossy(), powershell)
        )
    };
    let policy = GatePolicy::load(root)?;
    let hooks = config
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();
    let mut events = vec!["SessionStart", "PreCompact", "Stop", "SessionEnd"];
    if client == "codex" {
        events.extend(["PostCompact", "Interrupt"]);
    }
    if policy.hooks.write_gate != WriteGate::Off {
        events.push("PreToolUse");
    }
    for event in events {
        let mut group = json!({"hooks":[{"type":"command","command":command,"timeout":if ["SessionEnd","Interrupt"].contains(&event){3}else{15},"statusMessage":"AWR work continuity"}]});
        if event == "PreToolUse" {
            group["matcher"] = json!(if client == "claude" {
                CLAUDE_WRITE_MATCHER
            } else {
                "Edit|Write|MultiEdit|NotebookEdit|apply_patch"
            });
        }
        hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(group);
    }
    Ok(())
}

pub fn change(
    root: &Path,
    client: &str,
    shared: bool,
    uninstall: bool,
    accept: bool,
    expected: Option<&str>,
    work_ignored: bool,
) -> Result<Value> {
    let paths = installation::configuration_paths(client)?;
    // Keep the existing initialized-project boundary without refreshing sources.
    awr_store::Store::open_readonly(&crate::source::runtime_dir(root, false)?.join("state.db"))?
        .project_by_root(root)?;
    if shared && client != "claude" {
        return Err(Error::InvalidInput(
            "--shared applies to the claude project adapter".into(),
        ));
    }
    if expected.is_some_and(|value| !awr_core::is_sha256_hash(value)) {
        return Err(Error::InvalidInput(
            "--expected-preview requires 64 hexadecimal characters".into(),
        ));
    }
    let target = if shared { 1 } else { 0 };
    let selected = if uninstall {
        paths.to_vec()
    } else {
        vec![paths[target]]
    };
    let mut configurations = vec![];
    let preview_error = |error| {
        if expected.is_some() {
            Error::SourceConflict(
                "hook configuration cannot reproduce the reviewed preview; preview again".into(),
            )
        } else {
            error
        }
    };
    for relative in selected {
        let path = root.join(relative);
        let original = installation::read_configuration(&path).map_err(preview_error)?;
        let mut value =
            installation::parse_configuration(original.as_deref()).map_err(preview_error)?;
        remove_managed(&mut value, client);
        if !uninstall {
            add_managed(root, &mut value, client, shared)?;
        }
        configurations.push(Configuration {
            path,
            original,
            value,
        });
    }
    // Bind the exact source bytes (including absence), operation, destination and
    // proposed output, so changes in settings, policy or executable invalidate review.
    let basis = configurations
        .iter()
        .map(|c| json!({"path":c.path,"original":c.original,"configuration":c.value}))
        .collect::<Vec<_>>();
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(
            &json!({"client":client,"uninstall":uninstall,"files":basis})
        )?)
    );
    if expected.is_some_and(|expected| expected != fingerprint) {
        return Err(Error::SourceConflict(
            "hook configuration or proposed adapter changed since preview; preview again".into(),
        ));
    }
    if accept {
        let _lock = crate::client::lock(root, "clients", "hook-install")?;
        for config in &configurations {
            if installation::read_configuration(&config.path)? != config.original {
                return Err(Error::SourceConflict(
                    "hook configuration changed during installation".into(),
                ));
            }
        }
        for config in &configurations {
            // Uninstalling an absent adapter leaves absent settings absent.
            if uninstall && config.original.is_none() {
                continue;
            }
            if config
                .original
                .as_ref()
                .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                .as_ref()
                == Some(&config.value)
            {
                continue;
            }
            let parent = config.path.parent().unwrap();
            let root_dir = awr_source::open_dir_exact(root)?;
            if let Err(e) = root_dir.create_dir(parent.file_name().unwrap()) {
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(e.into());
                }
            }
            let dir = awr_source::open_dir_exact(parent)?;
            let temporary = format!("awr-hooks-{}.json", Id::new());
            let mut options = cap_std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            let mut file = dir.open_with(&temporary, &options)?;
            let result = (|| -> Result<()> {
                file.write_all(&serde_json::to_vec_pretty(&config.value)?)?;
                file.sync_all()?;
                if installation::read_configuration(&config.path)? != config.original {
                    return Err(Error::SourceConflict(
                        "hook configuration changed during installation".into(),
                    ));
                }
                dir.rename(&temporary, &dir, config.path.file_name().unwrap())?;
                Ok(())
            })();
            if result.is_err() {
                let _ = dir.remove_file(&temporary);
            }
            result?;
        }
    }
    Ok(
        json!({"installed":accept && !uninstall,"uninstalled":accept && uninstall,"path":configurations[0].path,"configuration":configurations[0].value,
        "configurations":configurations.iter().map(|c|json!({"path":c.path,"configuration":c.value})).collect::<Vec<_>>(),
        "preview_fingerprint":fingerprint,"work_ignored":work_ignored,"activation_verified":false,
        "next_action":"Inspect client status, then review hooks in a fresh trusted client session. Receiver observations and native client acceptance are separate."}),
    )
}

pub fn status(root: &Path, client: Option<&str>, json_output: bool) -> Result<()> {
    let adapters = installation::inspect_host_adapters(root, client)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"adapters":adapters,"read_only":true,"source_refresh_performed":false})
            )?
        );
    } else {
        for adapter in adapters {
            println!(
                "{}: installed: {}; outdated: {}; paths: {}; adapter version: {}; program exists: {}; events: {}; gates consistent: {}; write gate: {}; completion reminder: {}; activation: {}{}",
                adapter.client,
                adapter.installed,
                adapter.outdated,
                if adapter.paths.is_empty() {
                    "none".into()
                } else {
                    adapter
                        .paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                adapter
                    .adapter_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "none".into()),
                adapter.program_exists,
                adapter.events.into_iter().collect::<Vec<_>>().join(", "),
                adapter.gates_consistent,
                adapter.write_gate,
                adapter.completion_reminder,
                adapter.activation,
                adapter
                    .observed_at
                    .map(|t| format!(" at {t} (Unix milliseconds)"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}
