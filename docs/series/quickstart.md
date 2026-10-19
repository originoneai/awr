# Quickstart

This walkthrough takes you from a fresh install of AWR to a verified session
handoff: install the CLI, initialize a project, start a work session, and prove
that a successor session can pick up where the first one left off. Everything
is copy-paste runnable; AWR bundles its own SQLite.

## 1. Install AWR

AWR ships as native packages on npm and PyPI. Both channels install the same
Rust CLI (`awr`) and MCP server (`awr-mcp`); there is no separate JavaScript or
Python SDK.

With npm (requires Node 22.14 or newer):

```sh
npm install -g @originoneai/agent-work-runtime@0.6.0
```

Or with pip, inside a virtual environment (requires Python 3.9 or newer):

```sh
python -m pip install agent-work-runtime==0.6.0
```

Verify both executables:

```sh
awr --version
awr-mcp --version
```

Supported platforms are macOS 15+ (arm64 and Intel x64), Linux x64 and arm64
with glibc 2.39 or newer (the Ubuntu 24.04 baseline), and Windows x64. On
Linux, check your glibc first:

```sh
ldd --version | head -n 1
```

For npm installs, keep optional dependencies enabled — the launcher resolves a
per-platform native package. There is no install script or network downloader;
pip wheels embed the binaries. A third channel exists for application authors:
a pinned native payload of the two binaries that a host app can embed, so its
users need no Node, Python, or Rust at runtime.

## 2. Initialize your project

Initialization is a two-step operation: a preview, then an explicit accept.
Nothing is written until you pass `--accept`.

```sh
AWR_PROJECT=/absolute/path/to/your/project
awr --project "$AWR_PROJECT" init
```

Read the preview: it shows the inventory AWR found — existing Markdown task
ledgers, YAML sources, goals — and the source mapping it proposes. Existing
files are never overwritten; your original documents stay the authority. If the
preview looks right, accept it:

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

```sh
awr --project "$AWR_PROJECT" intake inspect --json
```

The report's `organization.state` tells you where you stand: `ready` means at
least one task has a source-declared goal, acceptance criteria, a next action,
and resolved prerequisites. `needs_organization` means something is missing —
the report's ordered `actions` tell you what to add to your source files.

## 3. Start a work session

A session is AWR's unit of work ownership. These commands use a small shell
helper so every call carries the project path and JSON output:

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

```sh
awrj context compile --work "$AWR_WORK" --session "$AWR_SESSION" \
  --budget 5000 > "$AWR_NOTES/context.json"
jq -e '.completeness.complete and (.work_context != null)' "$AWR_NOTES/context.json"
```

Read the packet, not only the boolean. On `BudgetExceeded`, widen `--budget`;
on `SourceStale`, run `awrj source reindex` and compile again.

## 4. Record progress with a checkpoint

Before you step away — or before a long host conversation compacts — save a
checkpoint with the context hash you actually used, an honest digest, the exact
next action, and every open loop:

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

A checkpoint can record unfinished work; it is not proof of passing tests or
completed acceptance criteria.

## 5. Continue the work in a new session

To prove continuation works, end the first session and resume from it — the
same takeover path a new agent, machine, or host conversation would use. End
releases the claim; it does not mark source work complete:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session end --session "$AWR_SESSION" --outcome incomplete \
  --expected-revision "$AWR_REV"
```

Now resume. Any agent — a different one, or the same one later — creates a
successor session from the predecessor:

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

```sh
awrj recovery inspect --session "$AWR_SESSION"
```

Working inside a coding agent's chat? Bind the host conversation to the AWR
session so checkpoints survive host compaction:

```sh
awrj client bind --client generic --external-session "myhost:$HOST_CONVERSATION_ID" \
  --work "$AWR_WORK" --session "$AWR_SESSION"
```

AWR does not transfer a host's process memory or take over arbitrary existing
processes — continuity comes from what was explicitly recorded.

## Where to go next

- [Concepts](concepts.md) — what sessions, claims, and checkpoints mean.
- [CLI reference](cli.md) — the full command surface used above.
- [Daily workflow](daily-workflow.md) — this loop in everyday work.
- [Troubleshooting](troubleshooting.md) — when something reports unexpected.
