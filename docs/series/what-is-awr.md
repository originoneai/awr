# What is AWR

AWR is an open-source project delivery platform for people
and AI. It connects **goals, workstreams, dependencies, context and acceptance**
into one project delivery workflow, so that a project can move from an agreed
goal to a verified delivery — even when the work is split across multiple
agents, sessions and collaborators.

AWR is built for two situations that share the same underlying problem:

- **One person coordinating several agents** on a complex project.
- **A team delivering a shared project**, starting with complex software
  projects.

In both cases, people set direction and review outcomes; agents do the work.
AWR connects their work to the same project state through its CLI and MCP
service.

## The problem AWR solves

When you work with AI coding agents, the project's knowledge usually lives in
the agent's private conversation. When the session ends, the model changes, or
a different person or agent picks up the work, that context is gone. Parallel
efforts drift apart, and "done" means whatever the last agent said it meant.

AWR is designed around what should be true instead:

- **Changing sessions, models or collaborators preserves the project.** Goals,
  constraints and verified progress survive the handoff. A successor receives
  the current task's required context and its checkpoint, so the project can
  continue across sessions and agents.
- **Parallel work has clear ownership and handoffs.** Independent workstreams
  (for example frontend, backend and testing) keep their own tasks, context and
  ownership while sharing project constraints.
- **"Done" is verifiable.** Completion claims connect to version-bound
  evidence. Implementation, verification, merge and release are tracked as
  separate facts, not collapsed into one claim.
- **Effort is visible.** At every stage you can tell what is ready, what is
  waiting, what has been verified, and where the effort went.

In short: AWR preserves **project continuity across finite context windows**.
An agent's context window is limited, and compaction (when a host shortens the
conversation to fit) is the host's job — AWR's checkpoints carry the recorded
project facts forward instead.

## How it works: your files stay the source of truth

AWR's architecture has three parts:

1. **Your files hold the intent.** Markdown and YAML files in your project
   describe goals, plans, work, constraints and decisions. AWR indexes these
   sources without silently replacing them.
2. **AWR maintains continuity.** Local state (a SQLite project) holds
   projections of those sources, plus sessions, claims, checkpoints and
   evidence. Context compilation — assembling the relevant goals, rules,
   dependencies and evidence for the current task — runs locally and makes no
   model calls. Revision checks protect writes from stale state.
3. **People and agents do the work.** The CLI and MCP service connect project
   facts to whatever host you use. Agents bring their own models, tools and
   conversations; reviewed changes and progress return to the project sources.

Because recorded checkpoints only carry recorded facts, they do not reconstruct
an agent's unrecorded conversation history — that remains the host's
responsibility.

## The pieces and how they fit together

You interact with AWR through two entry points, both installed from the same
package:

- **The `awr` CLI** — initializes a project, inspects status, and drives the
  explicit session workflow: claim a piece of work, get its focused context,
  and save a checkpoint before stopping. Any agent that can run shell commands
  can use it.
- **The `awr-mcp` MCP service** — exposes the same project state as MCP tools
  (MCP, the Model Context Protocol, is how AI clients call external tools). A
  client connects either to a single project over stdio, or to a shared HTTP
  MCP service that serves multiple clients and projects.

On top of these, AWR supports the agent you already work with. Integrations
come in layers:

- **Generic contract (default, fully supported).** Any host that can run a CLI
  or speak MCP uses the same project state through the shared session workflow.
  This includes Claude Code, Cursor, Windsurf and similar hosts — no special
  installer is needed.
- **Host notes.** For some hosts (Cursor, Kimi Code, Grok Build) AWR documents
  the exact configuration merge paths and native session flags, verified
  against a dated host version. These notes add no runtime features.
- **Optional lifecycle adapter.** Where a host's native lifecycle hooks are
  stable, an adapter can drive AWR checkpoints from host events. Today this
  exists for Codex. An adapter never turns AWR into that host's runtime.

For a person coordinating several agents, the same dependency, review and
accounting discipline applies as for a larger team — it is one project model,
not two products.

## What AWR is NOT

AWR is deliberately scoped. It is a host-agnostic work runtime, and its product
boundary is the shared CLI/MCP contract — not any one editor or agent.

- **AWR is not an AI agent.** It does not select, start or configure a coding
  agent. Agents bring their own models, tools and conversations; provider and
  model strings stored by AWR are display labels only.
- **AWR is not an editor, chat UI or model provider.** The agent's UI, native
  conversation IDs and compaction belong to the host.
- **AWR does not replace your project files.** Markdown/YAML sources remain
  authoritative; AWR's local state is an index plus continuity records, and it
  must not silently replace your sources.
- **AWR is not tied to one transport.** Whether MCP runs over stdio, HTTP or is
  absent is the host's choice; if a host has no native hooks, you checkpoint
  manually through the CLI.
- **AWR does not physically isolate workstreams.** Workstream isolation covers
  project state and supported operations; physical process isolation depends on
  the execution host.

## Current scope: published release vs. development

AWR is honest about what is shipped versus what is being built. The published
**0.5.1** CLI/MCP packages provide:

- Source-backed goals and tasks, with dependency navigation
- Focused context compilation for the current task
- Session claims and checkpoints
- Version-bound evidence
- A shared HTTP MCP service for multiple clients and projects
- Personal Workspace file exchange
- A personal workstream foundation: explicit source ownership, session
  attribution and scoped context in a local SQLite project

This release does **not** provide an authenticated Team service or complete
multi-client isolation. Still in development on `main`:

- Versioned cross-workstream delivery adoption (bind downstream work to an
  accepted artifact and contract version; recheck consumers when it changes)
- Team collaboration and delivery review (explicit reviewer and approval
  records)
- Workstream usage, time and ETA accounting

The release notes for each published version define the actual package
boundary.

## What this means for you

If you are evaluating AWR: you keep your current agent and your current files.
You initialize AWR in your project, give your agent a working agreement, and
from then on every session starts from recorded goals, rules, dependencies and
checkpoints rather than from scratch. You can inspect progress at any time.

If you are adopting it in a team: the same CLI/MCP foundation extends toward
shared delivery and review as those capabilities ship — the published release
already gives you the single-user and multi-agent core.

## Where to go next

- [Quickstart](quickstart.md) — install AWR, connect a project, and run your
  first session.
- [Concepts](concepts.md) — the project model in detail: goals, workstreams,
  sessions, claims, checkpoints and evidence.
- [CLI](cli.md) and [MCP service](mcp.md) — the two entry points in depth.
- [Agent integrations](agents.md) — connect the agent you already use.
