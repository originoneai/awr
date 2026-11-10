# Claude Code lifecycle hooks

Connect Claude Code project events to AWR context, checkpoints and optional write gates. Preview settings, install the adapter, inspect receiver observations, and remove the managed entries while preserving other project settings.

## Use it

Start in an initialized AWR project. Preview the configuration:

```sh
awr client install --client claude
```

Read the proposed `configuration`, destination `path` and `preview_fingerprint`.
Accept that exact proposal by replacing `PREVIEW_SHA256` with the returned value:

```sh
awr client install --client claude --accept --expected-preview PREVIEW_SHA256
awr client status --client claude
```

Open Claude Code in that project and review the project hooks. At session start,
AWR injects either the bound work's recovery context or a project action summary
with a `client bind` command. Follow that command with the work key you select.
The [session workflow](session-workflow.md) covers claims and explicit handoffs.

For settings shared through version control, use the portable variant:

```sh
awr client install --client claude --shared
awr client install --client claude --shared --accept --expected-preview PREVIEW_SHA256
```

Use the fingerprint from the shared preview. Each receiving machine supplies
`awr` through `PATH`; Claude Code supplies `CLAUDE_PROJECT_DIR` for the root.
See the [shared settings example](../../../examples/claude-code/settings.json.example)
and [local settings example](../../../examples/claude-code/settings.local.json.example).

## How it behaves

| Operation or input | Result |
| --- | --- |
| Default install | Writes `.claude/settings.local.json` with absolute program and project paths. |
| `--shared` install | Writes `.claude/settings.json` using `awr` and `"$CLAUDE_PROJECT_DIR"`. |
| Preview | Returns the complete proposed settings and a SHA-256 fingerprint; settings stay as read. |
| `--accept` | Merges the proposal using an in-call source check. |
| `--expected-preview SHA256` | Also compares the exact source bytes and proposal with the earlier preview; a change returns `SourceConflict`. |
| Repeated install | Replaces matching managed entries with one current entry per event. |
| Existing configuration | Preserves parsed values of other settings, groups and handlers, including handlers sharing a managed group. |
| `--work KEY` on install | Returns `work_ignored: true`; installation covers the project. |
| Uninstall | Removes recognized Claude entries from both project settings files and cleans arrays emptied by removal. |

The command stored in each handler ends with
`client hook --client claude --adapter-version 1`. Its version identifies managed
entries and is recorded with receiver observations. Existing user hook commands
retain their parsed content when settings are serialized.

| Claude Code event | AWR behavior |
| --- | --- |
| `SessionStart` with `startup`, `resume` or `clear` | Injects recovery context or binding guidance. |
| `SessionStart` with `compact` | Uses the post-compaction recovery path. |
| `PreCompact` | Checkpoints saved continuity for a bound conversation. |
| `Stop` | Checkpoints continuity; the optional completion reminder uses the existing completion gate. |
| `SessionEnd` | Saves a checkpoint for a bound conversation. |
| `PreToolUse` | Installed when `write_gate` is enabled; matches `Edit|Write|MultiEdit|NotebookEdit`. |

Configure the optional gates in `.awr/gates.toml`:

```toml
version = 1
[hooks]
write_gate = "ask"
completion_reminder = false
```

Reinstall after changing `write_gate` so event coverage follows the policy.
See [hook decisions](../reference/host-contract.md#hook-decisions) for claim,
path and reminder decisions, and the `AWR_HOOK_GATES=off` override.

## Inspect and remove

`awr client status --json` reports both supported hosts. Filter with
`--client claude` to read installation paths, adapter version, program existence,
event coverage and consistency with the project gate policy. `doctor` shares
the inspection code and advises about missing, outdated or inactive adapters.

Receiver state starts at `activation: not observed`. A successfully handled
simulated or native event records its time, event name and supplied adapter
version under `.awr/clients/`. The timestamp uses Unix milliseconds. These
small runtime receipts stay separate from work bindings and the gate event
stream, and status inspection reads them alongside the settings.

Preview removal, then accept its returned fingerprint:

```sh
awr client uninstall --client claude
awr client uninstall --client claude --accept --expected-preview PREVIEW_SHA256
```

## Limits

- Verification: **simulated events verified**. Native Claude Code verification is **unexecuted, awaiting approval** of client version, isolated project and project-setting writes.
- Scope: project settings. User-home global settings remain operator-managed; configuration serialization may normalize whitespace and key order.
- Observation: receiver timestamps establish event delivery, not native activation, completed work or business acceptance.
- Privacy: transcript paths are ignored; recovery uses explicitly recorded AWR facts.

## Code and tests

- [Installer and removal](../../../crates/awr-cli/src/client_install.rs)
- [Shared status inspection](../../../crates/awr-runtime/src/host_adapter/installation.rs)
- [Installation acceptance tests](../../../crates/awr-cli/tests/client_install.rs)
- [Simulated Claude protocol tests](../../../crates/awr-cli/tests/client_claude.rs)
