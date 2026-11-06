#!/usr/bin/env python3
"""Collect one fully checked native build per supported platform for publication."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
from build_packages import PLATFORMS, python_version
from mcp_catalog import validate_stdio_tools


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expected-sha", required=True)
    parser.add_argument("--candidate", type=Path, help="also require the recorded candidate artifact hashes")
    args = parser.parse_args()
    manifests = list(args.input.rglob("manifest.json"))
    assert len(manifests) == len(PLATFORMS), "one independent build manifest per supported platform is required"
    selected = {}
    versions = set()
    files = {}
    for path in manifests:
        manifest = json.loads(path.read_text())
        platform = manifest["platform"]
        assert platform in PLATFORMS and platform not in selected, "duplicate or unsupported platform"
        assert manifest["source_sha"] == args.expected_sha and manifest["source_tree_clean"], "build source identity mismatch"
        assert manifest["python_version"] == python_version(manifest["version"]), "registry version mismatch"
        receipt = json.loads((path.parent / "installation-checks.json").read_text())
        assert receipt["source_sha"] == args.expected_sha and receipt["platform"] == platform
        assert receipt["artifact_sha256"] == manifest["artifacts"], "installation proof covers different artifacts"
        for ecosystem in ("npm", "python"):
            check = receipt[ecosystem]
            assert check["version_and_help"] is True and check["exit_code_and_stderr"] is True
            assert check["unicode_space_project_init_and_status"] is True and check["task_context_and_intake"] is True
            validate_stdio_tools(check["mcp_stdio_tools"])
            if manifest.get("local_project_hub", False):
                assert check.get("local_project_hub") is True, "bundled local project hub was not exercised"
        versions.add(manifest["version"])
        for name, expected in manifest["artifacts"].items():
            assert Path(name).name == name and name.endswith((".whl", ".tgz"))
            source = path.parent / "publish" / name
            actual = hashlib.sha256(source.read_bytes()).hexdigest()
            assert actual == expected, f"artifact hash mismatch: {name}"
            if name in files:
                assert files[name][1] == actual, "platform-independent npm wrapper differs across builds"
            files[name] = (source, actual)
        selected[platform] = manifest
    assert set(selected) == PLATFORMS and len(versions) == 1
    assert len({m.get("local_project_hub", False) for m in selected.values()}) == 1, "local hub coverage differs across platforms"
    assert sum(name.endswith(".whl") for name in files) == len(PLATFORMS)
    assert sum(name.endswith(".tgz") for name in files) == len(PLATFORMS) + 1
    if args.candidate:
        candidate = json.loads(args.candidate.read_text())
        assert candidate["source_sha"] == args.expected_sha
        assert versions == {candidate["version"]}
        assert candidate["artifacts"] == {name: sha for name, (_, sha) in files.items()}, "candidate artifact bytes differ"
    args.output.mkdir(parents=True, exist_ok=False)
    for ecosystem in ("npm", "python"):
        (args.output / ecosystem).mkdir()
    for name, (source, _) in files.items():
        ecosystem = "python" if name.endswith(".whl") else "npm"
        shutil.copy2(source, args.output / ecosystem / name)
    receipt = {"version": versions.pop(), "source_sha": args.expected_sha,
               "python_version": selected["darwin-arm64"]["python_version"],
               "channel": "next" if "-" in selected["darwin-arm64"]["version"] else "latest",
               "platforms": sorted(selected), "artifacts": {name: sha for name, (_, sha) in files.items()}}
    if selected["darwin-arm64"].get("local_project_hub", False):
        receipt["local_project_hub"] = True
    (args.output / "release-manifest.json").write_text(json.dumps(receipt, indent=2) + "\n")
    (args.output / "SHA256SUMS").write_text("".join(f"{sha}  {name}\n" for name, sha in sorted(receipt["artifacts"].items())))
    print(json.dumps(receipt))


if __name__ == "__main__":
    main()
