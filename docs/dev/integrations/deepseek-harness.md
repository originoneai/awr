# Using AWR from DeepSeek Harness

> **Layer: L1 host note.** Session start, claim, compile, checkpoint and resume
> follow the [L0 session workflow](session-workflow.md). This page records
> DeepSeek Harness merge paths, native session IDs and a dated check. See
> [host integration layers](README.md).

Checked against **DeepSeek Harness (DSH) desktop 0.2.2-rc.1** (`com.deepseek.dsh`,
macOS arm64, nightly update feed) on **2026-11-11** and **2026-11-12**. DSH owns
the agent window, native session ID, compaction and sandbox. AWR owns project
facts, claims and checkpoints.

## Use it

Ask the DSH conversation to run an AWR command. DSH's `bash` tool runs it:

```sh
awr --project /absolute/path/to/initialized/project --json session list --active
```

A new desktop conversation starts in permission preset `workspace-write` with
approval policy `ask`. AWR commands that write `.awr/state.db` inside the
workspace ran under that preset without an approval prompt. The L0 steps then
apply unchanged: `session start --claim`, `context compile`, `session
checkpoint`, `session end`.

To use MCP, merge one `@deepseek-ai/dsh-mcp-client` entry into the profile's
`cordis.patch.yml` (`~/.dsh/profiles/<profile>/` for a profile run with
`dsh <profile>`) and restart DSH. The entry also worked in the desktop app's own
profile file after a restart.

```yaml
- insert:
    - id: mcp-awr
      name: "@deepseek-ai/dsh-mcp-client"
      config:
        serverName: awr
        transport: stdio
        command: /absolute/path/to/awr-mcp
        args: ["--project", "/absolute/path/to/initialized/project"]
        cwd: /absolute/path/to/initialized/project
```

Probe the entry before relying on it. Save it as `probe.yml`, add
`failOnStartupError: true` and `reconnect: {enabled: false}` under `config`, and run
`dsh headless --patch probe.yml "say hi"`. A server that cannot connect makes
`dsh` print `1 entry did not activate`. Then call the tool from a conversation:

```json
{"child_tool": "awr_project_status", "arguments": {}}
```

DSH lists it as `mcp__awr__awr_query`. Compare the returned project ID and
revision with `awr status` before any mutation.

## How it behaves

| Topic | DSH behavior | Action |
| --- | --- | --- |
| Configuration | The app has no MCP settings page; servers are `dsh-mcp-client` entries in `cordis.patch.yml` | Merge the entry above and restart |
| Environment | stdio servers start with a scrubbed environment; the npm `awr-mcp` launcher is a Node script that finds no `node` | Set `command` to the native `awr-mcp` binary of the platform package |
| Protocol | The client speaks `2026-07-28`: it sends `server/discover`, then `tools/list`, and requires `ttlMs` and `cacheScope` in the `tools/list` result | Use an `awr-mcp` that sends both fields on `tools/list` (PR #614) |
| Tool names | `mcp__<serverName>__<tool>`, default grouped catalog of eight domain tools | Call `mcp__awr__awr_query` with a `child_tool` |
| Disconnected server | DSH reports `mcp-client(awr): server is disconnected` to the model and retries with backoff | Probe with `failOnStartupError` as above |
| Project scope | One entry serves the one project in its `--project` argument, for every conversation of the profile | Add the entry to a profile that serves that project |
| Compaction | `/compact` and automatic compaction write `compaction/start`, `compaction/summary` and `compaction/end` to the session log. The only token count is DSH's own estimate of the condensed history; no event precedes a compaction | Checkpoint at turn boundaries. Report a compaction with `awr session compaction observe`, `measurement_scope: history_only`, `measurement_basis: estimated` and no `after_tokens` |

## Identity

| DSH | AWR |
| --- | --- |
| Session ID `session-<uuid>`; log at `~/.dsh/sessions/<workspace>/<session ID>/session.v4.jsonl.zstd` | `awr client bind --client generic --external-session dsh:<session ID> --work <KEY>` |
| `dsh headless --session-id <ID>` adopts a stored session and keeps its ID | The same external session ID resolves to the same active AWR session |
| The AWR session ended, the DSH session continues | AWR refuses to bind the same external ID again; bind a new one such as `dsh:<session ID>#2` with `--from-session <ended AWR session>` |
| Fork session creates a new session ID and copies the history up to a cut | The new ID is a new conversation. DSH records no parent: the session header holds `isSeeded` and the inherited event count |

`--from-session` makes a successor and marks the predecessor `interrupted`
(checked on `awr` 0.7.0 built from `main`). Bind a fork of a conversation that is
still running as a new AWR session, and use `--from-session` for a fork whose
parent session has ended. DSH shows the parent's inherited events in the fork, so a host that needs
the parent compares that prefix with its candidate sessions.

## Dated check

| Date | What was called | Result |
| --- | --- | --- |
| 2026-11-11 | Two desktop conversations; the model ran `session list`, `status`, `session start --claim`, `context compile`, `session checkpoint` and `session end` through `bash` | Every command succeeded; `awr` run outside DSH showed the same sessions, claim and revisions |
| 2026-11-11 | Desktop conversation, released `awr-mcp` 0.7.0 over stdio | `tools/list` was rejected for the missing `ttlMs` and `cacheScope`; the server showed as disconnected |
| 2026-11-11 | Desktop conversation, a build of `main` plus the fix commit for that rejection (PR #614 before it merged), no shim | The model called `mcp__awr__awr_query`; the result held the project ID and revision that `awr status` reported |
| 2026-11-12 | `dsh headless --patch` probe with `failOnStartupError`, `awr-mcp` built from `main` at `1880dbda`, then one `mcp__awr__awr_query` call from a local stand-in for the model API | The entry activated; the status result matched `awr status` (same project ID and revision). The released 0.7.0 binary failed the same probe |
| 2026-11-12 | Fork session in the DSH web profile; the fork's session header, `agent/created` payload and event log were read | New session ID, `isSeeded: true`, 22 inherited events, no `parentSession`, `agent/created` source `startup` |
| 2026-11-12 | `awr client bind --client generic --external-session dsh:<ID> --from-session <S>` with `S` active, then with `S` ended | `S` active: `S` became `interrupted` and `session show S` listed the successor. `S` ended: the successor was created |

## Limits

- At the check date the latest release, `awr-mcp` 0.7.0, lacked the `tools/list` fields. Use a build with PR #614 until a release has it.
- Streamable HTTP between DSH and AWR is unchecked.
- A desktop model turn with a build of `main` is unchecked; the 2026-11-12 call came from a local model stand-in.
- `awr-mcp --catalog` project registration is unchecked with DSH.
- Only DSH 0.2.2-rc.1 on macOS arm64 was checked.
- DSH offers plugin events for turns, tools and compaction instead of `awr client install` hooks. An automatic adapter is a DSH plugin and lives outside this repository.
