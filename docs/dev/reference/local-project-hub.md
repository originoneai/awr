# Local Project Hub

The development native payload embeds Project Hub and Project Progress Overview.
Both npm and Python distributions carry the same native service and page assets.
Python installations use their native executable to serve the pages.

The published 0.6.0 packages predate this feature. Use a build of this source until
a release containing the hub is published.

## Open your projects

```sh
awr overview
```

The command starts or reuses the current user's service and opens its address.
Use `awr overview --no-open` to print the address, or `awr overview --json` for a
machine-readable service receipt.

The landing page lists local projects as cards, ordered by the latest AWR use.
Search matches project names and directory paths. Each card shows its name,
directory and connection state. Discovery time is distinguished from actual use.
Select a connected card to open the project's three progress views, then use
**Back to projects** to return. Pending or attention-required cards provide a
short instruction to give your agent and the current inspection details.

The page refreshes its catalog every ten seconds while visible. The coverage
panel reports the latest background scan, its roots, exclusions and failures.
An isolated user-state directory is explicitly labeled as a separate environment.

## Automatic discovery and Agent intake

The first CLI or installed MCP use starts the local background service. In the
default user-state directory, it scans the current user's home and checks again
every 60 seconds. Discovery recognizes AWR metadata, Git repositories and common
package/build manifests. It reads directory metadata and project identity, without
calling a model or writing target-project files.

Connected projects are registered automatically. Ordinary repositories appear as
**Pending setup**. Incomplete, unreadable or older AWR databases appear as
**Needs inspection**. Their diagnostic remains visible. A discovered directory's
card retains its key when initialization connects the actual project identity.

Agents inspect a specific directory through the normal CLI entry point.

```sh
awr --project /absolute/path/to/project overview inspect --json
```

The compact response gives a condition, observed basis, next command and recheck
trigger. Pending projects use the existing `awr init` preview and acceptance
workflow. Goals come from the user's request and project sources. Missing intent
remains explicit. A connected identity still requires an organization/readiness
check before implementation. Repair guidance starts with `awr doctor`; supported
migrations require a matching backup. An uninitialized local MCP project's
`awr_project_status` response includes the same intake guidance for its bound
directory. Shared MCP clients do not receive the machine-wide project catalog.

To scan additional workspace roots or change the interval, configure them once.

```sh
awr overview discover --root /absolute/workspace --root /another/workspace --interval-seconds 60
awr overview projects --json
```

Roots replace the configured root list. Hidden, dependency, generated, cache and
fixture directories are excluded by default. Child symlinks are skipped. The
coverage report exposes these boundaries and access errors. Scans are bounded to
200,000 directories and depth 32, and incomplete scans report a partial result.
Use `--config` with a complete `DiscoveryConfig` JSON to customize roots,
`exclude_names`, `interval_seconds`, `directory_limit` or `depth_limit`. Custom
`AWR_LOCAL_HOME` environments start with no scan roots until configured.

The service first tries port 7381 on `127.0.0.1`. An occupied port selects an
available loopback port. The service receipt records the actual address. Recovery
tries that recorded port first, preserving existing links when it is available.

## Register projects

Successful initialization and subsequent CLI use register an initialized project.
The installed MCP server registers its bound projects and updates their last-use
time on project calls. Existing projects appear as they are used with this version.
You can also register initialized directories explicitly.

```sh
awr overview register /absolute/path/to/alpha /absolute/path/to/beta
awr overview projects --json
```

Registration checks the canonical directory and the actual AWR project identity.
Aliases of the same directory and identity share a card. A replacement project at
the same path receives a separate identity-bound entry. The prior entry stays
visible as unavailable. Moved or unreadable directories also remain visible.

The catalog stores project identity, name, root and usage timestamps. Project
goals, tasks, sessions and evidence remain in their existing project sources and
runtime. Progress views use AWR's bounded observation API over the selected project's
SQLite projection. A single private database snapshot keeps tasks, dependencies,
claims, sessions and runtime findings on the same revision.

**Refresh map** rereads local recorded state. **Sync project sources** explicitly
refreshes the source projection before reading. Ordinary viewing reports that source
currentness was not checked. Runtime findings remain visible, while full file,
artifact and database-integrity diagnosis is available through `awr doctor`.

```sh
awr --project /absolute/path/to/project overview snapshot --json
awr --project /absolute/path/to/project overview snapshot --refresh --json
```

The source Node viewer negotiates `local.progress_snapshot` and uses the same bulk
read when available. Older CLI versions retain the compatible multi-command read.

## Background service lifecycle

Ordinary AWR CLI invocations automatically start the local service when needed.
Installed MCP project calls do the same. A service terminated by the user or OS is
started again on the next invocation. Startup coordination and the service's
lifetime use OS file locks. A health response binds the running instance to the
private service receipt. Concurrent callers reuse that instance.

```sh
awr overview status --json
awr overview start --json
awr overview stop --json
```

`status` observes the current service. `stop` shuts it down and retains the
catalog. The next ordinary AWR invocation can start it again. Browser opening is
explicit through `awr overview`.

Set `AWR_LOCAL_WEB=0` to disable automatic catalog observation and startup in test
runs or embedded hosts. Explicit `overview` commands remain available. The local
service's own CLI readers use this setting to keep reads independent of startup
and last-use observation.

## State and boundaries

State is stored under `AWR_LOCAL_HOME` when that absolute path is configured.
Otherwise Windows uses `%LOCALAPPDATA%/AWR`, an absolute `XDG_STATE_HOME` uses its
`awr` child, and the fallback is `$HOME/.local/state/awr`.

The directory contains the versioned `projects.json` catalog, discovery settings
and coverage, `service.json`, coordination locks and `service.log`. Version-one
catalogs remain readable and are upgraded on the next catalog write. State belongs
to the current OS user. The HTTP service binds to
loopback and validates request hosts and origins.

Project selection is explicit in every progress request. The browser URL carries
an identity-bound catalog key. Opening another project navigates to a separate
page, keeping each document's request binding fixed. Available project data and
project identity are checked at the request boundary.

## Source development

```sh
cargo build -p awr-cli -p awr-mcp --locked
./target/debug/awr overview
```

The Node source viewer is available for a directly bound project and explicit
display configuration.

```sh
node tools/project-progress-overview/server.js --project /absolute/path/to/project
```

Native package verification installs both real package artifacts in isolated
directories, checks automatic startup and catalog registration, fetches the
bundled hub and overview assets, and confirms that subsequent calls reuse the
running instance. Native integration tests also exercise concurrent startup and
hard-kill recovery with an occupied default port.
