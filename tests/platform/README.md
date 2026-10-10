# Native platform verification

The [contract](contract.json) defines five native targets: macOS arm64 and Intel
x64, Linux x64 and arm64, and Windows x64, using Rust 1.93.1. Each platform runs
all fourteen gates on its own recorded source commit. A workflow definition,
cross compilation or another platform's result cannot supply that evidence.

The gates include subprocess-runner regression, workspace/all-targets build and
tests, doctests, ignored-test inventory and actual CLI crash recovery, all nine
CLI/MCP parity checks, manual checkpoint/resume, and native CLI conditions. The
conditions cover Unicode/space paths, CRLF, WAL/schema integrity, persisted
sessions, source writes and receipts, stale-write rejection, changed hard rules,
outside-source rejection and closed-session health. The final gate checks that
tracked inputs and the source commit remain unchanged.

Run from a committed, clean checkout with a fresh output directory:

```sh
python tests/platform/verify.py --platform macos-arm64 --output .local/platform-local-v1
```

Use `macos-x64`, `linux-x64`, `linux-arm64` or `windows-x64` for other native
targets. The runner checks the actual OS, CPU and Rust host. Parameters cannot
pretend to be another platform. The scripts use Python's standard library and
UTF-8 mode; locally installed RTK proxies external commands.

## Bounded processes and finalized evidence

The default command deadline is 1,800 seconds. Only the Intel workspace gate has
a 3,600-second deadline; its CI job has a 90-minute limit to include compilation
and the remaining checks. Other platform job limits remain 45 minutes. Budgets
are explicit in the contract and per-gate receipt, and do not skip any tests.

Each command owns a POSIX process group or Windows Job Object. A Windows
bootstrap joins its job before spawning the command. After normal exit or a
timeout, the runner terminates remaining contained descendants and reaps the
launcher. It then allows up to ten seconds for cleanup and output drainage.
The runner alone writes the log; the output writer must finish before the log
is hashed. Timeouts always fail. Failed cleanup stops later gates, and an
unfinished log receives no final fingerprint. These checks cover descendants
within the declared process boundary; POSIX children that deliberately create a
new session are outside that boundary and cannot supply a finalized receipt if
they retain its output pipe.

The native subprocess regression gate exercises real child/grandchild processes,
nonzero exits, missing commands, timeouts, orphaned output and stable final logs.
Windows and Intel results require their own native jobs.

## CI receipts and verification limits

[The workflow](../../.github/workflows/platform.yml) runs every platform
independently and uploads failures as well as successes. Actions are pinned;
checkout keeps no credentials. Permissions are limited to `contents: read`.
Uploads contain synthetic verification evidence, not private project state.

Reports bind OS/architecture, compiler, source/tree/input/binary fingerprints,
workflow run/attempt, command deadlines, actual exits, cleanup and finalized log
digests. Rust counts come from executed test results. Unix-only tests omitted by
Windows configuration are not credited there. Seven ignored entries require
actual parent-test execution or the explicit recovery command; listing them does
not prove execution. Partial stdout cannot replace a completed workspace gate.

CLI probes and synthetic lifecycle checks make no Agent model calls. They do not
establish native Agent-client activation, independent business review, E4,
performance results or a release. Coverage is limited to the listed hosted
platforms; older systems, other architectures and network filesystems remain
unverified. Runner image versions are recorded for each job: a fixed hosted
runner label does not freeze the underlying image.
