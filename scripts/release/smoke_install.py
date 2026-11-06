#!/usr/bin/env python3
"""Install each real package in a fresh directory and exercise its public commands."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import urllib.request
from mcp_catalog import validate_stdio_tools

ROOT = Path(__file__).resolve().parents[2]


def run(args, code=0, env=None):
    if env is None:
        env = dict(os.environ, AWR_LOCAL_WEB="0")
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True, env=env, timeout=90)
    if result.returncode != code:
        raise AssertionError(f"{args}: expected {code}, got {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result


def check_mcp(command, project):
    process = subprocess.Popen([str(command), "--project", str(project)], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               env=dict(os.environ, AWR_LOCAL_WEB="0"))
    responses = queue.Queue()

    def receive():
        for line in process.stdout:
            responses.put(json.loads(line))

    reader = threading.Thread(target=receive, daemon=True)
    reader.start()
    try:
        requests = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "awr-installed-package-check", "version": "1"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        ]
        process.stdin.write(json.dumps(requests[0]) + "\n")
        process.stdin.flush()
        hello = responses.get(timeout=20)
        assert hello["id"] == 1 and "result" in hello, hello
        process.stdin.write("\n".join(json.dumps(r) for r in requests[1:]) + "\n")
        process.stdin.flush()
        tools = responses.get(timeout=20)
        assert tools["id"] == 2 and "result" in tools, tools
        names = validate_stdio_tools([t["name"] for t in tools["result"]["tools"]])
        process.stdin.close()
        assert process.wait(timeout=10) == 0
        return names
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        reader.join(timeout=3)
        process.stdout.close()
        process.stderr.close()


def check_local_hub(awr, project, state_dir):
    env = dict(os.environ, AWR_LOCAL_HOME=str(state_dir), AWR_LOCAL_WEB="1")
    try:
        run([awr, "--project", project, "status", "--cached", "--json"], env=env)
        first = json.loads(run([awr, "overview", "status", "--json"], env=env).stdout)["service"]
        assert first and first["service"] == "awr-project-hub"
        entries = json.loads(run([awr, "overview", "projects", "--json"], env=env).stdout)["projects"]
        assert len(entries) == 1 and os.path.samefile(entries[0]["root"], project)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        for route, expected in [("/", "AWR Project Hub"), ("/hub.js", "createHub"),
                                ("/?project=" + entries[0]["key"], 'id="projectHome"')]:
            with opener.open(first["url"] + route, timeout=10) as response:
                assert response.status == 200 and expected in response.read().decode("utf-8")
        run([awr, "--version"], env=env)
        second = json.loads(run([awr, "overview", "status", "--json"], env=env).stdout)["service"]
        assert second["instance"] == first["instance"]
        return True
    finally:
        run([awr, "overview", "stop", "--json"], env=env)


def exercise(awr, mcp, scratch, version):
    for command in (awr, mcp):
        result = run([command, "--version"])
        assert version in result.stdout and not result.stderr, result
        result = run([command, "--help"])
        assert "Usage:" in result.stdout and not result.stderr, result
    bad = run([awr, "--not-an-awr-option"], code=2)
    assert bad.stderr and not bad.stdout
    project = scratch / "安装验证 project"
    shutil.copytree(ROOT / "examples/basic", project)
    run([awr, "--project", project, "init", "--manifest", project / "project.toml", "--accept"])
    status = run([awr, "--project", project, "--json", "status"])
    assert isinstance(json.loads(status.stdout), dict) and not status.stderr
    context = json.loads(run([awr, "--project", project, "--json", "context", "compile",
                              "--work", "EXAMPLE-001", "--goal", "goal#demo"]).stdout)
    assert context["completeness"]["complete"] and "EXAMPLE-001" in context["work_context"]["rendered_context"]
    diagnosis = json.loads(run([awr, "--project", project, "--json", "intake", "inspect"]).stdout)
    organization = diagnosis["organization"]
    assert organization["state"] == "ready", organization
    assert organization["business_execution_ready"] and "EXAMPLE-001" in organization["executable_work"], organization
    names = check_mcp(mcp, project)
    local_hub = check_local_hub(awr, project, scratch / "hub-user")
    return {"version_and_help": True, "exit_code_and_stderr": True,
            "unicode_space_project_init_and_status": True, "task_context_and_intake": True,
            "mcp_stdio_tools": names, "local_project_hub": local_hub}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    directory = args.directory.resolve()
    manifest = json.loads((directory / "manifest.json").read_text())
    for name, sha in manifest["artifacts"].items():
        assert hashlib.sha256((directory / "publish" / name).read_bytes()).hexdigest() == sha, name
    wheel, = (directory / "publish").glob("*.whl")
    tarballs = sorted((directory / "publish").glob("*.tgz"))
    assert len(tarballs) == 2, tarballs
    with tempfile.TemporaryDirectory(prefix="awr distribution ") as temporary:
        scratch = Path(temporary)
        python_env = scratch / "python"
        run([sys.executable, "-m", "venv", python_env])
        python_bin = python_env / ("Scripts" if os.name == "nt" else "bin")
        python = python_bin / ("python.exe" if os.name == "nt" else "python")
        run([python, "-m", "pip", "install", "--no-index", "--no-deps", wheel])
        suffix = ".exe" if os.name == "nt" else ""
        py_result = exercise(python_bin / ("awr" + suffix), python_bin / ("awr-mcp" + suffix),
                             scratch / "python-use", manifest["version"])
        npm_root = scratch / "npm"
        npm_root.mkdir()
        # Only the two tarballs under test are available; no registry fallback can hide omissions.
        run(["npm.cmd" if os.name == "nt" else "npm", "install", "--prefix", npm_root,
             "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--package-lock=false", *tarballs])
        npm_bin = npm_root / "node_modules/.bin"
        suffix = ".cmd" if os.name == "nt" else ""
        npm_result = exercise(npm_bin / ("awr" + suffix), npm_bin / ("awr-mcp" + suffix),
                              scratch / "npm-use", manifest["version"])
    receipt = {"scope": "local distribution installation and CLI/MCP smoke only; not business acceptance or publication",
               "source_sha": manifest["source_sha"], "platform": manifest["platform"],
               "artifact_sha256": manifest["artifacts"], "python": py_result, "npm": npm_result}
    (directory / "installation-checks.json").write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"platform": manifest["platform"], "npm": "passed", "pip": "passed"}))


if __name__ == "__main__":
    main()
