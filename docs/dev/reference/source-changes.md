# Source changes and task graphs

Hosts that need to monitor files beyond registered source mappings can use the
read-only [file inventory](file-inventory.md) interface.

AWR owns task identity, source authority, required dependencies, execution claims,
readiness, checkpoints and completion evidence. A planner supplies explicit work
and dependency proposals. A host chooses Agents, allocates resources, dispatches
executors and enforces its concurrency limit. AWR does not launch models or shells.

## Query a plan and its impact

`awr_work_graph {"roots":["API"],"limit":100}` returns API, its transitive required
dependents and all required ancestors of that affected set. Omit roots to inspect
all work; use external keys, not titles. The same core query is available as
`awr --json work graph --root API --limit 100` (CLI queries may refresh projections;
add `--cached` to read the last recorded projection without opening business files or
writing project files, see [host contract](host-contract.md)).
MCP queries never persist a refresh; explicitly reindex stale sources first.

The result includes source references and fingerprints, project revision, graph
fingerprint, readiness diagnostics and active claims. Each node also has
`last_event_at`, the newest event time of that work in milliseconds over every branch
and session (null without events); it never enters the graph fingerprint. `affected`
identifies the changed roots and dependents, while `nodes` also includes
prerequisites. Required missing references and a real cycle are explicit. Optional edges remain visible
but do not block execution or participate in required dependency cycles.
If the closure exceeds the selected limit, the query returns `BudgetExceeded`;
it never labels a truncated plan complete. The default limit is 100, maximum 1000.

`ready` is a current scheduling observation. Consume the task context and acquire
an actual claim before dispatch. A graph fingerprint is not an execution lease:
readiness and claims can change without a source edit. Review new source/revision
state after an incremental plan change. Completed dependencies release successors
through the existing readiness rules; executor termination alone does not.

## Review and apply a source change

Four tools share a source-operation identity scoped by project, authenticated
client, change kind and caller-selected `request_id`:

1. `awr_change_preview`: send `request_id`, a concrete `reason`, and `change`.
   Read the returned before/after source text and `preview.fingerprint`.
   Preview uses an in-memory database snapshot and does not persist source/cache edits.
2. `awr_change_apply`: send the identical request plus `expected_revision` and
   `expected_preview` from that review. Version/source conflicts require a fresh
   review. An already-recorded identical request returns its original outcome.
3. `awr_change_status`: send the original `request_id` and `kind` (`create`,
   `batch`, `edit` or `work_edit`). This remains available when a partial write left stale sources.
4. `awr_change_recover`: after inspecting a pending outcome, send the same identity
   and the current `expected_revision`. Recovery preserves externally edited bytes
   and reports which phases actually completed; it does not start another executor.

These tools reuse the existing CLI creation, batch and host-edit journals. They do
not add MCP lifecycle bookkeeping revisions between preview and apply. Query them
with `awr_change_status`, **not** `awr_operation_get`. Other MCP lifecycle mutations
continue to use the operation journal. Recovery and completion are separate facts.
Source writes, indexing and related-file replacement are not a filesystem transaction.
Inspect `source_write_performed`, `status`, and `historical_outcome`; do not infer
"nothing happened" from an error, missing response or missing receipt.

Shared HTTP still requires the registered project selector and that client's write
permission. A different client cannot apply another client's preview or retrieve
its journal merely by guessing the request ID. Stdio has one local client identity.
The `reason` and field values describe the caller's intent; they do not independently
verify it. Batch/edit attribution uses the actual MCP client as `delegated_agent`.
The API does not grant a human-confirmation completion path to an Agent.

For title/summary/priority/next-action changes, use the [small edit shortcut](daily-work.md):
`change:{kind:"work_edit",work:"KEY",fields:{next_action:"Review the draft"}}`.
Its preview returns compact changed fields and a normalized `change`, plus
`preview_fingerprint` at the top level. Apply that returned change so retries
retain the reviewed source fingerprint. General create/batch/edit responses keep
their existing full preview shape.

### CLI and MCP request envelopes

The CLI reads a complete request from `--input FILE`. MCP wraps the same change in
its own request identity and supplies actor provenance from the connected client.
Use the envelope for the selected entry point:

| Operation | CLI | MCP |
| --- | --- | --- |
| Create | `work create --input create.json` | `awr_change_preview` / `awr_change_apply`, `change.kind="create"` |
| Batch | `batch change --input batch.json` | `awr_change_preview` / `awr_change_apply`, `change.kind="batch"` |
| Host edit | `host preview --input edit.json`, then `host save` | `awr_change_preview` / `awr_change_apply`, `change.kind="edit"` |

CLI creation input:

```json
{"version":1,"request_key":"appendix-1","title":"Review appendix","fields":{"goal":"GUIDE","acceptance":["The conclusion is reviewed"],"next_action":"Read the source material"}}
```

CLI batch input (the `actor` and `reason` keys are required):

```json
{"version":1,"request_key":"batch-1","actor":{"host":"awr-cli","subject":"planner","origin":"delegated_agent"},"reason":"Refine the appendix plan","change":{"kind":"ledger","source_id":"<registered-source-id>","source_fingerprint":"<reviewed-source-fingerprint>","operations":[{"operation":"fields","target":"APPENDIX","fields":{"next_action":"Review the draft"}}]}}
```

The corresponding MCP batch envelope:

```json
{"request_id":"batch-1","reason":"Refine the appendix plan","change":{"kind":"batch","change":{"kind":"ledger","source_id":"<registered-source-id>","source_fingerprint":"<reviewed-source-fingerprint>","operations":[{"operation":"fields","target":"APPENDIX","fields":{"next_action":"Review the draft"}}]}}}
```

CLI host activation input:

```json
{"version":1,"request_key":"activate-1","actor":{"host":"awr-cli","subject":"planner","origin":"delegated_agent"},"reason":"Activate the reviewed appendix","change":{"operation":"activate_draft","work":"APPENDIX","source_fingerprint":"<reviewed-source-fingerprint>"}}
```

The MCP equivalent is `{"request_id":"activate-1","reason":"Activate the reviewed appendix","change":{"kind":"edit","change":{"operation":"activate_draft","work":"APPENDIX","source_fingerprint":"<reviewed-source-fingerprint>"}}}`.

CLI creation also accepts repeated `--acceptance`, `--goal`, `--depends-on`, `--path`,
`--tag`, and `--spec-ref` (one specification-reference JSON object), plus `--kind`,
`--priority`, `--summary`, `--next-action`, `--owner` and `--milestone`. These map to
`fields.acceptance`, `goal`, `depends_on`, `paths`, `tags`, `spec_refs`, `kind`,
`priority`, `summary`, `next_action`, `owner` and `milestone`. `--input` and field
flags are mutually exclusive. No omitted source facts are invented.

Provide the title, acceptance, goal and a concrete `--next-action` before requesting
activation. For JSON input, put the same next step in `fields.next_action`.
Include any milestone required by the registered source policy.

```sh
awr work create --title "Review appendix" --request-key appendix-1 \
  --acceptance "The conclusion is reviewed" --goal GUIDE \
  --next-action "Read the source material" --activate
```

Repeat that request with `--accept`, `--expected-preview` and `--expected-revision`
from the preview to create and attempt activation. If the activation checks refuse it, creation
succeeds, `draft` stays true, and `activation.error` explains the reason. The
creation receipt retains that outcome. Use a new activation request after repairing
the draft. Creation and activation share a caller request but retain separate
recoverable source operations.

An activation refusal lists missing fields and suggests an edit preview instead of
repeating the refused activation. Supply the actual business values in the named
`AWR_*` variables before executing that command. Unset variables stop the shell;
the command does not write placeholder criteria or select a goal for the caller.
For example, repair a missing next step with:

```sh
export AWR_NEXT_ACTION="Read the source material"
awr work edit APPENDIX --request-key repair-appendix --actor planner \
  --reason "Complete draft fields" --next-action "${AWR_NEXT_ACTION:?}"
```

Accept the returned edit with its `source_fingerprint`, `preview_fingerprint` and
`project_revision`, then preview and accept activation. Draft repairs also support
repeated `--acceptance` and `--goal`, which replace the complete corresponding lists,
and `--kind` / `--milestone`. Those planning-field repairs require a draft. Activation
continues to validate the supplied criteria, goal, dependencies and source policy.

Creation revision conflicts include `details.retry.expected_revision`. The caller
can reuse its request with that revision. If a suggested command would exceed 300
bytes or contain control characters, the CLI retains the exact request in a private
project runtime input file and suggests `work create --input FILE` with the reviewed
fingerprint, activation intent and new revision. Changed sources still require a
fresh preview. Long context-budget recovery similarly uses
`context compile --input FILE --budget N`, retaining all original selectors and the
delta baseline; input and selection flags cannot be mixed.

For an existing draft, `awr work activate APPENDIX --request-key activate-1`
previews activation. Accept with the same arguments plus `--accept`,
`--source-fingerprint`, `--expected-preview` and `--expected-revision` from that
preview. `work create-status`, `work create-recover`, `batch status` and
`batch recover` accept both `--key` and its visible alias `--request-key`.

### Create a draft

```json
{
  "request_id": "appendix-draft-1",
  "reason": "Add the requested appendix to the existing guide",
  "change": {
    "kind": "create",
    "title": "Review the appendix",
    "fields": {
      "goal": "GUIDE",
      "acceptance": ["The appendix conclusion is reviewed"],
      "next_action": "Read the source material",
      "depends_on": ["RESEARCH"]
    }
  }
}
```

`source_id` is optional when one primary ledger is unambiguous. Creation first
writes a draft; missing goals, criteria and results are never invented. The CLI can
explicitly request activation as described above. MCP creation keeps its existing
input shape and draft behavior. Both entries support the same bounded source
fields and preserve stable task identity.

### Change several tasks together

Use `change.kind="batch"` and the existing `BatchChange` shape:

```json
{
  "kind": "batch",
  "change": {
    "kind": "ledger",
    "source_id": "<registered-source-id>",
    "source_fingerprint": "<reviewed-source-fingerprint>",
    "operations": [
      {"operation":"import","external_key":"RESEARCH","title":"Review source material","fields":{},"duplicate":"fail"},
      {"operation":"import","external_key":"APPENDIX","title":"Review appendix","fields":{"depends_on":["RESEARCH"]},"duplicate":"fail"},
      {"operation":"fields","target":"GUIDE","fields":{"next_action":"Review the appendix plan"}}
    ]
  }
}
```

Operations are `fields {target,fields}`, `import
{external_key,title,fields,duplicate:"fail"|"skip_exact"}`, or `archive
{target,archived}`. A batch contains at most 100 distinct task targets. Import order
does not constrain dependency order: the **whole final candidate graph** is checked
before source replacement. Required missing references, cycles, archive conflicts,
duplicate cross-source task keys and changes to an occupied task's planning contract
are rejected. Next-action notes do not replace the planning contract.

The existing related-file batch remains available as
`{"kind":"related","changes":[...]}`; its document/adoption/ledger variants and
partial-write recovery semantics are documented in [host contract](host-contract.md).
It can include one ledger with its related registered documents. This is controlled
source editing, not arbitrary filesystem writing.

### Refine or activate a draft

`change.kind="edit"` accepts the existing `HostChange` work operations:

```json
{"kind":"edit","change":{"operation":"fields","kind":"work_item","target":"APPENDIX","source_fingerprint":"<reviewed-fingerprint>","fields":{"acceptance":["The conclusion and limitations are reviewed"]}}}
```

```json
{"kind":"edit","change":{"operation":"activate_draft","work":"APPENDIX","source_fingerprint":"<reviewed-fingerprint>"}}
```

Activation retains the normal goal, rule, acceptance, dependency and occupancy
checks. Draft successors whose required predecessors are unfinished remain drafts
until those prerequisites permit activation. Activation does not complete or claim
the work. Lifecycle actions and engineering completion still use their own tools.

## Replaceable host scheduling example

[orchestrator.py](../../../examples/host-app/orchestrator.py) supplies a caller-driven
step over an injected driver. The driver can query this MCP graph or the same CLI
query, and can use [Workflow](../../../examples/host-app/workflow.py) for pinned-runtime
sessions, explicit context consumption, checkpoints and guarded completion.

The host saves a stable dispatch identity before any effect, respects its configured
parallelism and conservative path conflicts, and retains capacity for unknown
executions. Restart queries retained identities; it never automatically repeats an
unknown dispatch. An executor reporting stopped does not mark work completed. AWR's
current source state and completion gates determine whether successors are ready.
Missing path hints serialize work by default; real resource isolation, quota policy,
Agent credentials and process termination belong to the host's executor adapter.

The example regression uses native CLI claims/context/evidence/completion, independent
synthetic output files, simultaneous outstanding executions, path conflicts, a lost
dispatch response and dependency release. It is a protocol/host fixture, not an
enterprise or native Agent-client business acceptance result.
