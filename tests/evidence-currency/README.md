# Evidence currency verification

This contract exercises the issue #345 semantics around a per-work-item section
fingerprint. It keeps the exact completion report and its source binding as the
proof of acceptance, while allowing a work-aware evidence query to survive an
unrelated edit to the same ledger. The ledger exception requires a fresh source
projection, a stamped record, the current ledger SHA and an unchanged work
section. It does not bless an arbitrary Git/code revision, a changed contract,
or a stale projection. Generic evidence reads remain SHA-strict.

The runner accepts a dirty local checkout. It records `HEAD`, the complete
porcelain status, hashes for the contract inputs and all unignored worktree
files, command logs, and the two native binaries. Use a new ignored output
directory for every run:

```sh
python3 tests/evidence-currency/verify.py --output .local/evidence-currency-001
```

The runner uses the Rust toolchain pinned in `rust-toolchain.toml`, always builds
its own native CLI and MCP binaries, runs the focused
core/store/source/runtime/context tests,
runs the dedicated ignored crash-to-CLI recovery test with `AWR_RECOVERY_CLI`,
then runs the native CLI and MCP integration targets and executes the standalone
POC. It resolves binaries from Cargo metadata, so `CARGO_TARGET_DIR`
is supported for container or external build volumes. The runner copies each
built executable into the receipt directory, verifies the copy against the
build artifact, and uses this fixed snapshot for crash recovery and the POC.
Later Cargo test builds cannot replace the binary under that POC. To run the
standalone POC against a manually selected executable, invoke `poc.py` directly:

```sh
python3 tests/evidence-currency/poc.py \
  --awr target/debug/awr \
  --output .local/evidence-currency-poc-manual-001
```

`poc/report.json` must contain `version: 1`, `passed: true`, the four unique
passed scenarios `cli_full_flow`, `cross_instance_transport`,
`stale_revision_retry`, and `git_revision_strict`, plus `steps`,
`inputs_unchanged`, `input_hashes`, `binary_sha256`, `binary_unchanged`, a
`binary_identity` object, a `platform` object, `source_dirty`, and
`verification_kind: synthetic_cli_process_poc`. The runner validates the POC
receipt and copies its hash into the main report. The POC uses synthetic
projects and the real `awr` binary; it makes no model or network calls.
Each scenario must also supply the contract's required observations. The
cross-instance scenario explicitly reopens the imported delivery, runs the
checker again, and accepts a new local completion receipt. It transfers only
ledger/report bytes and checks that the runtime identities remain independent.
The concurrent CLI test repeats two competing writers across eight fixtures;
exactly one writer can commit each shared revision.

For a target that is not the host platform, request compile-only evidence:

```sh
python3 tests/evidence-currency/verify.py \
  --target x86_64-unknown-linux-gnu \
  --output .local/evidence-currency-x86-build-001
```

That mode records `compile_only` and reports runtime tests and the POC as
`not_run` or `unsupported`. A successful compile-only build returns exit code
zero only when `compile_verification_passed: true`; the receipt separately
records `compile_passed: true` and `native_tests_passed: false`. It cannot pass
the native evidence contract. The five native platform jobs invoke the existing
platform runner, which runs this POC against the binaries built on that exact
host and records the platform receipt.

## Compilation and platform execution

Compile-only mode builds the entire workspace with `--all-targets --locked`,
including test binaries and examples. Rust standard-library targets alone do
not supply a C compiler, linker or operating-system SDK.

| Rust target | Required compiler environment | Runtime verification |
| --- | --- | --- |
| `aarch64-apple-darwin` | macOS SDK and Apple Clang | Native macOS arm64 job |
| `x86_64-apple-darwin` | macOS SDK with x64 support | Native Intel job; Rosetta is a separate translated run |
| `aarch64-unknown-linux-gnu` | Linux arm64 GNU compiler and libc headers | Linux arm64 job or an explicitly recorded arm64 container run |
| `x86_64-unknown-linux-gnu` | Linux x64 GNU compiler and libc headers | Native Linux x64 job; cross compilation supplies no execution result |
| `x86_64-pc-windows-msvc` | Windows MSVC Build Tools and Windows SDK | Native Windows x64 job |

The existing [five-platform workflow](../../.github/workflows/platform.yml)
builds and runs workspace tests on each native runner, then executes this POC.
It requires a clean committed checkout. The focused runner above accepts local
uncommitted changes and binds them by hash. A local run does not trigger CI.

For an arm64 Docker host, the pinned [Dockerfile](Dockerfile) supplies the Linux
arm64 compiler plus the Linux x64 cross compiler. Keep Cargo output and raw
receipts on separate ignored writable volumes; mount the source read-only:

```sh
docker build --platform linux/arm64 -t awr-evidence-build:rust-1.93.1 tests/evidence-currency
mkdir -p .local/evidence-currency-linux-cache .local/evidence-currency-container-runs
docker run --rm --platform linux/arm64 \
  -v "$PWD:/src:ro" \
  -v "$PWD/.local/evidence-currency-linux-cache:/build" \
  -v "$PWD/.local/evidence-currency-container-runs:/src/.local/evidence-currency-container-runs" \
  -e CARGO_HOME=/build/cargo -e CARGO_TARGET_DIR=/build/target -e CARGO_BUILD_JOBS=4 \
  -w /src awr-evidence-build:rust-1.93.1 \
  python3 tests/evidence-currency/verify.py \
  --output .local/evidence-currency-container-runs/linux-arm64-001
```

Use the same container command with `--target x86_64-unknown-linux-gnu` and a
new output directory for Linux x64 compilation. The image digest pins the Rust
base image; Debian package updates are not frozen. Record the resulting image
ID with the execution receipt. Running this arm64 image on an x64 host requires
emulation and must be labeled accordingly.

The evidence level is local component/process/CLI/MCP/POC coverage. It does not
claim model usage, real-agent activation, independent business review, E4,
performance, release, or native results for a platform that did not execute.
