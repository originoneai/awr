#!/usr/bin/env python3
"""Run the focused evidence-currency component, transport and CLI POC gate.

The runner intentionally accepts dirty worktrees. It records the worktree state
and input hashes so a receipt cannot be mistaken for a clean committed result.
"""

import argparse
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "tests" / "platform"))
from process_runner import run_command

CONTRACT = HERE / "contract.json"
PREFIX = ["rtk", "proxy"] if shutil.which("rtk") else []
COMMAND_TIMEOUT = 1800
CLEANUP_TIMEOUT = 10


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def snapshot_binary(source, destination):
    before = digest(source)
    shutil.copy2(source, destination)
    require(digest(destination) == before and digest(source) == before,
            "Built binary changed while collecting its immutable test snapshot")
    return before


def text(value):
    if isinstance(value, bytes):
        return value.decode("utf-8", "replace")
    return value or ""


def now():
    return datetime.now().astimezone().isoformat(timespec="seconds")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def command(argv):
    return PREFIX + [str(item) for item in argv]


def run_capture(argv, timeout=COMMAND_TIMEOUT):
    return subprocess.run(command(argv), cwd=ROOT, capture_output=True, timeout=timeout,
                          env=dict(os.environ, PYTHONUTF8="1", CARGO_TERM_COLOR="never"))


def git_output(*argv):
    result = run_capture(["git", *argv], timeout=120)
    require(result.returncode == 0, f"git {' '.join(argv)} failed: {text(result.stderr)}")
    return result.stdout.decode("utf-8", "replace")


def status_snapshot():
    status = git_output("status", "--porcelain=v1", "--untracked-files=all")
    names = git_output("ls-files", "-z", "--cached", "--others", "--exclude-standard").split("\0")
    hashes = {}
    for name in sorted(set(item for item in names if item)):
        path = ROOT / name
        if path.is_file():
            hashes[name] = digest(path)
    return status, hashes


def tree_hash(values):
    return hashlib.sha256(json.dumps(values, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def parse_rust_release(output):
    fields = dict(line.split(": ", 1) for line in output.splitlines() if ": " in line)
    return fields.get("release"), fields


def rust_test_totals(body):
    return [tuple(map(int, match)) for match in re.findall(
        r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored", body)]


def compile_gate_passed(rows):
    build = rows.get("build", {})
    return build.get("status") == "passed" and build.get("built", True)


def native_gate_passed(mode, statuses, names):
    return mode == "native_test" and all(statuses.get(name) == "passed" for name in names)


def validate_poc_report(data, contract, expected_scenarios, awr):
    for field in contract["poc"]["required_fields"]:
        require(field in data, "POC report missing " + field)
    require(data["version"] == 1 and data["passed"] is True,
            "POC did not pass its synthetic CLI contract")
    require(data["verification_kind"] == "synthetic_cli_process_poc", "Unexpected POC kind")
    require(data["inputs_unchanged"] is True and data["binary_unchanged"] is True,
            "POC input or binary changed during execution")
    scenarios = data["scenarios"]
    observed = [item.get("id") for item in scenarios]
    require(len(observed) == len(set(observed)) and set(observed) == set(expected_scenarios),
            "POC scenario inventory does not match the contract")
    require(all(item.get("passed") is True for item in scenarios), "POC scenario failed")
    observations = {item['id']: item.get('details', {}) for item in scenarios}
    for scenario in contract['scenarios']:
        for field, expected in scenario.get('required_observations', {}).items():
            actual = observations[scenario['id']].get(field)
            require(type(actual) is type(expected) and actual == expected,
                    f"POC {scenario['id']} missing or incorrect observation: {field}")
    require(isinstance(data["input_hashes"], dict) and data["input_hashes"],
            "POC input hashes are empty")
    require(isinstance(data["binary_sha256"], str) and len(data["binary_sha256"]) == 64,
            "POC binary hash is missing")
    require(data["binary_sha256"] == digest(awr), "POC binary hash does not bind awr")
    identity = data["binary_identity"]
    require(isinstance(identity, dict)
            and identity.get("format") in {"Mach-O", "ELF", "PE"}
            and identity.get("architecture") not in {None, "unknown"}
            and identity.get("architecture_relation") == "same",
            "POC binary architecture is not a native host match")
    require(isinstance(data["platform"], dict) and data["platform"].get("system")
            and data["platform"].get("machine"), "POC platform binding is incomplete")
    return {"scenarios": observed, "steps": len(data["steps"]),
            "source_dirty": data["source_dirty"]}


def output_path(value):
    path = Path(value).expanduser().resolve()
    require(not path.exists(), f"Output already exists; use a unique ignored directory: {path}")
    try:
        relative = path.relative_to(ROOT)
    except ValueError:
        return path
    require(relative.parts and relative.parts[0] == ".local",
            "Output inside the repository must be below .local/")
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True,
                        help="New ignored directory below .local (or a path outside the repository)")
    parser.add_argument("--target", help="Cross target for compile-only evidence; never executes its binaries")
    args = parser.parse_args()
    output = output_path(args.output)

    contract = json.loads(CONTRACT.read_text(encoding="utf-8"))
    required_gates = contract["required_gates"]
    require(len(required_gates) == contract["target_gates"] == len(set(required_gates)),
            "Invalid evidence-currency gate inventory")
    scenario_specs = contract["scenarios"]
    expected_scenarios = [item["id"] for item in scenario_specs]
    require(len(expected_scenarios) == len(set(expected_scenarios)), "Duplicate POC scenario")
    output.mkdir(parents=True, exist_ok=False)
    report_path = output / "report.json"

    before_status, before_worktree = status_snapshot()
    input_hashes = {}
    missing_inputs = []
    for name in contract["input_paths"]:
        path = ROOT / name
        if path.is_file():
            input_hashes[name] = digest(path)
        else:
            missing_inputs.append(name)
    require(not missing_inputs, "Contract input is missing: " + ", ".join(missing_inputs))
    source_commit = git_output("rev-parse", "HEAD").strip()
    rustc_output = ""
    report = {
        "contract_id": contract["contract_id"],
        "contract_version": contract["version"],
        "contract_sha256": digest(CONTRACT),
        "work_item": contract["work_item"],
        "started_at": now(),
        "source_commit": source_commit,
        "source_dirty": bool(before_status),
        "source_status": before_status,
        "source_status_sha256": hashlib.sha256(before_status.encode()).hexdigest(),
        "input_sha256": input_hashes,
        "input_tree_sha256": tree_hash(input_hashes),
        "worktree_input_sha256": before_worktree,
        "worktree_input_tree_sha256": tree_hash(before_worktree),
        "mode": "compile_only" if args.target else "native_test",
        "target": args.target or platform.machine(),
        "binary_source": "cargo_build",
        "evidence_level": "cross_target_compilation" if args.target else contract["evidence_level"],
        "gates": [],
        "passed": False,
        "binary_sha256": {},
        "limitations": contract["limitations"],
    }

    def save():
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    def append_gate(row):
        report["gates"].append(row)
        save()

    def skipped(name, status, reason, execution_kind="native_test"):
        append_gate({"id": name, "status": status, "passed": False,
                     "not_run": status == "not_run", "unsupported": status == "unsupported",
                     "execution_kind": execution_kind, "reason": reason,
                     "command": [], "started_at": now(), "finished_at": now()})

    process_boundary_ok = True

    def gate(name, argv, validator=None, requires=(), execution_kind="native_test", env=None):
        nonlocal process_boundary_ok
        if not process_boundary_ok:
            skipped(name, "not_run", "A preceding process did not finish cleanup or log drainage", execution_kind)
            return False
        unmet = [dependency for dependency in requires
                 if not any(row["id"] == dependency and row["status"] == "passed"
                            for row in report["gates"])]
        if unmet:
            skipped(name, "not_run", "Unmet dependencies: " + ", ".join(unmet), execution_kind)
            return False
        row = {"id": name, "status": "failed", "passed": False, "not_run": False,
               "unsupported": False, "execution_kind": execution_kind,
               "command": command(argv), "started_at": now()}
        log = output / (name + ".log")
        started = time.monotonic()
        process_env = dict(os.environ, PYTHONUTF8="1", CARGO_TERM_COLOR="never")
        process_env.update({key: str(value) for key, value in (env or {}).items()})
        try:
            result = run_command(command(argv), cwd=ROOT, log=log, env=process_env,
                                 timeout_seconds=COMMAND_TIMEOUT, cleanup_seconds=CLEANUP_TIMEOUT)
            if not result.get("cleanup_complete") or not result.get("log_finalized"):
                process_boundary_ok = False
            body = log.read_text(encoding="utf-8", errors="replace") if log.is_file() else ""
            row.update({key: value for key, value in result.items() if key != "passed"},
                       elapsed_seconds=round(time.monotonic() - started, 3),
                       log=str(log.relative_to(output)))
            if result.get("log_finalized") and log.is_file():
                row["log_sha256"] = digest(log)
            if result.get("passed"):
                try:
                    extra = validator(body) if validator else {}
                except Exception as error:
                    row.update(status="failed", passed=False, error=str(error))
                else:
                    row.update(status="passed", passed=True, **extra)
            else:
                row["status"] = "failed"
                row["passed"] = False
                if execution_kind == "compile_only" and any(marker in body.lower() for marker in (
                        "target may not be installed", "target ... is not installed",
                        "linker not found", "could not find linker", "can't find crate")):
                    row.update(status="unsupported", unsupported=True,
                               reason="Cross-target toolchain or linker is unavailable")
                row.setdefault("error", f"command exited {result.get('exit_code')}")
        except Exception as error:
            row.update(error=str(error), elapsed_seconds=round(time.monotonic() - started, 3),
                       log=str(log.relative_to(output)))
            if log.is_file():
                row["log_sha256"] = digest(log)
        row["finished_at"] = now()
        append_gate(row)
        return row["passed"]

    def test_validator(body):
        rows = rust_test_totals(body)
        require(rows and sum(row[0] for row in rows) > 0, "No executed Rust tests")
        require(sum(row[1] for row in rows) == 0, "Rust test failure")
        return {"test_functions_passed": sum(row[0] for row in rows),
                "test_functions_failed": sum(row[1] for row in rows),
                "test_functions_ignored": sum(row[2] for row in rows),
                "test_targets_reported": len(rows)}

    def environment_validator(body):
        nonlocal rustc_output
        rustc_output = body
        release, fields = parse_rust_release(body)
        require(release == contract["rust_version"],
                f"Rust toolchain {release!r} does not match pinned {contract['rust_version']}")
        return {"rust_release": release, "rust_host": fields.get("host")}

    save()
    gate("environment", ["rustc", "-vV"], environment_validator)
    if not any(row["id"] == "environment" and row["status"] == "passed" for row in report["gates"]):
        for name in required_gates:
            if name != "environment":
                skipped(name, "not_run", "Pinned environment check failed")
        report.update(finished_at=now(), inputs_unchanged=False, binary_unchanged=False,
                      rustc=rustc_output, compile_passed=False, native_tests_passed=False,
                      compile_verification_passed=False, native_passed=False)
        save()
        return 1

    gate("runner_contract", [sys.executable, "-m", "unittest", "discover",
                             "-s", "tests/evidence-currency", "-p", "test_*.py"],
         requires=("environment",))
    metadata = run_capture(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], timeout=300)
    target_directory = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).expanduser()
    if not target_directory.is_absolute():
        target_directory = (ROOT / target_directory).resolve()
    if metadata.returncode != 0:
        report["workspace_metadata_error"] = text(metadata.stderr)
    else:
        data = json.loads(text(metadata.stdout))
        target_directory = Path(data["target_directory"])
        if not target_directory.is_absolute():
            target_directory = (ROOT / target_directory).resolve()
        report["workspace_members"] = sorted(
            package["name"] for package in data["packages"] if package["id"] in data["workspace_members"])
    report["target_directory"] = str(target_directory)
    build_args = ["cargo", "build", "-p", "awr-cli", "-p", "awr-mcp", "--locked"]
    if args.target:
        build_args = ["cargo", "build", "--workspace", "--all-targets", "--locked", "--target", args.target]
    gate("build", build_args, requires=("runner_contract",),
         execution_kind="compile_only" if args.target else "native_test")
    suffix = ".exe" if (args.target and "windows" in args.target) or os.name == "nt" else ""
    binary_root = target_directory / args.target / "debug" if args.target else target_directory / "debug"
    awr, mcp = binary_root / ("awr" + suffix), binary_root / ("awr-mcp" + suffix)

    if any(row['id'] == 'build' and row['status'] == 'passed' for row in report['gates']):
        snapshots = output / 'binaries'
        snapshots.mkdir()
        try:
            captured = {}
            for name, path in [('awr', awr), ('awr-mcp', mcp)]:
                destination = snapshots / (name + ('.exe' if path.suffix == '.exe' else ''))
                sha = snapshot_binary(path, destination)
                captured[name] = dict(build_path=str(path), snapshot=str(destination), sha256=sha)
            report['binary_provenance'] = captured
            awr, mcp = Path(captured['awr']['snapshot']), Path(captured['awr-mcp']['snapshot'])
        except Exception as error:
            build = next(row for row in report['gates'] if row['id'] == 'build')
            build.update(status='failed', passed=False, error=str(error))
            save()

    if awr.is_file():
        report["binary_sha256"]["awr"] = digest(awr)
    if mcp.is_file():
        report["binary_sha256"]["awr-mcp"] = digest(mcp)

    if args.target:
        for name in ["crash_recovery", "core", "store", "store_evidence", "source", "source_fingerprint", "runtime", "context", "cli", "mcp", "poc"]:
            skipped(name, "not_run", "Foreign target is compile_only; no target executable is run")
    else:
        gate("crash_recovery", ["cargo", "test", "-p", "awr-runtime", "--lib", "--locked",
                                "mutation_apply::recovery_tests::cli_recovery_after_process_death_binds_original_attempt_and_resolves_once",
                                "--", "--ignored", "--exact", "--test-threads=1"], test_validator,
             requires=("build",), env={"AWR_RECOVERY_CLI": str(awr.resolve())})
        for name in ["core", "store", "store_evidence", "source", "source_fingerprint", "runtime", "context"]:
            gate(name, contract["test_targets"][name], test_validator, requires=("build",))
        gate("cli", contract["test_targets"]["cli"], test_validator, requires=("build",))
        gate("mcp", contract["test_targets"]["mcp"], test_validator, requires=("build",))

        poc_report = output / "poc" / contract["poc"]["report_name"]

        def validate_poc(_):
            require(poc_report.is_file(), "POC did not produce report.json")
            data = json.loads(poc_report.read_text(encoding="utf-8"))
            extra = validate_poc_report(data, contract, expected_scenarios, awr)
            require(data["source_commit"] == source_commit and data["source_dirty"] == report["source_dirty"],
                    "POC source identity differs from the focused runner")
            require(data["input_hashes"] == before_worktree,
                    "POC input hashes differ from the focused worktree snapshot")
            return {"receipt": str(poc_report.relative_to(output)), "receipt_sha256": digest(poc_report),
                    **extra}

        gate("poc", [sys.executable, str(HERE / "poc.py"), "--awr", str(awr),
                      "--output", str(output / "poc")], validate_poc, requires=("build",))

    gate("format", ["cargo", "fmt", "--all", "--check"])
    gate("diff", ["git", "diff", "--check"])
    gate("public_tree", [sys.executable, "scripts/check_public_tree.py"])

    after_status, after_worktree = status_snapshot()
    after_inputs = {}
    for name in contract["input_paths"]:
        path = ROOT / name
        if path.is_file():
            after_inputs[name] = digest(path)
    inputs_unchanged = before_status == after_status and input_hashes == after_inputs and before_worktree == after_worktree
    binary_paths = {"awr": awr, "awr-mcp": mcp}
    binaries_unchanged = set(report["binary_sha256"]) == set(binary_paths) and all(
        path.is_file() and digest(path) == report["binary_sha256"][name]
        for name, path in binary_paths.items())
    statuses = {row["id"]: row["status"] for row in report["gates"]}
    rows_by_id = {row["id"]: row for row in report["gates"]}
    compile_passed = compile_gate_passed(rows_by_id)
    native_gate_names = ["crash_recovery", "core", "store", "store_evidence", "source", "source_fingerprint",
                         "runtime", "context", "cli", "mcp", "poc"]
    native_tests_passed = (native_gate_passed(report["mode"], statuses, native_gate_names)
                           and binaries_unchanged)
    all_required_passed = all(statuses.get(name) == "passed" for name in required_gates)
    compile_checks = ["runner_contract", "format", "diff", "public_tree"]
    compile_verification_passed = (args.target and compile_passed and binaries_unchanged
                                   and inputs_unchanged
                                   and all(statuses.get(name) == "passed" for name in compile_checks))
    report.update(
        rustc=rustc_output,
        finished_at=now(),
        after_source_status=after_status,
        after_source_status_sha256=hashlib.sha256(after_status.encode()).hexdigest(),
        inputs_unchanged=inputs_unchanged,
        binary_unchanged=binaries_unchanged,
        compile_passed=compile_passed,
        compile_verification_passed=bool(compile_verification_passed),
        native_tests_passed=native_tests_passed,
        native_passed=report["mode"] == "native_test" and all_required_passed and inputs_unchanged and binaries_unchanged,
        passed=not args.target and all_required_passed and inputs_unchanged and binaries_unchanged,
        gates_passed=sum(status == "passed" for status in statuses.values()),
        target_gates=len(required_gates),
        unsupported_gates=[name for name, status in statuses.items() if status == "unsupported"],
        not_run_gates=[name for name, status in statuses.items() if status == "not_run"],
    )
    save()
    print(json.dumps({key: report[key] for key in ["passed", "mode", "source_dirty", "inputs_unchanged",
                                                    "binary_unchanged", "compile_passed",
                                                    "compile_verification_passed",
                                                    "native_tests_passed", "native_passed",
                                                    "gates_passed", "target_gates"]}))
    return 0 if (args.target and compile_verification_passed) or report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
