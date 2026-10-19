# Native npm and PyPI distributions

AWR 0.6.0 is a stable package release. npm uses the `latest` channel and PyPI uses
`0.6.0` without a prerelease suffix. Both install the same Rust CLI and MCP server;
no separate JavaScript or Python SDK is included.

```sh
npm install -g @originoneai/agent-work-runtime@0.6.0
# or, in a virtual environment
python -m pip install agent-work-runtime==0.6.0
awr --version
awr-mcp --version
```

| Target | Compatibility |
| --- | --- |
| macOS arm64 | macOS 15+; `macosx_15_0_arm64` wheel |
| macOS Intel x64 | macOS 15+; `macosx_15_0_x86_64` wheel |
| Linux x64 GNU | glibc 2.39+; `manylinux_2_39_x86_64` wheel |
| Linux arm64 GNU | glibc 2.39+; `manylinux_2_39_aarch64` wheel |
| Windows x64 | `win_amd64` wheel; native checks run on Windows Server 2025 |

Both Linux targets need glibc 2.39 or newer, the Ubuntu 24.04 baseline. Pip rejects the
`manylinux_2_39_x86_64` and `manylinux_2_39_aarch64` wheels on older systems, and the npm
launcher checks the runtime glibc before resolving the native package, failing with
`this Linux distribution requires glibc 2.39 or newer (Ubuntu 24.04 baseline).` Debian 12
and the Raspberry Pi OS releases based on it (glibc 2.36) are out of scope. Check a target
machine with `ldd --version | head -n 1` or
`node -p "process.report.getReport().header.glibcVersionRuntime"`.

Python launchers require Python 3.9+, npm launchers Node 22.14+. Distribution checks
run Python 3.12 and Node 24. Other architectures and older operating systems need
separate verification. Git-bound operations require Git; SQLite is bundled.

The npm wrapper depends on exact-version optional native packages ending in
`-darwin-arm64`, `-darwin-x64`, `-linux-x64-gnu`, `-linux-arm64-gnu` and `-win32-x64`. Keep optional dependencies enabled.
There is no install script or network downloader. Wheels embed the binaries.
Native payloads include source identity, binary hashes and dependency license notices.
Only explicit packaging inputs and binaries are included; internal records and AWR
runtime databases are excluded.

## Build and verify

Use a separate environment with `scripts/release/requirements.txt`. On Linux also
install `auditwheel` and `patchelf`. From a clean committed source tree:

```sh
python scripts/release/build_packages.py --output .local/distribution-001
python scripts/release/smoke_install.py .local/distribution-001
```

Each run uses a new output directory. The installation check uses isolated pip/npm
installs, verifies both native versions and errors, initializes a Unicode project,
compiles task context, checks intake diagnosis and verifies the exact 32-tool stdio
MCP catalog. Shared HTTP exposes 30 tools, including project discovery. Previously
published versions retain their release catalog.
These are installation checks, not proof of every real-agent business scenario.

## Publish reviewed builds

The `distributions.yml` workflow builds and checks each native platform. Ordinary
push and PR runs never publish. Manual publication is restricted to the `main`
branch and the `package-registries` environment. npm and PyPI have independent jobs
so an authentication failure in one registry does not hide the other's result.
Trusted publishers bind this repository, workflow and environment.

The assembler requires matching source/version identities, clean trees, matching
artifact hashes and successful installation receipts for all five platforms. It
produces six npm archives, five wheels, a release manifest and `SHA256SUMS`.
Native archives and wheels carry Apache-2.0 and applicable third-party notices.

Stable versions publish with npm `latest`; development/prerelease versions use
`next` and their PEP 440 equivalent. A release number cannot be overwritten. If an
upload partly succeeds, compare the actual registry bytes before any retry. Use the
verified assembled archives for a manual npm upload if interactive authentication
is required; never replace them with a fresh unverified build.

After publication, download all eleven registry files, compare hashes, and install
both channels into fresh environments. GitHub release assets contain the same
packages, checksums and aggregate manifest. Raw execution records stay local or in
isolated CI artifacts and are not committed to the repository.


## Pinned host payload (0.6.0)

AWR `0.6.0` uses database schema 14. Host applications can bundle the same native CLI and MCP server without the registry launchers. A locally built payload has its own recorded source and artifact identity. The matching macOS arm64 or Intel x64 payload can be embedded directly in an application; runtime users need no Node/Python/Rust installation and no PATH changes. The build/verification machine still needs its development tools. The host is responsible for application signing, notarization and updating its bundled binary.

From clean committed source:

```sh
python scripts/release/build_host_bundle.py --output .local/host-payload-001 --batch candidate-001
```

The output directory is new and never overwritten. Its archive name binds purpose, version, platform, source SHA and batch. The payload contains `bin/awr`, `bin/awr-mcp`, `build.json`, `capabilities.json`, `SHA256SUMS`, Apache-2.0 and dependency license notices. `host-manifest.json` binds the archive digest and every payload file. Verify these hashes and the source commit before embedding; keep the same reviewed bytes through signing and installation, recording any signing changes separately. Both macOS architectures require macOS 15+. Other target builds and application integrations require their own native receipts.

Invoke the executable by its absolute application-resource path with `--project <original project root> --json`. Capability negotiation requires no project, database, model configuration, shell, or language interpreter. Initialization, source reads and work context likewise run natively; Git is a separate optional requirement for Git-bound workflows. SQLite is bundled. Do not use `npx`, `pip`, a shell command string or an opportunistic downloader as the embedded invocation path.

## Upgrading from 0.5.x (schema 7 → 14)

AWR 0.5.x uses SQLite schema 7; AWR 0.6.0 uses schema 14. The first explicit
write to the project database with 0.6.0, normally `awr source reindex`, applies
migrations 008 to 014 in order. Each migration only **adds new tables and
indexes**; no existing table, column or row is changed, and project, work, source
and evidence IDs are preserved. Read-only commands and previews never migrate the
file on disk.

| Migration | Adds | Used by |
|---|---|---|
| 008 responsibility | persons, person–agent bindings, task responsibilities and collaborators, with event and receipt tables | Team collaboration model; empty in personal use |
| 009 operation readset receipts | readset receipts for operations | Personal runtime (consistent reads for operations) |
| 010 agent authorization | agent authorizations and receipts | Team collaboration model; empty in personal use |
| 011 team handoff | Team handoffs and receipts | Team only; used only when the [Team runtime switch](../reference/team-switch.md) is on |
| 012 delivery dependencies | hard delivery dependencies, export authorizations, adoption credentials and receipts | Cross-workstream delivery adoption; empty unless that feature is used |
| 013 usage time | usage receipts, corrections, allocations, counter snapshots, execution intervals | Personal runtime (observed usage and time on work) |
| 014 ETA checkpoints | ETA forecasts, sample ledger, acceptance data, ingest receipts | Personal runtime (ETA and stage checkpoints in navigation) |

Tables that a personal installation never uses stay empty and cost nothing at
runtime.

**Before upgrading**

1. Stop every writer: CLI loops, `awr-mcp` servers (stdio and shared HTTP), host
   applications and supervisors.
2. Back up with the **old** 0.5.x binary, which still opens schema 7:

   ```sh
   awr runtime backup --project /path/to/project --output /path/to/new-backup-dir      --companion "$(command -v awr-mcp)"
   awr runtime check --project /path/to/project --backup /path/to/new-backup-dir
   ```

   At minimum, keep a consistent copy of `.awr/state.db` made with SQLite's
   backup API (copying a live file without its WAL can lose history), plus
   `.awr/project.toml`, every registered source and the 0.5.x executables.
3. Install 0.6.0, run `awr --version`, then `awr source reindex` once from the
   project root and check `awr status`.

**Rollback**

0.5.x binaries refuse schema 14, and installing the old package does not make the
database readable again. To roll back, stop all writers, keep a copy of the
upgraded `.awr/` (it holds any history written after the upgrade), then restore
the pre-upgrade backup with the matching 0.5.x program
(`awr runtime restore-preview` and `awr runtime restore`), following the checklist
below. Don't overwrite sources that changed after the upgrade without reviewing them.

## Matched upgrade and rollback checklist

AWR 0.6.0 exposes a native `runtime.matched_snapshot` capability for
`runtime binding`, `backup`, `check`, `restore-preview`, `restore`, `restore-status`
and `restore-recover`. See the [snapshot contract](../reference/runtime-snapshots.md)
for exact program/configuration/source matching and offline history restoration.
Cross-schema downgrade and host files
outside `.awr` still require the broader matched inventory below.

Use the same project directory and identity. Arbitrary relocation, cloud synchronization and long-lived dual writers are outside this contract.

1. Quiesce host/CLI/MCP writers and execution supervisors. Record the one application owner that will resume writes. Negotiate the candidate's capabilities/schema before opening the existing database; retain the previous executable and its source/version/hash. Check the current database's integrity and foreign keys with its matching program.
2. Retain an immutable baseline of the program(s), `.awr/project.toml`, every registered original source (including explicitly allowed external paths), and **all** runtime files. Use SQLite's backup API for a consistent `state.db` snapshot; copying a live database while ignoring its WAL can lose history. Events, checkpoints, sessions, claims, evidence and artifacts are not disposable projections. Include managed artifacts, execution references, mutation/checkpoint journals and retained recovery bytes; keep secrets local. Bind file hashes, schema, project/root identity and table/record inventories in the snapshot manifest.
3. Test the candidate against an isolated fixture/candidate environment first. At the original project root, use the explicit source refresh/initialization path to migrate eligible older schemas, then compare project/work/source IDs and historical rows. Source mapping changes need their own reviewed configuration change. Do not silently interpret old completion flags as current evidence.
4. On failure, stop candidate writes and retain the entire current runtime plus all post-upgrade user changes before rollback. Verify the baseline hashes. Restore its matching program, database/runtime history, configuration and necessary source snapshots together. Compare each current original source to the recorded expected version first; changed content requires explicit review, never blind overwrite. Restore only listed original files and keep newly added user/APP files. A program downgrade alone cannot make a newer schema readable. Retain the upgraded runtime separately so new history is not discarded. Verify the restored schema/integrity and original project IDs before reopening one writer.
5. Keep pre-AWR APP records in read-only storage with stable reference mappings. Reindex the authoritative project sources and link only semantics that are actually understood. Legacy completion reports and native-session references never become verified completion or resumable AWR sessions merely by import. Switch the APP's write path once; do not maintain two authoritative writers indefinitely.

The fixture-only rehearsal is reproducible with an older compatible executable:

```sh
python scripts/release/host_upgrade_drill.py --old-awr /absolute/retained/awr --new-awr /absolute/payload/bin/awr --output .local/upgrade-drill-001
```

It creates new isolated projects, captures a matched SQLite/runtime/source/program snapshot, upgrades while preserving identities and history, checks old-program rejection for a newer schema, and restores the baseline. It also checks changed-source rollback refusal, preserved new user files, read-only legacy APP records, absence of verification promotion, and isolation between two projects sharing task key `W`. This is release engineering verification, not a backup service or a claim of full APP/native-client business acceptance. Never point this rehearsal at a user's live project.
