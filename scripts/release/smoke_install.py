#!/usr/bin/env python3
"""Install each real package in a fresh directory and exercise its public commands."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import signal
import shutil
import subprocess
import sys
import tempfile
import threading
import time
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


def check_mcp(command, project, *, env=None, expect_intake=False):
    process = subprocess.Popen([str(command), "--project", str(project)], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               env=env if env is not None else dict(os.environ, AWR_LOCAL_WEB="0"))
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
        if expect_intake:
            request = {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                "name": "awr_project_status", "arguments": {}}}
            process.stdin.write(json.dumps(request) + "\n")
            process.stdin.flush()
            reply = responses.get(timeout=20)
            assert reply["id"] == 3 and reply["result"]["isError"] is True, reply
            intake = reply["result"]["structuredContent"]["takeover"]
            assert intake["project"]["state"] == "discovered", intake
            assert intake["guidance"]["basis"]["goal_inferred"] is False, intake
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


def check_local_hub(awr, mcp, project, state_dir):
    env = dict(os.environ, AWR_LOCAL_HOME=str(state_dir), AWR_LOCAL_WEB="1")
    mcp_env = dict(env, AWR_LOCAL_HOME=str(state_dir.parent / "mcp-hub-user"))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def catalog():
        return json.loads(run([awr, "overview", "projects", "--json"], env=env).stdout)

    def refresh():
        request = urllib.request.Request(first["url"] + "/api/projects/refresh",
                                         headers={"X-AWR-Project-Hub": "1"}, method="POST")
        with opener.open(request, timeout=10) as response:
            return json.load(response)["data"]

    try:
        run([awr, "--project", project, "status", "--cached", "--json"], env=env)
        first = json.loads(run([awr, "overview", "status", "--json"], env=env).stdout)["service"]
        assert first and first["service"] == "awr-project-hub"
        entries = json.loads(run([awr, "overview", "projects", "--json"], env=env).stdout)["projects"]
        assert len(entries) == 1 and os.path.samefile(entries[0]["root"], project)
        for route, expected in [("/", "AWR Project Hub"), ("/hub.js", "createHub"),
                                ("/?project=" + entries[0]["key"], 'id="projectHome"')]:
            with opener.open(first["url"] + route, timeout=10) as response:
                assert response.status == 200 and expected in response.read().decode("utf-8")
        run([awr, "--version"], env=env)
        second = json.loads(run([awr, "overview", "status", "--json"], env=env).stdout)["service"]
        assert second["instance"] == first["instance"]

        repository = project.parent / "Pending repository"
        (repository / ".git").mkdir(parents=True)
        run([awr, "overview", "discover", "--root", project.parent, "--interval-seconds", "1", "--json"], env=env)
        assert len(catalog()["projects"]) == 1
        request = urllib.request.Request(first["url"] + "/api/projects/intake",
                                         data=json.dumps({"root": str(repository.resolve())}).encode(),
                                         headers={"Content-Type": "application/json", "X-AWR-Project-Hub": "1"}, method="POST")
        with opener.open(request, timeout=10) as response:
            enrolled = json.load(response)["data"]["project"]
        assert enrolled["intake_requested"] is True
        pending, = [entry for entry in catalog()["projects"] if os.path.samefile(entry["root"], repository)]
        assert pending["state"] == "discovered" and pending["project_id"] is None, pending
        assert not (repository / ".awr").exists()
        intake = json.loads(run([awr, "--project", repository, "overview", "inspect", "--json"], env=env).stdout)
        assert intake["guidance"]["next_action"]["argv"][3] == "init", intake
        assert intake["guidance"]["basis"]["goal_inferred"] is False, intake
        with opener.open(first["url"] + "/api/projects/" + pending["key"] + "/intake", timeout=10) as response:
            assert json.load(response)["data"]["project"]["key"] == pending["key"]

        check_mcp(mcp, repository, env=mcp_env, expect_intake=True)
        mcp_status = json.loads(run([awr, "overview", "status", "--json"], env=mcp_env).stdout)["service"]
        assert mcp_status and mcp_status["service"] == "awr-project-hub", mcp_status
        mcp_entries = json.loads(run([awr, "overview", "projects", "--json"], env=mcp_env).stdout)["projects"]
        assert not mcp_entries, mcp_entries
        assert not (repository / ".awr").exists()

        goal = "Deliver a documented report viewer with verified source dates"
        run([awr, "--project", repository, "init", "--goal", goal, "--json"], env=env)
        assert not (repository / ".awr").exists()
        run([awr, "--project", repository, "init", "--goal", goal, "--accept", "--json"], env=env)
        connected, = [entry for entry in catalog()["projects"] if entry["key"] == pending["key"]]
        assert connected["state"] == "connected" and connected["project_id"], connected

        added = project.parent / "New managed repository"
        (added / ".git").mkdir(parents=True)
        run([awr, "--project", added, "init", "--goal", goal, "--accept", "--json"], env=dict(env, AWR_LOCAL_WEB="0"))
        last_scan = catalog()["discovery"]["completed_at"]
        time.sleep(1.1)
        idle = catalog()
        assert idle["discovery"]["completed_at"] == last_scan, idle
        assert all(not os.path.samefile(entry["root"], added) for entry in idle["projects"]), idle
        observed = refresh()
        added_entry, = [entry for entry in observed["projects"] if os.path.samefile(entry["root"], added)]
        assert added_entry["state"] == "connected", added_entry
        shutil.rmtree(added)
        assert any(entry["key"] == added_entry["key"] and not entry["available"] for entry in refresh()["projects"])

        request = urllib.request.Request(first["url"] + "/api/projects/" + pending["key"],
                                         headers={"X-AWR-Project-Hub": "1"}, method="DELETE")
        with opener.open(request, timeout=10) as response:
            assert json.load(response)["ok"] is True
        run([awr, "--project", repository, "status", "--cached", "--json"], env=env)
        assert all(entry["key"] != pending["key"] for entry in refresh()["projects"])
        assert (repository / ".awr/state.db").is_file()

        os.kill(first["pid"], signal.SIGTERM)
        deadline = time.monotonic() + 10
        while json.loads(run([awr, "overview", "status", "--json"], env=env).stdout)["service"]:
            assert time.monotonic() < deadline, "Owned test service did not terminate"
            time.sleep(0.05)
        run([awr, "--version"], env=env)
        recovered = json.loads(run([awr, "overview", "status", "--json"], env=env).stdout)["service"]
        assert recovered["instance"] != first["instance"] and recovered["url"] == first["url"], recovered
        with opener.open(recovered["url"] + "/api/projects", timeout=10) as response:
            assert all(entry["key"] != pending["key"] for entry in json.load(response)["data"]["projects"])
        request = urllib.request.Request(recovered["url"] + "/api/projects/intake",
                                         data=json.dumps({"root": str(repository.resolve())}).encode(),
                                         headers={"Content-Type": "application/json", "X-AWR-Project-Hub": "1"}, method="POST")
        with opener.open(request, timeout=10) as response:
            assert json.load(response)["data"]["project"]["key"] == pending["key"]
        assert any(entry["key"] == pending["key"] for entry in catalog()["projects"])
        run([awr, "--version"], env=env)
        reused = json.loads(run([awr, "overview", "status", "--json"], env=env).stdout)["service"]
        assert reused["instance"] == recovered["instance"], reused
        return True
    finally:
        for environment in (env, mcp_env):
            run([awr, "overview", "stop", "--json"], env=environment)


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
    local_hub = check_local_hub(awr, mcp, project, scratch / "hub-user")
    return {"version_and_help": True, "exit_code_and_stderr": True,
            "unicode_space_project_init_and_status": True, "task_context_and_intake": True,
            "mcp_stdio_tools": names, "local_project_hub": local_hub,
            "local_project_discovery_and_intake": local_hub}


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
        installed_version = run([python, "-c", "from importlib.metadata import version; print(version('originoneai-awr'))"]).stdout.strip()
        assert installed_version == manifest["python_version"], "canonical Python distribution identity mismatch"
        suffix = ".exe" if os.name == "nt" else ""
        py_result = exercise(python_bin / ("awr" + suffix), python_bin / ("awr-mcp" + suffix),
                             scratch / "python-use", manifest["version"])
        npm_root = scratch / "npm"
        npm_root.mkdir()
        # Only the two tarballs under test are available; no registry fallback can hide omissions.
        run(["npm.cmd" if os.name == "nt" else "npm", "install", "--prefix", npm_root,
             "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--package-lock=false", *tarballs])
        installed = json.loads((npm_root / "node_modules/@originoneai/awr/package.json").read_text())
        assert installed["name"] == "@originoneai/awr" and installed["version"] == manifest["version"]
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
