#!/usr/bin/env python3
"""Consistency checks for DEC-012 envelope fixtures (no product side effects)."""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DOC = ROOT.parents[3] / "docs" / "dev" / "reference" / "assessment.md"


def load(name: str):
    return json.loads((ROOT / name).read_text())


def main() -> int:
    manifest = load("manifest.json")
    assert manifest["schema_id"] == "awr-assessment-envelope-v1"
    assert manifest["dec_013_started"] is True
    assert manifest.get("dec_020_started") is True
    assert set(manifest["support_vocab"]) == {
        "supported",
        "unknown",
        "conflicting",
        "unsupported",
    }
    for name in manifest["fixtures"]:
        data = load(name)
        assert "case" in data, name

    support = load("support-vocab.json")
    assert len(support["assessments"]) == 4
    assert {a["support"] for a in support["assessments"]} == set(manifest["support_vocab"])

    hard = load("hard-rule-vs-score.json")
    assert hard["expect"]["hard_gate"] == "reject"
    assert hard["expect"]["soft_scores_may_rank"] is False

    order = load("reason-order.json")
    assert order["expected_order"][0] == "a_conflict"

    limits = load("resource-limits.json")
    assert limits["expect"]["hard_gate_not"] == "pass"

    conf = load("no-confidence.json")
    assert "probability" in conf["forbidden_conclusion_keys"]

    assert DOC.is_file(), "docs/dev/reference/assessment.md missing"
    doc = DOC.read_text()
    assert "DEC-012" in doc
    assert "hard_gate" in doc or "HardGate" in doc or "hard-rule" in doc.lower()
    assert "unknown" in doc and "false" in doc.lower()

    print("DEC-012 envelope fixtures OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
