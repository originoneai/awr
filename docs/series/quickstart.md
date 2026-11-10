# Quickstart

Install AWR, initialize a project, claim work and pass a checkpoint to a successor session. The walkthrough uses the bundled SQLite runtime and copyable shell commands.

## Use it

### 1. Install AWR

AWR ships the same native Rust CLI (`awr`) and MCP server (`awr-mcp`) through npm and PyPI.

With npm (requires Node 22.14 or newer):

```sh
npm install -g @originoneai/awr@0.7.0
```

Or with pip, inside a virtual environment (requires Python 3.9 or newer):

```sh
python -m pip install originoneai-awr==0.7.0
```

Verify both executables:

<!-- doc-test: version -->
```sh
awr --version
```
<!-- expect: awr -->

```sh
awr-mcp --version
```

Supported platforms are macOS 15+ (arm64 and Intel x64), Linux x64 and arm64
with glibc 2.39 or newer (the Ubuntu 24.04 baseline), and Windows x64. On
Linux, check your glibc first:

```sh
ldd --version | head -n 1
```

For npm, keep optional dependencies enabled so the launcher resolves its per-platform native package. Pip wheels embed the binaries. Application authors can embed a pinned native payload.

## How it behaves

| Step | Result |
| --- | --- |
| Preview and accept | Registered authoritative sources |
| Start and claim | A work-bound session |
| Compile and read | Budgeted context with completeness and hash |
| Checkpoint and resume | A successor with recorded next actions and open loops |

### 2. Initialize your project

From your project directory, initialization is a two-step operation: a preview, then an explicit accept.
Nothing is written until you pass `--accept`.

<!-- doc-test: init-preview -->
```sh
AWR_PROJECT=$PWD
awr --project "$AWR_PROJECT" init
```

Read the inventory of Markdown task ledgers, YAML sources and goals, then review the proposed mapping before accepting it:

<!-- doc-test: init-accept -->
```sh
awr --project "$AWR_PROJECT" init --accept
```

For a blank project, state its purpose up front:

```sh
awr --project "$AWR_PROJECT" init --goal "Deliver a document portal" --accept
```

If your Markdown ledger uses nonstandard status words, map them at init time:

```sh
awr --project "$AWR_PROJECT" init \
  --status-map pending=planned --status-map complete=completed --accept
```

Then inspect the organization report:

<!-- doc-test: intake-report -->
```sh
awr --project "$AWR_PROJECT" intake inspect --json
```

The report's `organization.state` tells you where you stand: `ready` means at
least one task has a source-declared goal, acceptance criteria, a next action,
and resolved prerequisites. `needs_organization` means something is missing —
the report's ordered `actions` tell you what to add to your source files.

### 3. Start a work session

A session is AWR's unit of work ownership. These commands use a small shell
helper so every call carries the project path and JSON output:

<!-- doc-test: session-config -->
```sh
AWR_BIN=$(command -v awr)
AWR_WORK=INTAKE-001          # a task key from your intake report
AWR_AGENT=agent-primary      # a label for who is working
AWR_MODEL=your-current-model # a recorded label; AWR does not invoke the model
AWR_NOTES=$(mktemp -d "${TMPDIR:-/tmp}/awr-session.XXXXXX")
awrj() { "$AWR_BIN" --project "$AWR_PROJECT" --json "$@"; }
```

Check what is executable, then start a session with a claim (runtime ownership
of the work item) and the current project revision:

<!-- doc-test: start-session -->
```sh
awrj ready
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session start --work "$AWR_WORK" --agent "$AWR_AGENT" \
  --provider generic --model "$AWR_MODEL" --claim --ttl-ms 3600000 \
  --expected-revision "$AWR_REV" > "$AWR_NOTES/start.json"
AWR_SESSION=$(jq -er '.session.id' "$AWR_NOTES/start.json")
```

Compile the work context — the goal, acceptance criteria, recorded facts, and
history assembled into a bounded packet:

<!-- doc-test: compile-context -->
```sh
awrj context compile --work "$AWR_WORK" --session "$AWR_SESSION" \
  --budget 5000 > "$AWR_NOTES/context.json"
jq -e '.completeness.complete and (.work_context != null)' "$AWR_NOTES/context.json"
```

Read the complete packet. On `BudgetExceeded`, widen `--budget`; on `SourceStale`, run `awrj source reindex` and compile again.

### 4. Record progress with a checkpoint

Before you step away — or before a long host conversation compacts — save a
checkpoint with the context hash you actually used, an honest digest, the exact
next action, and every open loop:

<!-- doc-test: checkpoint -->
```sh
AWR_CONTEXT_HASH=$(jq -er '.work_context.context_hash' "$AWR_NOTES/context.json")
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session checkpoint --session "$AWR_SESSION" --agent "$AWR_AGENT" \
  --context-hash "$AWR_CONTEXT_HASH" \
  --digest "Record the work actually done; do not invent a passing review." \
  --next-action "State the exact next operator or agent action." \
  --open-loop "List every unresolved loop." \
  --expected-project-revision "$AWR_REV" > "$AWR_NOTES/checkpoint.json"
```

### 5. Continue the work in a new session

End the first session to release its claim, then create a successor through the same takeover path used by another agent, machine or host conversation:

<!-- doc-test: end-session -->
```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session end --session "$AWR_SESSION" --outcome incomplete \
  --expected-revision "$AWR_REV"
```

Now resume. Any agent — a different one, or the same one later — creates a
successor session from the predecessor:

<!-- doc-test: resume-session -->
```sh
AWR_PREDECESSOR="$AWR_SESSION"
awrj session show "$AWR_PREDECESSOR"
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session resume --from-session "$AWR_PREDECESSOR" \
  --agent successor --provider generic --model "$AWR_MODEL" \
  --budget 5000 --expected-revision "$AWR_REV" > "$AWR_NOTES/resume.json"
jq -e '.context_ready' "$AWR_NOTES/resume.json"
AWR_SESSION=$(jq -er '.resumed.session.id' "$AWR_NOTES/resume.json")
```

Two checks confirm the handoff worked:

- `jq -e '.context_ready'` exits 0: the successor received the predecessor's
  recorded context, including the last successful checkpoint.
- The resumed session has a new ID — resume creates a new AWR session rather
  than mutating the old one.

You can also inspect recovery state read-only at any time:

<!-- doc-test: inspect-recovery -->
```sh
awrj recovery inspect --session "$AWR_SESSION"
```

Working inside a coding agent's chat? Bind the host conversation to the AWR
session so checkpoints survive host compaction:

<!-- doc-test: bind-conversation -->
```sh
HOST_CONVERSATION_ID=replace-with-your-host-conversation-id
awrj client bind --client generic --external-session "myhost:$HOST_CONVERSATION_ID" \
  --work "$AWR_WORK" --session "$AWR_SESSION"
```

### Where to go next

- [Concepts](concepts.md) — what sessions, claims, and checkpoints mean.
- [CLI reference](cli.md) — the full command surface used above.
- [Daily workflow](daily-workflow.md) — this loop in everyday work.
- [Troubleshooting](troubleshooting.md) — when something reports unexpected.

## Limits

- The packages expose native executables; JavaScript and Python SDKs are outside their scope.
- Installation uses bundled native payloads rather than install scripts or network downloaders.
- Embedded native payloads run independently of Node, Python and Rust runtimes.
- Initialization preserves existing files and their authority.
- Checkpoint success records progress, including unfinished work; test results and acceptance require their own evidence.
- Session end releases runtime ownership; completing source work uses a separate operation.
- Continuity transfers explicitly recorded facts; host process memory and arbitrary existing processes remain host responsibilities.

## Code and tests

- [Executable guide checks](../../tests/docs/run_doc_commands.py)
- [CLI session tests](../../crates/awr-cli/tests/session_cli.rs)
- [Client lifecycle fixture](../../examples/codex/README.md)
