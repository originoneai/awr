#!/usr/bin/env python3
"""Publish already-assembled npm artifacts; authentication remains external."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
from build_packages import PLATFORMS, python_version


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    parser.add_argument("--provenance", action="store_true", help="enable when publishing from the configured CI identity")
    args = parser.parse_args()
    manifest = json.loads((args.directory / "release-manifest.json").read_text())
    version = manifest["version"]
    python_version(version)  # Reject malformed or unsupported release versions.
    tag = "next" if "-" in version else "latest"
    assert manifest["channel"] == tag, "release channel mismatch"
    assert set(manifest["platforms"]) == PLATFORMS, "release platform set mismatch"
    packages = []
    for file in (args.directory / "npm").glob("*.tgz"):
        assert hashlib.sha256(file.read_bytes()).hexdigest() == manifest["artifacts"][file.name]
        with tarfile.open(file) as archive:
            package = json.load(archive.extractfile("package/package.json"))
        assert package["version"] == version and package["publishConfig"]["tag"] == tag
        assert package["name"] in {"@originoneai/awr", *[f"@originoneai/awr-{platform}" for platform in manifest["platforms"]]}
        packages.append((package["name"], file))
    assert len(packages) == len(PLATFORMS) + 1
    assert {name for name, _ in packages} == {"@originoneai/awr", *[f"@originoneai/awr-{platform}" for platform in manifest["platforms"]]}
    # Installers may fetch the wrapper as soon as it exists, so its dependencies go first.
    packages.sort(key=lambda p: p[0] == "@originoneai/awr")
    for name, file in packages:
        command = ["npm", "publish", str(file), "--access", "public", "--tag", tag,
                   "--registry=https://registry.npmjs.org/"]
        if args.provenance:
            command.append("--provenance")
        subprocess.run(command, check=True)
        print(json.dumps({"published": name, "version": version}))


if __name__ == "__main__":
    main()
