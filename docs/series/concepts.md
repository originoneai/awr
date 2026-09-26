# Core concepts

AWR keeps a project's work facts outside any single agent
conversation, so work survives context loss, agent switches and restarts. This
article explains the five ideas behind that model; the commands in the
[Quickstart](quickstart.md) and the routines in the
[Daily workflow](daily-workflow.md) then read naturally.

The core principle: your source files (ledgers, plans) are the authority, and
everything the runtime stores — sessions, claims, evidence — is a checkable
projection of those facts. AWR never invents missing intent, and never lets a
conversation transcript substitute for a recorded fact.

## Project facts: the source ledger

AWR's *source ledger* is a small work contract you keep in ordinary project
files: a YAML ledger such as `work-ledger.yaml`, or an existing Markdown
ledger (`*-ledger.md`) that stays read-only through AWR and is edited in its
original file. A ledger records two kinds of facts:

- **Goals** — what the project is trying to achieve, with success criteria.
  Goals can be `draft`, `candidate` or `needs_confirmation` when uncertain,
  and `active` or `confirmed` when supported by source material or user
  instruction. AWR checks the declaration; it does not certify that the goal
  matches what a human meant.
- **Work items** — the tasks, described below.

A minimal ledger looks like this:

```yaml
goals:
  - id: search
    title: Let readers find documents
    status: active
    summary: The user requested search in the existing portal; see README.md.
    success_criteria: [Readers find a requested document]
work_items:
  - id: search-api
    title: Add document search
    status: ready
    goal: search
    acceptance: [A matching query returns the requested document]
    next_action: Implement the search handler using the current document index
```

You bring these files under AWR's management with init. Nothing is written
until you accept the preview:

```sh
awr --project /absolute/project init
awr --project /absolute/project init --accept
awr --project /absolute/project intake inspect --json
```

Init never overwrites existing files, and inspection only refreshes a
rebuildable projection cache — never your goals, task files or execution
state.

## Tasks: work items

A *work item* is one task in the ledger. Its key fields are the ones AWR can
actually check:

- `acceptance` — the completion criteria: what must be observably true when
  the task is done. A completion report is later checked against these exact
  criteria.
- `next_action` — the persisted next step, and the most valuable field for
  recovery: it lets a fresh session continue without re-reading history.
- Supporting fields: `summary`, `goal`, `kind`, `paths`, `tags`, `priority`,
  `depends_on`, `owner` and `milestone`.

You create a task from a JSON draft that states only explicitly known facts:

```sh
awr work create --input draft.json
```

Creation always produces a *draft*. Missing goals or required facts stay
visible, and a draft grants no permission to execute or complete anything;
applying it is a separate, explicit step.

AWR also reports project-level organization states such as `not_initialized`,
`needs_organization`, `ready`, `blocked`, `awaiting_verification`,
`completed` and `closed_without_completion`. Two matter most day to day:
`ready` means at least one task has a source-declared goal, acceptance
criteria, a next action and resolved prerequisites; `awaiting_verification`
means the ledger says tasks are done but their acceptance reports have not all
been verified yet.

## Checkpoints and sessions

A *session* is one agent's working attachment to a work item. A *checkpoint*
is the durable record of where that session stands: the persisted next action,
open loops, and the actual AWR event and source delta — not a copy of the
conversation.

When a client session starts or resumes after compaction, AWR returns recovery
context built from the checkpoint. When the turn stops or ends, the changed
next action and open loops are saved before they can be lost:

```sh
awr client progress --client codex --external-session CLIENT_ID \
  --next-action "Apply the reviewer corrections" --open-loop "Independent review remains"
```

Duplicate events with unchanged work reuse their checkpoint; a continued turn
with changed progress creates a new one. One honest boundary: the native hook
adapter that automates this is currently installed for Codex only. Other hosts
use the L0 binder (`--client generic`), which attaches a host conversation to
an active AWR session without installing hooks. The adapter never reads
transcript bodies — it records what you tell it, not what a model said.

## Claims and handoff

A *claim* is the explicit lock a session holds before executing a task. AWR
requires you to acquire the session claim before execution, and completion
re-checks it — this is how two agents avoid silently working the same task.

A *handoff* moves work from one session or client to a successor. You resume a
session explicitly, with revision checks and a successor transition:

```sh
awr session resume --from-session AWR_SESSION_ID --agent successor \
  --provider generic --model selected-model --no-claim --expected-revision REVISION
```

Or bind a new client conversation to its predecessor:

```sh
awr client bind --client generic --external-session NEW_CLIENT_ID \
  --work INTAKE-001 --from-session AWR_PREDECESSOR_ID
```

Handoff moves recorded facts — ledger state, checkpoints, execution history —
not process memory. Shutdown hooks are advisory: they never release claims or
terminate sessions, which remains your explicit responsibility at handoff.

## Delivery and acceptance

*Acceptance* is where AWR is deliberately strict. A task is not done because
someone wrote `status: completed` in a ledger. It is done when a registered
completion report verifies against the ledger's current acceptance criteria.

A completion report records the command actually executed, the scope actually
verified, the time, and named checks — each mapping to an exact acceptance
criterion and describing the observed outcome, not the intended result:

```json
{
  "version": 1,
  "work_item": "WORK",
  "source_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "command": "the command or verification procedure actually executed",
  "scope": ["the scope actually verified"],
  "verified_at": 1,
  "checks": [{
    "name": "independent check name",
    "passed": true,
    "details": "the observed outcome, not an intended result",
    "criteria": ["an exact current source acceptance criterion"]
  }]
}
```

Before completing, you preflight the report:

```sh
awr work prepare-completion WORK --report report.json --evidence-key KEY \
  --source-sha FULL_SHA --level locally_verified
```

Preflight runs no command, registers no evidence and completes no task — it
reads the report bytes and returns the evidence arguments and the acceptance
mapping. Completion itself is a separate write that rechecks claims,
dependencies, source freshness and report bytes; changing a report after
preflight invalidates its digest. Source-declared completion alone never
supplies a verified receipt, and `source_completed` and `verified_completed`
remain separate counts in status reports.

## One task, end to end

Tie it together with the search task from the ledger above:

1. **Facts.** You run `awr --project /absolute/project init --accept`; the
   ledger with the `search` goal and the `search-api` work item becomes the
   authority.
2. **Task.** The work item carries acceptance criteria and a next action, so
   the project reports `ready`.
3. **Session and claim.** You prepare the work from one snapshot, then acquire
   the session claim before touching code:

   ```sh
   awr work prepare search-api --session AWR_SESSION_ID --source-sha FULL_SHA
   ```

4. **Checkpoint.** As you work, you save progress with `awr client progress`,
   so the next action and open loops survive a crash or compaction.
5. **Handoff (if needed).** A successor resumes with
   `awr session resume --from-session …` and gets the same facts, without any
   chat history being replayed.
6. **Delivery.** You write a report whose checks map to the exact acceptance
   criterion, preflight it with `awr work prepare-completion`, and only then
   record completion. Status now shows the task as *verified* against the
   source SHA, not merely marked done.

## Where to go next

- [Quickstart](quickstart.md) — run these concepts for the first time.
- [Daily workflow](daily-workflow.md) — the routine loop of prepare, claim,
  checkpoint and complete.
