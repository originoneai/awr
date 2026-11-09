#!/usr/bin/env python3
"""Validate AWR-DEC-010 assessment contract fixtures (offline, no network)."""
from __future__ import annotations
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DOC = ROOT.parents[3] / "docs" / "dev" / "reference" / "assessment.md"
REQUIRED = [
    "manifest.json",
    "baseline-inventory.json",
    "schema.json",
    "reason-codes.json",
    "layers.json",
    "read-boundaries.json",
    "legacy-compat.json",
    "consumers.json",
    "unknown-fields.json",
    "examples.json",
]


def load(name: str):
    return json.loads((ROOT / name).read_text())


def require(cond: bool, msg: str):
    if not cond:
        raise AssertionError(msg)


def main() -> int:
    for name in REQUIRED:
        require((ROOT / name).is_file(), f"missing {name}")
    manifest = load("manifest.json")
    sha = manifest["verified_main_sha"]
    require(len(sha) == 40 and all(c in "0123456789abcdef" for c in sha), "verified_main_sha")
    require(DOC.is_file(), "docs/dev/reference/assessment.md missing")
    doc = DOC.read_text()
    require(sha in doc, "assessment.md must cite verified_main_sha")
    require("decision show" in doc and "not" in doc.lower(), "decision show boundary")

    schema = load("schema.json")
    require(schema["schema_id"] == "awr-assessment-envelope-v1", "schema id")
    require(schema["layer_ids"] == [
        "work_readiness",
        "execution_admission",
        "context_completeness",
        "delivery_observation",
        "completion_validity",
    ], "five layers")
    require(schema["management_execution_admission"] ==
            "not_granted_by_management_classification", "admission constant")
    require(schema["management_completion_policy"] == "unchanged_source_policy", "completion constant")
    require(schema["action_guidance_max_bytes"] == 1024, "guidance budget")

    reasons = load("reason-codes.json")
    require(reasons["verified_main_sha"] == sha, "reason sha")
    require(len(reasons["unknown_observation_fields"]) == 7, "seven unknown fields")
    require("independent_work_units" == reasons["unit_trigger"]["code"], "units code")
    require(set(reasons["host_assertion_triggers"].values()) >= {
        "multiple_outcomes",
        "scope_requires_planning",
        "handoff_or_collaboration",
        "deferred_wait",
        "plan_invalidated",
        "query_outcome_before_retry",
    }, "host triggers")

    layers = load("layers.json")
    require([L["id"] for L in layers["layers"]] == schema["layer_ids"], "layer inventory")
    require(layers["first_batch_explanations"] == [
        "management_assessment",
        "action_rationale",
    ], "first batch only")

    bounds = load("read-boundaries.json")
    require("model_provider_or_network_scoring" in bounds["forbidden"], "no model")
    require("use_decision_show_or_markdown_decisions_as_envelope_authority" in bounds["forbidden"],
            "no decision show")

    legacy = load("legacy-compat.json")
    require(legacy["no_new_command_names"] is True, "no new commands")
    require("decision show" in legacy["do_not_occupy"], "do not occupy decision show")
    require(legacy["preferred_delivery"]["capability_id"] == "assessment.explain", "capability")

    consumers = load("consumers.json")
    require(consumers["count"] == 2 == len(consumers["consumers"]), "two consumers")
    ids = {c["id"] for c in consumers["consumers"]}
    require(ids == {"cli", "mcp"}, "cli+mcp")

    unknown = load("unknown-fields.json")
    require(unknown["layer_default_when_unchecked"] == "not_evaluated", "not_evaluated default")
    require(all(v == "unknown" for v in unknown["examples"].values()), "examples unknown")

    examples = load("examples.json")
    env = examples["minimal_undetermined"]
    require(env["schema_id"] == schema["schema_id"], "example schema")
    require(set(env["layers"]) == set(schema["layer_ids"]), "example layers")
    require(env["management"]["decision"]["mode"] == "undetermined", "undetermined example")
    require(env["management"]["decision"]["execution_admission"] ==
            schema["management_execution_admission"], "example admission")
    require(all(v == "unknown" for v in env["unsupported_fields"].values()), "unsupported unknown")
    require("general_scheduler" not in json.dumps(env), "no scheduler in example")
    require("AWR-DEC-011" in manifest["out_of_scope"], "DEC-011 not started")

    baseline = load("baseline-inventory.json")
    require(baseline["verified_main_sha"] == sha, "baseline sha")
    require({s["id"] for s in baseline["tip_surfaces"]} ==
            {"management", "prepare", "status", "context"}, "four surfaces")

    for entry in manifest["fixtures"]:
        require((ROOT / entry["path"]).is_file(), entry["path"])
        body = load(entry["path"])
        if "verified_main_sha" in body:
            require(body["verified_main_sha"] == sha, f"sha drift in {entry['path']}")

    print("assessment contract fixtures OK")
    print(f"verified_main_sha={sha}")
    print(f"fixtures={len(manifest['fixtures'])}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001 — fixture gate should print the assertion
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
