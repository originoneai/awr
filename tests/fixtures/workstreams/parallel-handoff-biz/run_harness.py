#!/usr/bin/env python3
"""Harness entry for workstream parallel-handoff BIZ acceptance (AWR-WS-051).

Runs structural verification + deliverable materialization. Optionally probes
local Inspector/Team Web if marked available. Does NOT forge live gate passes.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from datetime import datetime
from pathlib import Path
from urllib.parse import urlparse
from zoneinfo import ZoneInfo

from support import GATES, HARD_GATES, ROOT, load_bundle, materialize_deliverable, read_json
from verify import main as verify_main


def probe_pg(url: str) -> dict:
    parsed = urlparse(url)
    host = parsed.hostname or ""
    if host not in {"127.0.0.1", "localhost", "::1"}:
        return {"ok": False, "reason": "refusing non-loopback PG probe for acceptance harness"}
    try:
        out = subprocess.check_output(
            ["psql", url, "-v", "ON_ERROR_STOP=1", "-c", "select 1 as ok;"],
            text=True,
            stderr=subprocess.STDOUT,
            timeout=15,
        )
        return {"ok": "ok" in out, "detail": "loopback select 1 succeeded", "read_only": True}
    except Exception as exc:  # noqa: BLE001
        # Never serialize URL/password material
        return {"ok": False, "reason": type(exc).__name__}


def probe_inspector() -> dict:
    """Best-effort local Observer presence check (no login, no forged ops)."""
    inspector = ROOT / "tools/observer"
    if not inspector.exists():
        return {"ok": False, "reason": "tools/observer not present"}
    return {
        "ok": True,
        "path": "tools/observer",
        "live_web_ops": False,
        "note": "Observer tree present; human Web operator still required for live gates",
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--evidence-root",
        type=Path,
        default=ROOT / ".local/awr-workstream-implementation-20260921/business/scope-r2",
    )
    parser.add_argument("--skip-pg-probe", action="store_true")
    args = parser.parse_args(argv)

    materialize_deliverable(args.evidence_root)
    rc = verify_main([
        "--evidence-root", str(args.evidence_root),
        "--write-reports",
    ])
    if rc != 0:
        return rc

    catalog, contract, specs = load_bundle()
    now = datetime.now(ZoneInfo("Asia/Taipei")).isoformat(timespec="seconds")
    pg = {"ok": False, "reason": "not probed"}
    if not args.skip_pg_probe:
        url = os.environ.get("AWR_TEAM_TEST_DATABASE_URL")
        if url:
            pg = probe_pg(url)
        else:
            pg = {"ok": False, "reason": "AWR_TEAM_TEST_DATABASE_URL unset"}

    inspector = probe_inspector()

    harness = {
        "work": "AWR-WS-051",
        "schema": "workstream_parallel_handoff_biz_acceptance",
        "scope_revision": 2,
        "recorded_at_cst": now,
        "contract_id": contract["contract_id"],
        "fixture_verification": "passed",
        "deliverable_materialized": True,
        "pg_probe": pg,
        "inspector_probe": inspector,
        "named_clients": contract["named_clients"],
        "required_surfaces": contract["required_surfaces"],
        "hard_gates": HARD_GATES,
        "trial_stack": {
            "team_web": "docs/integrations/team-web-entry.md",
            "named_host": "docs/integrations/named-agent-host.md",
            "eta": "docs/reference/eta-checkpoints.md",
            "member_handoff": "docs/reference/team-member-handoff.md",
            "spun_up_in_this_run": False,
            "reason": "Sandbox lacks two named Agent client operators + human Web operator + independent reviewer",
        },
        "scenarios": {},
        "status": "blocked_pending_trial_participants",
        "missing_resources": [
            "two distinct named Agent clients operated as codex_cli and claude_code",
            "human Team Web operator performing real UI ops per scenario",
            "independent reviewer person (not the executor; not the same-person second Agent)",
            "per-scenario independent project fixture execution with two meaningful rework rounds",
            "per-scenario delivery commit + verified remote SHA",
        ],
        "gates_policy": "待验 is not pass; do not SQL-mutate business state; do not auto-pass missing live client gates",
    }

    for sid, (scenario, directory, result) in specs.items():
        report = read_json(args.evidence_root / directory.name / "report.json")
        gate_matrix = {
            gate: {
                "status": report["gates"][gate]["status"],
                "evidence_paths": report["gates"][gate].get("evidence_paths", []),
                "blocker": report["gates"][gate].get("blocker"),
            }
            for gate in GATES
        }
        harness["scenarios"][sid] = {
            "namespace": scenario["namespace"],
            "hard_gate": result["hard_gate"],
            "persons": result["persons"],
            "clients": result["clients"],
            "surfaces": result["surfaces"],
            "gates": gate_matrix,
            "passed_gates": [g for g, row in gate_matrix.items() if row["status"] == "passed"],
            "pending_gates": [g for g, row in gate_matrix.items() if row["status"] == "待验"],
        }

    business_root = args.evidence_root.parent
    out = business_root / "harness-run.json"
    out.write_text(json.dumps(harness, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "status": harness["status"],
        "fixture_verification": "passed",
        "pg_probe_ok": pg.get("ok"),
        "inspector_present": inspector.get("ok"),
        "scenarios": list(specs),
        "hard_gates": HARD_GATES,
        "report": str(out.relative_to(ROOT)),
    }, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
