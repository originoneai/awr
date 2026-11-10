#!/usr/bin/env python3
"""Consistency checks for DEC-013 counterexample corpus (offline, no product side effects)."""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DOC = ROOT.parents[3] / "docs" / "dev" / "reference" / "assessment.md"
BENCH_DOC = ROOT.parents[3] / "docs" / "dev" / "benchmarks" / "assessment.md"
BENCH = ROOT.parents[3] / "tests" / "benchmarks" / "assessment"
ENVELOPE_MANIFEST = ROOT.parent / "envelope" / "manifest.json"

REQUIRED_CRITICAL = {
    "error_retry",
    "revoke",
    "wrong_task",
    "evidence_withdrawal",
    "missing_fields",
    "delivery_unknown",
    "time_budget",
}


def load(name: str):
    return json.loads((ROOT / name).read_text())


def main() -> int:
    manifest = load("manifest.json")
    matrix = load("coverage-matrix.json")
    assert manifest["schema_id"] == "awr-assessment-counterexample-corpus-v1"
    assert manifest["work"] == "AWR-DEC-013"
    method = manifest["method"]
    assert method["count_separately_from_evo"] is True
    assert method["evo_paid_or_dual_host_required"] is False
    assert method["baseline"] == "same_source_assessment_explain_off"
    assert method["candidate"] == "same_source_readonly_assessment_explain_on"
    assert set(method["costs_recorded"]) == {"collect", "judge", "output"}
    assert manifest["dec_020_started"] is True
    assert manifest["evo_000_started"] is False

    assert len(manifest["fixtures"]) == 20
    assert len(manifest["positives"]) == 5
    assert len(matrix["classes"]) == 20
    assert set(manifest["critical_families_zero_tolerance"]) == REQUIRED_CRITICAL
    assert set(matrix["critical_families_zero_tolerance"]) == REQUIRED_CRITICAL
    assert "hard_constraint_rule" in matrix

    seen_ids = []
    critical_seen = set()
    for name in manifest["fixtures"]:
        assert (ROOT / name).is_file(), name
        data = load(name)
        assert data.get("case", "").startswith("C"), name
        assert data.get("synthetic") is True, name
        assert "expect" in data and "forbidden" in data, name
        seen_ids.append(data["case"])
        row = next(c for c in matrix["classes"] if c["id"] == data["case"])
        assert row["fixture"] == name, (data["case"], row["fixture"], name)
        if data.get("hard_constraint"):
            fam = data.get("critical_family")
            assert fam in REQUIRED_CRITICAL, f"{name} bad family {fam}"
            critical_seen.add(fam)
            assert any(("offset" in k) or ("average" in k) for k in data["forbidden"]), name

    assert seen_ids == [f"C{i:02d}" for i in range(1, 21)]
    assert critical_seen == REQUIRED_CRITICAL, critical_seen

    for name in manifest["positives"]:
        data = load(name)
        assert data.get("kind") == "positive_boundary", name
        assert "expect" in data and "forbidden" in data, name

    p03 = load("p03-hard-reject-not-offset-by-score.json")
    assert p03["expect"]["soft_scores_may_clear_gate"] is False
    assert p03["forbidden"]["average_soft_score_clears_hard_reject"] is True

    p05 = load("p05-metric-families-separated.json")
    assert set(p05["expect"]["metric_families"]) == {
        "structural_correctness",
        "runtime_overhead",
        "advisory_effectiveness",
    }
    assert set(p05["expect"]["unmeasured_not_derived"]) == {
        "model_success_rate",
        "dollar_savings",
        "semantic_understanding",
    }

    c17 = load("c17-few-samples-cancel-or-cost-unmeasured.json")
    assert c17["expect"]["model_success_rate_derived"] is False
    assert c17["expect"]["dollar_savings_derived"] is False
    assert c17["forbidden"]["invent_p_success"] is True

    env = json.loads(ENVELOPE_MANIFEST.read_text())
    assert env["dec_013_started"] is True

    assert DOC.is_file(), "docs/dev/reference/assessment.md missing"
    assert "DEC-013" in DOC.read_text()
    assert "DEC-020" in DOC.read_text()

    assert BENCH_DOC.is_file(), "docs/dev/benchmarks/assessment.md missing"
    bench_doc = BENCH_DOC.read_text()
    assert "baseline" in bench_doc and "candidate" in bench_doc
    assert "structural_correctness" in bench_doc

    for req in ("contract.json", "budgets.json", "compare.py", "verify.py"):
        assert (BENCH / req).is_file(), f"missing benchmark artifact {req}"

    print("DEC-013 counterexample corpus OK")
    print(f"fixtures={len(manifest['fixtures'])} positives={len(manifest['positives'])}")
    print(f"critical_families={sorted(critical_seen)}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
