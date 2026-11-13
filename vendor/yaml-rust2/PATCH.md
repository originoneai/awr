# Vendored patch: yaml-rust2 0.12.0 — character-consistent `Marker::index()`

## Why this directory exists

AWR's `crates/awr-source/src/yaml_edit.rs` rebuilds byte-exact positions of the
ledger from `yaml_rust2::parser::Marker` values. It assumes `Marker::index()`
counts **characters** (it maps them through a `str::char_indices()` table).

Upstream yaml-rust2 advances `mark.index` by one per character everywhere
(`skip_blank`, `skip_non_blank`, `skip_n_non_blank`, `skip_nl`) **except** in the
block-scalar content fast path of `scan_block_scalar_content_line`, which adds
`line_buffer.len()` — a `String::len()`, i.e. **bytes**. For any block scalar
(`|` / `>`) whose content contains multi-byte UTF-8 (Chinese text, emoji, …),
every marker after it drifts ahead by the byte surplus. Downstream effects in
AWR before this patch: source mutations refused with
`missing YAML mapping colon` or `YAML marker outside source`, or positions that
fail write-back validation.

A community report hit exactly this on a ledger with a Chinese block scalar
(see AWR work item `WORK-e8d70deb043134db165c2fdf5df1103f`). The upstream
fix is not available: the inconsistency is still present in the latest
release (checked against the published 0.13.0 `src/scanner.rs`), and 0.12.0
only "corrected the documentation" for `Marker::index`
(https://docs.rs/crate/yaml-rust2/latest/source/CHANGELOG.md).

## The patch

Exactly one semantic change, in `src/scanner.rs`
(`scan_block_scalar_content_line`, buffered-EOF fast path):

```diff
             // We need to manually update our position; we haven't called a `skip` function.
-            self.mark.col += line_buffer.len();
-            self.mark.index += line_buffer.len();
+            // `line_buffer` is a `String`, so its `len()` counts BYTES while every other
+            // path advances `mark.index`/`mark.col` by one per CHARACTER. Count characters
+            // here to keep `Marker::index()` consistent for multi-byte content (see PATCH.md).
+            let char_count = line_buffer.chars().count();
+            self.mark.col += char_count;
+            self.mark.index += char_count;
```

`col` is corrected alongside `index` because both are character-counted by the
`skip_*` helpers; leaving `col` byte-counted would break block-scalar
indentation checks for multi-byte lines.

No other file is modified relative to the upstream 0.12.0 package
(`~/.cargo/registry/src/*/yaml-rust2-0.12.0`). `Cargo.toml`, `src/` (except the
hunk above), `tests/`, `examples/`, licenses, `README.md` and `CHANGELOG.md`
are verbatim copies. Packaging/CI helpers are dropped: `appveyor.yml`,
`garden.yaml`, `justfile`, `tools/`, `.github/`, `.cargo/`, `.cargo-ok`,
`.gitattributes`, `.gitignore`, `.gitmodules`, `Cargo.lock` and
`Cargo.toml.orig`. The vendored crate is not a workspace member, so its own
dev-dependency test suites never build here; AWR's regression
`multibyte_block_scalar_positions_stay_exact_for_later_field_edits` is the
executable guard for this patch.

## Wired in via

Root `Cargo.toml`:

```toml
[patch.crates-io]
yaml-rust2 = { path = "vendor/yaml-rust2" }
```

## How to remove this vendored copy

1. Verify the upstream fix: `Marker::index()` must be character-consistent for
   block-scalar content (or explicitly byte-consistent everywhere, together with
   the matching `yaml_edit.rs` offset mapping — either way, consistent).
2. Delete `[patch.crates-io]` from the root `Cargo.toml`, run `cargo update -p yaml-rust2`,
   delete this directory, and keep `crates/awr-source/tests/yaml_mutation.rs`'s
   multi-byte block-scalar regression green.
