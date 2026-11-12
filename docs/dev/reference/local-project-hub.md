# Local Project Hub

AWR 0.7.0 embeds Project Hub and Project Progress Overview in its native payload.
Both npm and Python distributions carry the same native service and page assets.
Python installations use their native executable to serve the pages.

No separate viewer installation or source checkout is needed.

## Open your projects

```sh
awr overview
```

The command starts or reuses the current user's service and opens its address.
Use `awr overview --no-open` to print the address, or `awr overview --json` for a
machine-readable service receipt.

Installation alone does not start the service. The first ordinary AWR command
(including `awr --version`) or local Agent MCP connection starts it automatically.
After that, open **<http://127.0.0.1:7381>** directly without running
`awr overview`. If the default port is occupied, run `awr overview status --json`
to find the actual URL. The service binds to this computer's loopback interface;
responsive layouts do not make it remotely accessible.

The landing page lists local projects as cards, ordered by the latest AWR use.
Search matches project names and directory paths. Each card shows its name,
directory and connection state. Discovery time is distinguished from actual use.
Select a connected card to open the project's three progress views, then use
**Back to projects** to return. Pending or attention-required cards provide a
short instruction to give your agent and the current inspection details.

The page reads the catalog once when opened. **Refresh projects** runs an explicit
scan and reloads the cards. Search filters the loaded cards locally. The coverage
panel reports the latest scan, its roots, exclusions and failures.
An isolated user-state directory is explicitly labeled as a separate environment.

Each card has a **Delete** button that persistently removes it from this list.
Hidden cards stay hidden across scanning, ordinary AWR use and service restarts.
Use **Take over project** with the same absolute path to restore a card. Removal
updates the current user's display preferences, preserving project files and AWR
state.

## Automatic discovery and Agent intake

The first CLI or installed MCP use starts the local background service. In the
default user-state directory, it scans the current user's home once per service
startup. Further scans are user-triggered through **Refresh projects** or
`awr overview discover`. Automatic discovery recognizes directories that already have
an AWR project manifest or database. Git repositories and package manifests alone
do not enroll a directory. Discovery reads directory metadata and project identity,
without calling a model or writing target-project files.

Directories that already hold an AWR identity appear as connected cards. Incomplete,
unreadable or older AWR databases appear as **Needs inspection**, with their diagnostic
retained. Discovery only finds projects; registering one is a separate, explicit step (see
[Register projects](#register-projects)).

Dated Codex session directories (`Codex/YYYY-MM-DD/...`) are treated as temporary
working areas. Discovery skips them, earlier cards are filtered out, and Hub
enrollment directs the user to a durable project directory. Coverage reports the
number of skipped temporary paths.

To connect a new project, click **Take over project**, enter its absolute directory
path and click **Create pending card**. The directory must already exist. This
creates a **Pending setup** card and records explicit takeover intent in the local
catalog. It does not initialize the project or change its files. Repeated paths
and canonical aliases reuse the same card.

Open that card to copy the Agent guidance. The agent inspects existing sources,
reviews the normal initialization preview and uses the actual user goal before
accepting setup. The card retains its key when the actual AWR identity connects.

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

To scan additional workspace roots, configure them once.

```sh
awr overview discover --root /absolute/workspace --root /another/workspace
awr overview projects --json
```

Roots replace the configured root list. Hidden, dependency, generated, cache and
fixture directories, application bundles and generated evidence are excluded by
default. Recognized Go module caches are skipped. Nested AWR-managed projects
remain independently discoverable. New directories, including ones inside the
scan scope, require explicit takeover enrollment.
Child symlinks are skipped. The
coverage report exposes these boundaries and access errors. Scans are bounded to
200,000 directories and depth 32, and incomplete scans report a partial result.
Use `--config` with a complete `DiscoveryConfig` JSON to customize roots,
`exclude_names`, `directory_limit` or `depth_limit`. Legacy `interval_seconds`
configuration and `--interval-seconds` inputs remain accepted for compatibility
and have no scheduling effect. Custom
`AWR_LOCAL_HOME` environments start with no scan roots until configured.

After a complete scan, earlier unused, unbound automatic suggestions that are
proven to be ordinary directories or excluded artifacts are retired. Explicitly
requested takeover cards and identity-bound entries remain in the catalog. Missing
registered or requested directories remain visible. Partial scans do not retire
suggestions. Project lists expose managed projects and explicitly requested cards.

The service first tries port 7381 on `127.0.0.1`. An occupied port selects an
available loopback port. The service receipt records the actual address. Recovery
tries that recorded port first, preserving existing links when it is available.

## Register projects

Registration is explicit. Ordinary commands, from the CLI or the installed MCP server, do
not add a project to the catalog: a machine that runs throwaway fixtures, copies and agent
scratch directories would otherwise collect all of them. They only refresh the last-use time
of a project that is already registered. Register initialized directories yourself:

```sh
awr overview register /absolute/path/to/alpha /absolute/path/to/beta
awr overview projects --json
```

`Take over project` in the hub registers a project the same way. Set
`AWR_LOCAL_AUTO_REGISTER=1` to register every project an ordinary command touches, as 0.7.0
did.

Registration checks the canonical directory and the actual AWR project identity.
Aliases of the same directory and identity share a card. A replacement project at
the same path receives a separate identity-bound entry. The prior entry stays
visible as unavailable. Moved or unreadable directories also remain visible.

The catalog stores project identity, name, root and usage timestamps. Project
goals, tasks, sessions and evidence remain in their existing project sources and
runtime. Progress views use AWR's bounded observation API over the selected project's
SQLite projection. A single private database snapshot keeps tasks, dependencies,
claims, sessions and runtime findings on the same revision.

Opening a project reads a fresh SQLite snapshot, including when browser history
restores a cached detail page. **Refresh map** rereads local recorded state. **Sync project sources** explicitly
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

The directory contains the versioned `projects.json` catalog, `hidden-projects.json`
display preferences, discovery settings and coverage, `service.json`, coordination
locks and `service.log`. Version-one
and version-two catalogs remain readable and upgrade to version three on the next
catalog write. Version three preserves explicit takeover intent across clients.
Older writers reject the newer catalog version. State belongs
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
