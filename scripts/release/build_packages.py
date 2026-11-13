#!/usr/bin/env python3
"""Build native AWR payloads and stage installable npm tarballs and Python wheels."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
TARGETS = {
    "Darwin-arm64": ("darwin-arm64", "aarch64-apple-darwin", "macosx_15_0_arm64", "darwin", "arm64"),
    "Darwin-x86_64": ("darwin-x64", "x86_64-apple-darwin", "macosx_15_0_x86_64", "darwin", "x64"),
    "Linux-x86_64": ("linux-x64-gnu", "x86_64-unknown-linux-gnu", "linux_x86_64", "linux", "x64"),
    "Linux-aarch64": ("linux-arm64-gnu", "aarch64-unknown-linux-gnu", "linux_aarch64", "linux", "arm64"),
    "Windows-AMD64": ("win32-x64", "x86_64-pc-windows-msvc", "win_amd64", "win32", "x64"),
}
PLATFORMS = {target[0] for target in TARGETS.values()}


def run(args, *, cwd=ROOT, env=None):
    if env is None:
        env = dict(os.environ, AWR_LOCAL_WEB="0")
    return subprocess.check_output([str(a) for a in args], cwd=cwd, env=env, text=True).strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")


def python_version(version):
    match = re.fullmatch(r"(\d+\.\d+\.\d+)(?:-(dev|alpha|beta|rc)(?:\.(\d+))?)?", version)
    if not match:
        raise ValueError(f"unsupported release version: {version}")
    base, channel, number = match.groups()
    return base + ({"dev": ".dev", "alpha": "a", "beta": "b", "rc": "rc"}[channel] + (number or "0") if channel else "")


def license_bundle(target, package_names=("awr-cli", "awr-mcp"), features=()):
    """Collect runtime and build notices for the selected workspace packages."""
    command = ["cargo", "metadata", "--locked", "--format-version=1", "--filter-platform", target]
    if features:
        command.extend(["--features", ",".join(features)])
    metadata = json.loads(run(command))
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    workspace_members = set(metadata["workspace_members"])
    package_ids = {package["name"]: package["id"] for package in metadata["packages"]
                   if package["id"] in workspace_members}
    pending = [package_ids[name] for name in package_names]
    selected = set()
    while pending:
        key = pending.pop()
        if key in selected:
            continue
        selected.add(key)
        for dep in nodes[key]["deps"]:
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])
    overrides = {(x["crate"], x["version"]): x for x in json.loads((ROOT / "packaging/licenses/sources.json").read_text())}
    parts = ["AWR dependency license notices\nIncludes the selected platform's runtime and build dependencies.\n"]
    inventory = []
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        # Path dependencies can be vendored third-party code. Only the workspace's
        # own packages are covered by the root license instead of this bundle.
        if package["id"] not in selected or package["id"] in workspace_members:
            continue
        base = Path(package["manifest_path"]).parent
        files = sorted(f for f in base.iterdir() if f.is_file() and any(token in f.name.upper() for token in ("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE")))
        if package.get("license_file"):
            files = sorted(set(files + [base / package["license_file"]]))
        override = overrides.get((package["name"], package["version"]))
        if not files and override:
            f = ROOT / "packaging/licenses" / override["file"]
            if digest(f) != override["sha256"]:
                raise ValueError(f"license hash mismatch: {package['name']}")
            files = [f]
        if not files:
            raise ValueError(f"license text missing: {package['name']} {package['version']}")
        header = f"{package['name']} {package['version']} | {package['license']} | {package.get('repository') or package['source']}"
        parts.append("\n" + "=" * 72 + "\n" + header + "\n")
        for file in files:
            parts.append("\n" + file.name + "\n" + file.read_text(encoding="utf-8"))
        inventory.append({"name": package["name"], "version": package["version"], "license": package["license"],
                          "notices": [{"file": f.name, "sha256": digest(f)} for f in files]})
    return "\n".join(parts), inventory


def copy_contract_docs(primary):
    # Ship a curated subset of the CLI contract docs so local `npm i -g` users
    # can discover JSON output shapes without reading the GitHub repo. The npm
    # `files` allowlist (packaging/npm/package.json) must list "docs" for these
    # to be published.
    contract_docs = [
        "docs/reference/cli-mcp-contract.md",
        "docs/integrations/session-workflow.md",
        "docs/reference/daily-work.md",
        "docs/reference/local-project-hub.md",
    ]
    for rel in contract_docs:
        # Repository docs moved; keep installed npm documentation paths stable.
        source = ROOT / "docs/dev" / Path(rel).relative_to("docs")
        dest = primary / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, dest)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True, help="new directory; existing outputs are never replaced")
    args = parser.parse_args()
    target_info = TARGETS.get(f"{platform.system()}-{platform.machine()}")
    if not target_info:
        raise ValueError("native distribution build is supported only on macOS arm64/x64, Linux arm64/x64 and Windows x64")
    target, rust_target, wheel_platform, npm_os, npm_cpu = target_info
    source_sha = run(["git", "rev-parse", "HEAD"])
    if run(["git", "status", "--porcelain"]):
        raise ValueError("commit the reviewed release inputs before building")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    py_version = python_version(version)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    publish = output / "publish"
    publish.mkdir()
    stage = output / "staging"
    stage.mkdir()
    env = os.environ.copy()
    if platform.system() == "Darwin":
        env["MACOSX_DEPLOYMENT_TARGET"] = "15.0"
    run(["cargo", "build", "--locked", "--release", "-p", "awr-cli", "-p", "awr-mcp",
         "--target", rust_target, "--target-dir", ROOT / ".local/dist-build"], env=env)
    bin_dir = ROOT / ".local/dist-build" / rust_target / "release"
    suffix = ".exe" if npm_os == "win32" else ""
    binaries = [bin_dir / (name + suffix) for name in ("awr", "awr-mcp")]
    for binary in binaries:
        if version not in run([binary, "--version"]):
            raise ValueError(f"binary version mismatch: {binary.name}")
    run([binaries[0], "--json", "capabilities", "--require", "local.project_hub"])
    architectures = {}
    if npm_os == "darwin":
        for binary in binaries:
            architectures[binary.name] = run(["lipo", "-archs", binary])
            if architectures[binary.name] != platform.machine():
                raise ValueError(f"Mach-O architecture mismatch: {binary.name}")
    notices, inventory = license_bundle(rust_target)
    if run(["git", "rev-parse", "HEAD"]) != source_sha or run(["git", "status", "--porcelain"]):
        raise ValueError("source tree changed during the native build")
    metadata = {
        "version": version, "python_version": py_version, "platform": target,
        "wheel_platform": wheel_platform, "rust_target": rust_target,
        "source_sha": source_sha,
        "source_tree_clean": not bool(run(["git", "status", "--porcelain"])),
        "rustc": run(["rustc", "--version"]),
        "binary_sha256": {p.name: digest(p) for p in binaries},
        "binary_architectures": architectures,
        "dependency_notices": inventory,
        "local_project_hub": True,
    }
    primary = stage / "npm"
    shutil.copytree(ROOT / "packaging/npm", primary)
    copy_contract_docs(primary)
    shutil.copyfile(ROOT / "LICENSE", primary / "LICENSE")
    package = json.loads((primary / "package.json").read_text())
    package["version"] = version
    package["publishConfig"]["tag"] = "next" if "-" in version else "latest"
    package["optionalDependencies"] = {name: version for name in package["optionalDependencies"]}
    write_json(primary / "package.json", package)
    # The platform-independent npm wrapper must be identical on Windows and Unix.
    for file in primary.rglob("*"):
        if file.is_file():
            file.write_text(file.read_text(encoding="utf-8"), encoding="utf-8", newline="\n")
    native = stage / "native"
    (native / "bin").mkdir(parents=True)
    for binary in binaries:
        shutil.copy2(binary, native / "bin" / binary.name)
    native_package = {
        "name": f"@originoneai/awr-{target}", "version": version,
        "description": f"Native AWR binaries for {target}", "license": "Apache-2.0",
        "repository": package["repository"], "os": [npm_os], "cpu": [npm_cpu],
        "files": ["bin", "build.json", "LICENSE", "THIRD_PARTY_LICENSES.txt"],
        "publishConfig": {"access": "public", "tag": "next" if "-" in version else "latest"},
    }
    if npm_os == "linux":
        native_package["libc"] = ["glibc"]
    write_json(native / "package.json", native_package)
    write_json(native / "build.json", metadata)
    shutil.copyfile(ROOT / "LICENSE", native / "LICENSE")
    (native / "THIRD_PARTY_LICENSES.txt").write_text(notices, encoding="utf-8")
    for source in (primary, native):
        run(["npm.cmd" if os.name == "nt" else "npm", "pack", "--ignore-scripts", "--json", "--pack-destination", publish], cwd=source)
    python = stage / "python"
    shutil.copytree(ROOT / "packaging/python", python)
    (python / "awr_binary/bin").mkdir()
    for binary in binaries:
        shutil.copy2(binary, python / "awr_binary/bin" / binary.name)
    write_json(python / "awr_binary/_build.json", metadata)
    shutil.copyfile(ROOT / "LICENSE", python / "LICENSE")
    (python / "THIRD_PARTY_LICENSES.txt").write_text(notices, encoding="utf-8")
    wheels = output / "wheels-unrepaired" if npm_os == "linux" else publish
    run([sys.executable, "-m", "build", "--wheel", "--no-isolation", "--outdir", wheels, python])
    if npm_os == "linux":
        # PyPI requires a manylinux-compatible wheel, not an unverified linux_* tag.
        for wheel in wheels.glob("*.whl"):
            run([sys.executable, "-m", "auditwheel", "repair", "--plat", f"manylinux_2_39_{wheel_platform.removeprefix('linux_')}", "-w", publish, wheel])
    run([sys.executable, "-m", "twine", "check", "--strict", *publish.glob("*.whl")])
    metadata["artifacts"] = {p.name: digest(p) for p in sorted(publish.iterdir())}
    write_json(output / "manifest.json", metadata)
    (output / "SHA256SUMS").write_text("".join(f"{sha}  {name}\n" for name, sha in metadata["artifacts"].items()))
    print(json.dumps({"output": str(output), "version": version, "platform": target, "artifacts": list(metadata["artifacts"])}))


if __name__ == "__main__":
    main()
