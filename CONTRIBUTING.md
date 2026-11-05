# Contributing

Build with the Rust toolchain pinned in `rust-toolchain.toml`:

```sh
cargo build --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo fmt --all --check
python3 scripts/check_public_tree.py
```

Use Python 3.11+ and `pip install -r requirements-dev.txt` for Python fixture checks.
The concentrated runner also requires `rtk` and is `python3 tests/regression/verify.py --output .local/regression-001`.
Use a new output directory for each run. Test specifications under `tests/` describe
checks; they do not claim that any release or real-client acceptance has passed.

Send a focused PR describing the behavior change and relevant verification.
Use synthetic inputs for reproductions. Preserve original source files unless a
source mutation was explicitly requested, and cover stale revisions and interrupted
writes when changing persistence behavior.

Internal development plans, work ledgers, review notes, transcripts and raw run
records stay local in `.local/` or the ignored root directories. Public test data,
user documentation and reproducible aggregate results are welcome. The public-tree
check prevents known internal paths from entering the Git index, including files
added with `git add -f`; review still needs to catch private material under new names.

Use [GitHub issues](https://github.com/originoneai/agent-work-runtime/issues) for
bugs and proposals. Never include credentials or private project sources. By
contributing, you agree to license your contributions under Apache-2.0.

## Language conventions

Write code comments, docstrings, test names, assertion diagnostics, and developer-facing
messages in English. Keep localized documentation (such as Chinese READMEs),
translation catalogs, and intentional multilingual parser or Unicode fixtures in
their original language. Do not translate user-provided project content.

Project Progress Overview interface text belongs in `tools/project-progress-overview/public/locales/`.
Provide matching English and Simplified Chinese keys and interpolation placeholders. Run
`node --test test/*.test.js` from `tools/project-progress-overview` after changing the maps.

Observer owns its Team bridge and observation contract in `tools/observer/server/`.
Run its frontend checks and production build, followed by `node --test test/*.test.js`
from `tools/observer`, when changing the workspace or its service contracts.
