# Named agent host adapters (WS-024)

AWR coordinates work; coding agents execute it. This page documents **capability
negotiation** for named controlled adapters and the always-available **L0 manual
report** path. It does not make AWR a process supervisor.

See also: [host integration layers](README.md), [Codex L1 + L2](codex.md),
[Cursor L1](cursor.md), [session workflow (L0)](session-workflow.md).

## Capability matrix

Each adapter declares these capabilities independently:

| Capability | Meaning |
| --- | --- |
| `start` | Host can launch the coding-agent client |
| `status_read` | Host can read status of a known execution |
| `stop_confirmation` | Host can confirm a stop request was acknowledged (**not** OS kill) |
| `reconnect_resume` | Host can rebind an existing native session |
| `result_forensics` | Host can collect logs/receipt references without re-running |

Unsupported capabilities return a **human continuation** instruction. Not every
host is auto-startable.

**Coordination ≠ process control.** AWR admission, cancel request, and session
end are never treated as process start or kill.

## Built-in clients

| Adapter ID | Mode | Auto-start | Notes |
| --- | --- | --- | --- |
| `manual_report` | L0 manual/client report | no | `ExternalExecutionReport` via `awr execution report` |
| `codex_cli` | Named controlled | yes (capability-flagged) | Full matrix including start/stop confirmation |
| `claude_code` | Named controlled | **no** | Status, reconnect, forensics; start via human/L0 |

Fixture and unit coverage verify both named clients are usable for negotiation
even when process launch is stubbed behind capability flags.

## L0 manual reporting

```sh
awr --json execution report --expected-revision N --input report.json
```

The report is an observation. It does not start a supervisor or settle managed
local executions. Prefer L0 when the host cannot auto-start.

## Controlled subtask parallelism

Independent child tasks under a parent may run in parallel:

1. Each subtask has **task identity**, its **own claim**, and **resource bounds**.
2. Hard dependencies decide start order; independent work may overlap.
3. Explicit **concurrency caps** and **user pause** gate new starts.
4. Parent **rollup references** child outcomes; it does **not** copy child artifacts.
5. Parent **session exit** does **not** auto-complete or release unknown children.
6. **Reconnect/retry** queries the original execution first — no duplicate starts.
7. Internal read-only helper calls may stay execution detail (no extra claim).

Rust entry points: `awr_runtime::host_adapter::{built_in_registry, ParallelScheduler}`
and `awr_server::named_agent_host::named_agent_host_capabilities()`.

## Contract

Machine-readable rules:
[`tests/fixtures/workstreams/named-agent-host/awr-workstream-isolation-v1.json`](../../../tests/fixtures/workstreams/named-agent-host/awr-workstream-isolation-v1.json).
