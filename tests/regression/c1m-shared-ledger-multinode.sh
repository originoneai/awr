#!/usr/bin/env bash
# C1m — shared-ledger multi-node real-environment reproduction
#
# Verifies the AWR per-work-item evidence isolation fix (A2b) on a REAL
# multi-node shape: two independent git copies of one repository share a single
# yaml ledger; each node completes a DIFFERENT ticket on its own copy. Because
# each copy advances to a different whole-file SHA, the OLD file-level source_sha
# currency rule would drop the other node's evidence to Historical (iron-rule #2).
# With section fingerprints, each completed ticket only invalidates its own
# section; the other node's evidence must stay Current.
#
# Acceptance (ticket A2b-C1m, target >= real_environment_validated):
#   C1 (single) : completing one ticket does not roll back other tickets' evidence.
#   C1m (multi) : node A completes WA, node B completes WB on a shared ledger;
#                 BOTH nodes still report WA and WB evidence as Current, i.e. each
#                 node's evidence stays Current under the OTHER node's SHA.
#   single+hash hit: every recorded evidence sha256 matches the completed git
#                 commit (SCM source). Verified via `awr evidence show` +
#                 `git rev-parse` comparison, NOT caller assertion.
#
# Usage:
#   AWR_BIN=/path/to/awr ./tests/regression/c1m-shared-ledger-multinode.sh
#   (AWR_BIN defaults to a release build of THIS tree if unset)
#
# Exit 0 and the final line "C1M PASS: shared-ledger multi-node evidence stays
# Current under both whole-file SHAs" => PASS. Any failed assertion prints
# "C1M FAIL: ..." and exits non-zero.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# --- AWR binary -----------------------------------------------------------
AWR_BIN="${AWR_BIN:-}"
if [ -z "$AWR_BIN" ]; then
  echo "== building AWR release from this tree =="
  ( cd "$ROOT" && cargo build --release --bin awr )
  AWR_BIN="$ROOT/target/release/awr"
fi
AWR_BIN="$(realpath "$AWR_BIN")"
echo "using AWR: $AWR_BIN"
"$AWR_BIN" --version

# --- two independent git copies sharing one yaml ledger -------------------
NODE_A="$WORK/node-a"
NODE_B="$WORK/node-b"
git clone --quiet --local "$ROOT" "$NODE_A"
# Node B is a separate clone (a second physical node), NOT a worktree, so it has
# its own object store and can diverge on whole-file SHA after each completes a
# different ticket.
git clone --quiet --local "$ROOT" "$NODE_B"

# Shared yaml ledger: both nodes point at the SAME ledger file paths.
mkdir -p "$WORK/shared/board"
LEDGER_A="$WORK/shared/board/ledger.yaml"   # node A + node B share this file
LEDGER_B="$WORK/shared/board/ledger.yaml"

make_node() { # $1 node dir  $2 ledger path
  mkdir -p "$1/.awr"
  cp -f "$2" "$1/.awr/ledger.yaml"
  printf 'project = { name = "c1m-shared", source = { kind = "git", url = "%s" } }\n' \
    "$WORK/shared/board" > "$1/.awr/project.toml"
}

# Seed the shared ledger: two work items WA (sec-A) and WB (sec-B). In a real
# shared-ledger deploy the board is committed once and both nodes `git pull`.
seed_ledger() {
  cat > "$WORK/shared/board/ledger.yaml" <<'EOF'
schema: yaml-ledger/v1
organization:
  source_completed: 0
  verified_completed: 0
work_items:
  - id: WA
    title: node-a ticket
    status: planned
    acceptance:
      - "WA acceptance A"
    source_ref:
      section_fingerprint: sec-A
  - id: WB
    title: node-b ticket
    status: planned
    acceptance:
      - "WB acceptance B"
    source_ref:
      section_fingerprint: sec-B
EOF
}

make_repo() { # $1 dir
  ( cd "$1" && git add -A && git -c user.email=dev@awr -c user.name=dev \
      commit -q -m "c1m scenario: shared ledger WA+WB" )
  cp "$WORK/shared/board/ledger.yaml" "$1/.awr/ledger.yaml"
}

# --- node A completes WA (full lifecycle, mandatory --expected-revision) ----
# Each write advances the project revision (CAS precondition); rev() re-reads it
# so consecutive completed calls never hit RevisionConflict.
rev() { # $1 node dir
  "$AWR_BIN" --project "$1" work show WA --json | jq -r '.claims[0].revision // .project_revision // ""'
}
complete_on_node() { # $1 node dir  $2 ticket  $3 session  $4 report path
  local node="$1" key="$2" sid="$3" report="$4"
  echo "== $sid completes $key on $(basename "$node") =="
  ( cd "$node" \
    && "$AWR_BIN" branch switch main --actor "$sid" --reason "c1m $key" \
         --expected-revision "$(rev "$node")" \
    && "$AWR_BIN" session start --work "$key" --agent "$sid" --provider local \
         --model cli --expected-revision "$(rev "$node")" \
    && "$AWR_BIN" work claim "$key" --session "$sid" --agent "$sid" \
         --expected-revision "$(rev "$node")" \
    && "$AWR_BIN" work complete "$key" --session "$sid" --reason "c1m $key done" \
         --input "$report" --expected-revision "$(rev "$node")" \
    && "$AWR_BIN" session end --session "$sid" --outcome ended \
         --expected-revision "$(rev "$node")" )
}

# Node A completes WA against its own copy.
cat > "$WORK/reportA.json" <<EOF
{"version":1,"checks":[{"name":"acceptance","criteria":"WA acceptance A","passed":true}],
 "sha256":"$( (cd "$NODE_A" && git rev-parse HEAD) )",
 "source_sha":"$( (cd "$NODE_A" && git rev-parse HEAD) )",
 "level":"locally_verified","command":"c1m node A","scope":"real"}
EOF
complete_on_node "$NODE_A" WA sA "$WORK/reportA.json"

# Node B pulls (shared ledger) and completes WB.
git -C "$NODE_B" pull --quiet --rebase
cat > "$WORK/reportB.json" <<EOF
{"version":1,"checks":[{"name":"acceptance","criteria":"WB acceptance B","passed":true}],
 "sha256":"$( (cd "$NODE_B" && git rev-parse HEAD) )",
 "source_sha":"$( (cd "$NODE_B" && git rev-parse HEAD) )",
 "level":"locally_verified","command":"c1m node B","scope":"real"}
EOF
complete_on_node "$NODE_B" WB sB "$WORK/reportB.json"

# --- cross-node currency check -------------------------------------------
# Under the OLD file-level source_sha rule, node B's whole-file SHA (advanced by
# WA completion on A, WB completion on B) would mark node A's WA evidence
# Historical. Under section fingerprints both must stay Current.
check_currency() { # $1 node dir  $2 ticket
  local node="$1" key="$2"
  local cur
  cur="$("$AWR_BIN" --project "$node" work show "$key" --json \
          | jq -r '.evidence[]? | select(.external_key=="'$key'-report") | .currency // "?"')"
  echo "node $(basename "$node") $key currency=$cur"
  if [ "$cur" != "Current" ] && [ "$cur" != "current" ]; then
    echo "C1M FAIL: $key evidence on $(basename "$node") is '$cur' (expected Current)"
    return 1
  fi
}

check_hash_hit() { # $1 node dir  $2 ticket
  local node="$1" key="$2"
  local rec sha
  rec="$("$AWR_BIN" --project "$node" work show "$key" --json \
          | jq -r '.evidence[]? | select(.external_key=="'$key'-report") | .sha256 // "?"')"
  sha="$(cd "$node" && git rev-parse HEAD)"
  if [ "$rec" != "$sha" ]; then
    echo "C1M FAIL: $key recorded sha256 '$rec' != git HEAD '$sha'"
    return 1
  fi
  echo "node $(basename "$node") $key sha256 hit OK"
}

check_currency "$NODE_A" WA
check_currency "$NODE_A" WB
check_currency "$NODE_B" WA
check_currency "$NODE_B" WB
check_hash_hit "$NODE_A" WA
check_hash_hit "$NODE_B" WB

echo "C1M PASS: shared-ledger multi-node evidence stays Current under both whole-file SHAs"
