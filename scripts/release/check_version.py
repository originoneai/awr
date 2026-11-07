#!/usr/bin/env python3
"""Reject mixed binary/package versions before the build or any publication."""
import json
import os
import tomllib
from build_packages import ROOT, PLATFORMS, python_version


def workspace_package_names() -> set[str]:
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
    names: set[str] = set()
    for member in cargo["workspace"]["members"]:
        package = tomllib.loads((ROOT / member / "Cargo.toml").read_text())["package"]
        names.add(package["name"])
    return names


def check():
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    normalized = python_version(version)
    expected = os.environ.get("EXPECTED_RELEASE_VERSION")
    assert not expected or expected == version, "requested version differs from the source tree"
    expected_names = workspace_package_names()
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    packages = [p for p in lock["package"] if p["name"] in expected_names]
    assert {p["name"] for p in packages} == expected_names, (
        f"Cargo.lock awr packages {sorted(p['name'] for p in packages)} "
        f"do not match workspace members {sorted(expected_names)}"
    )
    assert all(p["version"] == version for p in packages), "workspace package versions must match Cargo.toml"
    npm = json.loads((ROOT / "packaging/npm/package.json").read_text())
    assert npm["name"] == "@originoneai/awr", "canonical npm package identity mismatch"
    assert npm["version"] == version
    assert set(npm["optionalDependencies"]) == {
        f"@originoneai/awr-{target}" for target in PLATFORMS
    }
    assert set(npm["optionalDependencies"].values()) == {version}
    tag = "next" if "-" in version else "latest"
    assert npm["publishConfig"]["tag"] == tag
    print(
        json.dumps(
            {
                "version": version,
                "python_version": normalized,
                "npm_tag": tag,
                "workspace_packages": sorted(expected_names),
            }
        )
    )


if __name__ == "__main__":
    check()
