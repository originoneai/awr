# No-UI host example

For dependency scheduling, see [source changes and task graphs](../../docs/dev/reference/source-changes.md)
and `orchestrator.py`. It composes a replaceable executor with the existing workflow,
bounded parallelism and durable dispatch lookup; source work state remains in AWR.

This standard-library Python 3.11+ example calls a pinned native AWR executable by
absolute path and argv. It uses no shell, model, installed hooks or private SQLite API. Explicit managed
commands use AWR's existing local supervisor; no resident host service is installed. Python runs the example host; it is not an AWR end-user dependency.

Supply the checksum and version from your trusted build/release receipt. The example
does not treat a checksum computed from an unknown download as release verification.
Keep the executable in a host-controlled location; checksum checks are not an OS
execution sandbox or protection against simultaneous replacement by a privileged user.

```text
python3 examples/host-app/host.py --binary /absolute/path/to/awr --sha256 <trusted-sha256> --version <version> demo --directory '/private/tmp/示例 项目'
python3 examples/host-app/host.py --binary /absolute/path/to/awr --sha256 <trusted-sha256> --version <version> inspect --project '/absolute/project path' --receipts '/private/path/new-receipts'
```

`demo` requires a new directory and copies a synthetic 25-work fixture. It previews
and accepts exact intake effects, traverses multiple catalog pages, binds a generic
conversation, checks compiled context, reports external progress, saves/reads a
checkpoint, deduplicates a repeated callback, inspects recovery and explicitly binds
a successor conversation. It checks original ledger bytes and ends fixture sessions.
The synthetic driver is not Kimi/Grok, a model receipt or complete business acceptance.

`inspect` traverses the current project's complete work catalog and returns aggregate
counts. It never edits authoritative source files; query-driven projection refresh
can write AWR runtime state. It does not create a client binding or claim.

Raw stdout, stderr, invocation and outcome receipts stay in the caller's NEW private
directory (POSIX 0700 with files 0600). On Windows, place it beneath a directory with
appropriate user ACLs. Never register this directory as a source or publish it. JSON
input uses protected files or stdin. Each command is invoked once; nonzero exits keep
partial results and typed errors. A timeout, host crash or lost response means an
unknown write outcome: inspect the pending receipt and the documented AWR status
operation before deciding whether to retry. No automatic write retry occurs.

`Host.catalog` fails the whole traversal on source failure, revision mismatch or
missing/duplicate objects. Keep the last complete UI snapshot separately; do not
present partial traversal as a confirmed empty project. Field shapes remain the
command-specific contract in [host-contract](../../docs/dev/reference/host-contract.md).

`cargo test -p awr-cli --test host_app` exercises the example on a temporary Chinese
path containing spaces, with no TTY. It requires Python 3.11+ on the development/CI
machine. The test also checks binary pin rejection, partial errors, and unknown
timeout outcomes without replaying commands.

## Explicit work workflow

Preparation's command-line default is now `--response-view action`, negotiated
through the pinned binary's capabilities. Python callers can use
`workflow.prepare(response_view="action")`. Hosts can report completed native
compactions with `observe_compaction`, inspect with `compaction`, and record a
user's postponement with `defer_compaction`. These never create a native session;
see the [measurement and guidance contract](../../docs/dev/integrations/context-continuity.md).

`workflow.py` is a reusable thin caller for ordinary engineering work. It uses the
same `Host` pin checks, argv, public domain commands, and private receipts. It adds
an OS-locked, atomically saved workflow state with a pinned executable checksum,
program version, canonical project root and expected project ID. Python 3.11+ is
needed only for this optional example; the native runtime is unchanged.

Every invocation supplies `--binary`, `--sha256`, `--version`, `--project`,
`--project-id`, and `--state /private/path/workflow/state.json`. Keep that directory
ignored and outside registered sources. The individual commands are:

1. `begin --work KEY --agent NAME --provider NAME --model NAME --expected-revision N`
   starts and claims one session. `adopt --session ID --work KEY` attaches an existing
   active session to a new workflow without duplicating the runtime session.
2. `context` returns the full actual context pack and records its immutable receipt.
   Deliver and consume `work_context.rendered_context`, then call
   `ack --consumed-hash HASH`. A hash without the matching intact delivery is rejected.
   The acknowledgement is explicitly a caller attestation; software cannot prove
   model comprehension. The example never acknowledges context on the caller's behalf.
3. `checkpoint --consumed-hash HASH --digest TEXT --next-action TEXT --expected-revision N`
   uses only the acknowledged delivery. A new `context` invalidates the old acknowledgement.
4. `evidence --input DRAFT.json --expected-revision N` records evidence for the selected
   work through the existing evidence contract. Commands in evidence are not executed.
5. `finish --input COMPLETION.json --reason TEXT --expected-revision N` runs the domain
   completion gates, persists the completed-work phase, then ends that exact session.
   Source completion and session end are separate durable operations, not an atomic batch.

The class exposes the same methods for embedding. Start from an initialized project
and reviewed source work; this wrapper does not silently activate or rewrite a task.
Revision arguments may be omitted when the workflow has observed one; AWR still
checks optimistic concurrency on every supported write. Workflow state is
operation continuity, not a replacement for source work state or an AWR acceptance record.

Before any write, a pending operation is durably saved. A crash, timeout, malformed
response or domain error leaves it for inspection; writes stop and are never retried
automatically. `inspect [--session ID --work KEY]` uses actual public session, work
and recovery queries and returns an inspection checksum. Read both the invocation
receipt and observed state. `reconcile --inspection-sha256 HASH --reason TEXT`
records an explicit operator decision and observed phase, without asserting that the
uncertain command succeeded. For an unknown begin, identify its actual session and
pass `--session` and `--work` to inspect/reconcile. Unbound outcomes are not guessed.
A completed work/session is never automatically reopened or duplicated.

The regression simulates losing a response *after a real checkpoint was saved*,
then inspects and reconciles without replay. It also covers stale revisions, wrong
pins, delivery tampering, adoption and a complete synthetic evidence/finish flow.
This is a public CLI workflow, not restoration of a private native client or E4
business acceptance.

For unified `prepare`/`progress`, optional concise responses, and reviewed reports
assembled from actual managed execution, see [host workflows with less
bookkeeping](../../docs/dev/integrations/host-workflow.md). The optional report flow
requires state under an ignored project-local directory and explicit source and
new artifact paths. It preserves the same consumption, completion and recovery
requirements as the original workflow above.
