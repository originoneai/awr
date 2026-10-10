# CLI lifecycle example

`lifecycle.py` is the **L0** CLI walkthrough: it invokes AWR, not a coding
agent. The directory name is historical. For host-agnostic commands see the
[L0 session workflow](../../docs/dev/integrations/session-workflow.md). Codex-specific
MCP merge paths and optional hooks remain in the
[L1 + L2 Codex note](../../docs/dev/integrations/codex.md).

- `lifecycle.py`: executable CLI walkthrough using a fresh copy of
  [the basic fixture](../basic/) with this directory's `work-ledger.yaml`, which
  declares the phase required by bootstrap. It retains a receipt for every command.
- `config.toml.example`: grouped Codex stdio template for published AWR 0.5.1
  and this source tree. Replace absolute paths and merge the table into an
  existing trusted project configuration.
- `config.flat.toml.example`: legacy eight-tool allowlist. Use it only when the
  host cannot route through domain tools.
- `AGENTS.snippet.md`: Codex instruction snippet. Copying it does not install
  lifecycle hooks.

From the AWR repository root, with Python 3.9 or later and the Rust toolchain:

```sh
cargo build --locked -p awr-cli -p awr-mcp
python3 examples/codex/lifecycle.py \
  --awr target/debug/awr \
  --output .local/codex-lifecycle-example
```

Choose a new output directory for each run. An existing directory is rejected;
the script never overwrites or cleans an earlier run. The example executes
init preview/accept, session start with a claim, bootstrap, L1 compile,
checkpoint, resume, recovered-context inspection, session end and Doctor. It
checks that the checkpoint's next action/open loop survive and that resume
preserves the claim expiration. The copied source files remain unchanged; the
example ends as incomplete and releases the claim.

On failure, inspect the numbered JSON receipt and the retained project's
`session list`, `session show` and event history. The script does not retry a
mutation or discard a partly completed run. Runtime files and receipts are local
inspection material, not source-ledger completion evidence.

This script invokes AWR, not a coding agent or a model. Provider/model values are
recorded metadata. Running it from any host terminal establishes a terminal
workflow; native MCP discovery, lifecycle hooks and real business acceptance
require their own evidence. See [host integration layers](../../docs/dev/integrations/README.md).
