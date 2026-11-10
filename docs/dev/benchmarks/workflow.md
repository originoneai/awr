# Complete workflow costs and reliability

The [versioned contract](../../../tests/benchmarks/workflow/contract.json) compares two
ways of consuming the **same native runtime and initial source bytes**. The
primitive path reads work, management and context separately; the prepared path
uses `awr_work_prepare`. Both retain the same goals, hard rules, completion criteria,
claims, actual artifact checks, completion reports and source writes.

This is a comparison between equivalent supported API workflows. It is not a
comparison with doing no task management, a previous release, an optimized client
that already caches some observations, or another product. It does not establish
that every short task should use AWR.

## Recorded result

[Reviewed aggregate data](workflow-macos-arm64.json), measured on macOS arm64 with
the same **unoptimized development binaries** from source `4898e7122fb9`, after the completion-query regression fix and before
this measurement/documentation-only update. Three repetitions per strategy and
workload produce 30 complete runs; order alternates to reduce order bias.

| Workload | Primitive calls | Prepared calls | Returned text reduction | Whole workflow median, primitive → prepared |
| --- | ---: | ---: | ---: | ---: |
| Bounded lightweight work | 20 | 16 | 3.67% | 1,104 → 1,072 ms |
| Scope change and retained upgrade | 31 | 25 | 3.83% | 1,412 → 1,321 ms |
| Wait, server restart and reply | 30 | 22 | 4.68% | 1,800 → 1,710 ms |
| Verified dependency release | 39 | 31 | 3.74% | 1,599 → 1,541 ms |
| Lost result, restart and lookup | 22 | 18 | 3.45% | 1,212 → 1,131 ms |

Calls fall by 18–27%, but returned text decreases much less. The initial tool catalog
alone is about 30.8 KB of protocol output, and report/evidence/lifecycle maintenance
remains in both paths. Fewer calls are useful; they do not imply a large reduction
in total Agent context or cost.

The prepared query's observed median is 38.1 ms across 45 calls; its maximum is
533.6 ms. These samples include fresh-server context initialization. End-to-end
workflow durations include initialization and shutdown, protocol discovery, source
checks, expected rejections and persistence. Three workload repetitions are not a
latency SLO or a performance claim for optimized builds, large ledgers, or enterprise
concurrency. Per-operation sample counts, minima, maxima and medians are in the data.

All 30 runs passed the separate required-fact, false-completion, actual-completion,
identity, duplicate-execution and scenario-behavior checks. These checks establish
the tested behavior on these fixtures; they do not prove model comprehension or
general absence of defects. Each run deliberately attempts an invalid completion
before registering evidence and verifies that source work remains unfinished.

## Workloads and boundaries

- **Lightweight:** explicit bounded observations retain lightweight management;
  goal/rule/acceptance consumption and engineering completion remain enforced.
- **Upgrade:** an occupied contract edit is rejected. After explicit claim release,
  the source acceptance grows to include a separate tradeoff artifact. The same work
  identity upgrades, consumes new context, reacquires its claim and satisfies all criteria.
- **Wait:** a persisted question survives a server restart. Progress is blocked until
  the reply is recorded. Management upgrades and the original work identity survives.
- **Dependencies:** the successor is initially blocked. The predecessor produces a
  real synthetic artifact and passes completion before the successor becomes ready.
  The separate [host example](../reference/source-changes.md) tests bounded outstanding
  parallel work and resource conflicts; this timing workload runs the dependency in order.
- **Unknown result:** the native server really saves a checkpoint, and the fixture
  withholds its response from the host. After restart the host queries the original
  request and exact checkpoint, without replaying the checkpoint or business executor.
- **Checked completion:** a work-bound session records four failed baseline checks,
  proposes admission and receives approval from a distinct reviewer session. After
  claiming and starting work, it produces the artifact, commits the source state,
  runs all acceptance checks and completes with their generated report and input.
  The completion gate enforces the registered checks and combined session close-out.

The [oracle](../../../tests/benchmarks/workflow/oracle.py) independently checks actual
artifacts, limitations, exact criteria, context facts and one business execution per
work. Its negative tests reject missing rules, changed identities, missing coverage
and duplicate execution. A reported source status alone cannot pass the oracle.

## What is counted

Raw input/output for every CLI invocation and MCP request is recorded in a new
ignored output directory. The recorder includes failed/rejected calls, restart
discovery and recovery lookups. `wire_*_bytes` counts actual MCP JSON lines and
serialized CLI argv/returned bytes; CLI argv is a measurement encoding, not a
network protocol. `tool_text_bytes` counts the MCP text content once and CLI stdout.
MCP wire output separately includes both structured content and its text representation.
Actual client rendering may feed a different subset to a model.

Report assembly and artifact creation are separately recorded host operations.
`maintenance_*` includes maintenance API traffic plus report assembly bytes/time;
per-run phase records separate context, continuity, guard checks, recovery and
verification. Model-independent artifact production is included in wall time.
Fixture construction and initial binary hashing occur before the measured intake.
There are no real user-think times, model calls, provider retries or external network
executors in these fixtures. Explicit retries/replays are counted from repeated
mutation identities, including rejected requests, rather than assumed to be zero.

**Model input/output/reasoning tokens and fees remain `null`.** UTF-8 bytes, context
token estimates and fewer tool calls are not a provider billing receipt. The older
[context selection benchmark](README.md) measures a different denominator.

Actual project self-use and its private sources/receipts stay outside this public
dataset. Candidate use does not replace an existing pinned release binding or supply
native enterprise Agent-client E4 acceptance.

## Reproduce

Use Python 3.11+ and trusted native binaries built from one known source commit:

```sh
cargo build --locked -p awr-cli -p awr-mcp
python3 -m unittest discover -s tests/benchmarks/workflow -v
python3 tests/benchmarks/workflow/run.py \
  --awr target/debug/awr --mcp target/debug/awr-mcp \
  --runtime-source-sha <full-commit-that-built-both-binaries> \
  --output .local/workflow-measurement-001 --repetitions 3
```

On Windows use native `.exe` paths. Use a new output directory each time. The result
binds the actual binary hashes, harness hashes, environment and declared source SHA.
Only reviewed aggregate data belongs in Git; raw source, identities, operation
receipts and failures remain local. Native platform CI runs the oracle and all twelve
strategy/workload combinations on each actual platform, without timing thresholds.

## Golden-path budgets

The native CLI tests measure AWR calls and returned UTF-8 bytes, including stdout
and stderr. Fixture initialization and independent final-state assertions surround
the measured path. Revision bookkeeping reads the isolated fixture store directly.
The limits live at the top of `golden_paths_cli.rs`; changes to a limit require an
explanation in the PR.

```sh
AWR_LOCAL_WEB=0 cargo test -p awr-cli --test golden_paths_cli \
  --test governance_overhead_cli --locked -- --nocapture
```

On macOS arm64, using unoptimized binaries from `23dfa6499120`, the current fixture
measurements are:

| Path | AWR calls | Returned bytes | Call limit | Byte limit | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| New idea through claimed, prepared work | 4 | 9,013 | 4 | 6,144 | Byte limit exceeded |
| Checked admission through completion, author | 6 | 5,538 | 6 | 8,192 | Within limits |
| Separate admission reviewer | 2 | 4,391 | 2 | — | Within limit |
| Compacted session continuation | 0 model calls; 1 host hook | 869 | 0 model calls | — | Within call limit |
| Revision-only recovery | 1 | 180 | 1 | — | Within limit |
| Explicit context-budget recovery | 1 | 2,485 | 1 | — | Within limit |

The creation preview's text summary omits the fingerprint needed for acceptance.
Its measured call therefore requests JSON and counts the entire 4,611-byte
response; acceptance and activation return 204 bytes, session creation returns
97 bytes, and text preparation returns 4,101 bytes. The 6 KiB limit stays in force
and the positive regression test reports the excess. A clean main checkout
reproduces the same result with the main runtime binary.

The checked path begins with existing in-progress work at admission. Starting a
planned item also needs a progress transition. Automatic recovery covers unchanged
source revision conflicts and explicit context-budget errors; source edits, draft
activation and occupied claims require reviewed input or another actor's action.

The separate governance comparison retains its JSON measurement. Its measured
unchecked governed flow takes 7 calls, and checked completion takes 8 calls, with
542 bytes of guidance. The extra-call ratio is 1/7 (0.143), within its 0.2 target.
It includes preparation, a failed baseline, inline admission, claiming, progress,
acceptance and combined completion/session close-out. Git commits are fixture
implementation work, recorded separately from AWR calls.

The current checked workflow benchmark uses the same native source revision and
one repetition per strategy on macOS arm64. All 12 workload/strategy runs satisfy
the artifact, evidence, identity and recovery checks. Its checked-completion
subtotals are:

| Strategy | Tool calls, including AWR CLI and MCP | Returned tool-text bytes |
| --- | ---: | ---: |
| Primitive | 21 | 94,962 |
| Prepared | 19 | 93,810 |

These totals include onboarding, independent refusal probes, final verification
and reviewer close-out. They have a broader denominator than the six author calls
in the CLI budget. The existing 30-run aggregate above retains its original
five-workload source binding.
