#!/usr/bin/env python3
"""Compare built CLI and real MCP stdio on disposable, identical project state.

Run after `cargo build -p awr-cli -p awr-mcp --locked`:
python3 crates/awr-mcp/tests/cli_parity.py --awr target/debug/awr --mcp target/debug/awr-mcp
No third-party Python dependencies, model calls, or user project writes.
"""
import argparse
from contextlib import closing
from copy import deepcopy
import hashlib
import json
import queue
import shutil
import sqlite3
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

MANIFEST = """[project]
name = "Transport parity fixture"
external_key = "transport-parity"
[[sources]]
domain = "ledger"
role = "primary"
path = "work.yaml"
adapter = "yaml-ledger-v1"
[[sources]]
domain = "rules"
role = "primary"
path = "rules.md"
adapter = "markdown-rules-v1"
[[sources]]
domain = "goal"
role = "primary"
path = "goal.md"
adapter = "markdown-heading-v1"
[sources.options]
status = "active"
"""
WORK = """work_items:
- id: W
  title: Prepare customer analysis
  status: ready
  owner: coordinator
  next_action: Draft the analysis
  depends_on: [D]
  acceptance: [Deliver the reviewed analysis]
  evidence: []
  verification:
    evidence_level: none
- id: D
  title: Required input
  status: completed
- id: NEXT
  title: Prepare follow-up
  status: ready
  next_action: Review next steps
  acceptance: [Follow-up is available]
"""
TRANSPORT_FIELDS = ("freshness_basis", "source_refresh_performed", "read_only", "snapshot", "continuity")
TOOLS = set()


class Client:
    def __init__(self, binary, root):
        self.log = tempfile.TemporaryFile()
        self.process = subprocess.Popen([str(binary), "--project", str(root)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.log, text=True, bufsize=1)
        self.queue = queue.Queue()
        self.counter = 0

        def read():
            for line in self.process.stdout:
                self.queue.put(line)
            self.queue.put(None)

        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        self.rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                "clientInfo": {"name": "awr-cli-parity", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, value):
        self.process.stdin.write(json.dumps(value) + "\n")
        self.process.stdin.flush()

    def rpc(self, method, params):
        self.counter += 1
        self.send({"jsonrpc": "2.0", "id": self.counter, "method": method, "params": params})
        while True:
            line = self.queue.get(timeout=30)
            if line is None:
                raise AssertionError("MCP stream ended")
            response = json.loads(line)
            if response.get("id") == self.counter:
                return response

    def close(self):
        self.process.stdin.close()
        try:
            code = self.process.wait(timeout=10)
            assert code == 0, code
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait(timeout=10)
            self.reader.join(timeout=5)
            self.process.stdout.close()
            self.log.close()


class Parity(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="awr-cli-parity-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        (self.root / "work.yaml").write_text(WORK)
        (self.root / "rules.md").write_text(
            "# Authority {#authority severity=hard scope=project value=*}\n\n"
            "Preserve exact acceptance and source facts.\n")
        (self.root / "goal.md").write_text("# Deliver useful analysis\n\nDeliver reviewed customer results.\n")
        (self.root / "project.toml").write_text(MANIFEST)
        self.cli(["init", "--manifest", str(self.root / "project.toml"), "--accept"])

    def cli(self, args, code=0):
        output = subprocess.run([str(OPTIONS.awr), "--project", str(self.root), "--json", *map(str, args)],
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(output.returncode, code, output.stdout + output.stderr)
        body = json.loads(output.stdout) if output.stdout else None
        error = json.loads(output.stderr) if output.stderr else None
        if code:
            self.assertIsInstance(error, dict)
            return body, error
        self.assertIsNone(error)
        return body

    def tool(self, name, args, error=False, readonly=False):
        before = self.state() if readonly else None
        client = Client(OPTIONS.mcp, self.root)
        try:
            response = client.rpc("tools/call", {"name": name, "arguments": args})
            self.assertNotIn("error", response)
            result = response["result"]
            body = result["structuredContent"]
            self.assertEqual(json.loads(result["content"][0]["text"]), body)
            self.assertEqual(bool(result.get("isError")), error, body)
            TOOLS.add(name)
        finally:
            client.close()
        if readonly:
            self.assertEqual(self.state(), before, "MCP read changed persisted state")
        return body

    def state(self):
        with closing(sqlite3.connect(f"file:{self.root / '.awr/state.db'}?mode=ro", uri=True)) as db:
            tables = [row[0] for row in db.execute("select name from sqlite_master where type='table' order by name")]
            rows = {name: sorted(repr(row) for row in db.execute('select * from "' + name.replace('"', '""') + '"'))
                    for name in tables}
        return rows, {name: (self.root / name).read_bytes()
                      for name in ["work.yaml", "rules.md", "goal.md", ".awr/project.toml"]}

    def snapshot(self):
        backup = self.root / "snapshot.db"
        with closing(sqlite3.connect(self.root / ".awr/state.db")) as source, closing(sqlite3.connect(backup)) as dest:
            source.backup(dest)
        return self.state()[1]

    def restore(self, sources):
        # Every connection/process is closed; these are only this test's temporary files.
        with closing(sqlite3.connect(self.root / "snapshot.db")) as source, closing(sqlite3.connect(self.root / ".awr/state.db")) as dest:
            source.backup(dest)
        for name, body in sources.items():
            (self.root / name).write_bytes(body)
        shutil.rmtree(self.root / ".awr/mutations", ignore_errors=True)

    def revision(self):
        return self.cli(["branch", "list"])["project_revision"]

    def session(self):
        return self.cli(["session", "start", "--work", "W", "--agent", "parity-executor",
                         "--provider", "fixture", "--model", "no-model-call", "--claim",
                         "--ttl-ms", "600000", "--expected-revision", self.revision()])["session"]["id"]

    def assert_projection_receipt(self, work):
        source = work["progress"]["source_next_action"]
        self.assertIsInstance(source["projected_at"], int)
        with closing(sqlite3.connect(f"file:{self.root / '.awr/state.db'}?mode=ro", uri=True)) as db:
            receipts = [(json.loads(payload), created_at) for payload, created_at in db.execute(
                "SELECT payload_json, created_at FROM events WHERE event_type='source.projected'")]
        matching = [created_at for payload, created_at in receipts
                    if payload["source_id"] == source["source_id"]
                    and payload["source_revision"] == source["source_revision"]]
        self.assertEqual(matching, [source["projected_at"]], "timestamp must identify the source's own projection receipt")

    def equal_read(self, cli, mcp, *, replayed_projection=False):
        self.assertEqual(cli["freshness_basis"], "source_refresh")
        self.assertTrue(cli["source_refresh_performed"])
        self.assertFalse(cli["read_only"])
        self.assertEqual(mcp["freshness_basis"], "source_verified_readonly")
        self.assertFalse(mcp["source_refresh_performed"])
        self.assertTrue(mcp["read_only"])
        self.assertTrue(mcp["snapshot"]["coherent"])
        if "snapshot" in cli:
            self.assertTrue(cli["snapshot"]["coherent"])
            for key in ("version", "storage", "project_revision", "source_state_fingerprint", "source_currentness_verified"):
                self.assertEqual(cli["snapshot"][key], mcp["snapshot"][key])
            self.assertIsNotNone(cli["snapshot"]["source_refresh_revision"])
        else:
            # CLI context packs retain their existing hash-bearing wire contract.
            self.assertIn("work_context", cli)
            self.assertEqual(cli["project_revision"], mcp["snapshot"]["project_revision"])
        self.assertIsNone(mcp["snapshot"]["source_refresh_revision"])
        if replayed_projection:
            # Restoring a snapshot and replaying a write creates a new receipt time.
            # Each caller has checked its own receipt before this comparison; all
            # other fields, and timestamps for same-state reads, remain exact.
            cli, mcp = deepcopy(cli), deepcopy(mcp)
            for value in (cli, mcp):
                value["progress"]["source_next_action"].pop("projected_at")
        self.assertEqual({k: v for k, v in cli.items() if k not in TRANSPORT_FIELDS},
                         {k: v for k, v in mcp.items() if k not in TRANSPORT_FIELDS})

    def test_query_and_claim_identity_parity(self):
        self.session()
        for cli, tool, args in [
            (["status"], "awr_project_status", {}),
            (["status", "--view", "full"], "awr_project_status", {"view": "full"}),
            (["status", "--work", "W"], "awr_project_status", {"view": "action", "work": ["W"]}),
            (["status", "--view", "summary"], "awr_project_status", {"view": "summary"}),
            (["status", "--view", "summary", "--work", "W"], "awr_project_status", {"view": "summary", "work": ["W"]}),
            (["ready", "--limit", "1"], "awr_work_ready", {"limit": 1}),
            (["work", "show", "W"], "awr_work_get", {"work": "W"}),
            (["search", "analysis", "--type", "work"], "awr_search", {"text": "analysis", "kind": "work"}),
        ]:
            self.equal_read(self.cli(cli), self.tool(tool, args, readonly=True))
        claim = self.cli(["work", "show", "W"])["work"]["active_claims"][0]
        self.assertEqual(len(claim["id"]), 26)

    def test_work_graph_recorded_and_verified_reads_agree_without_writing(self):
        self.session()  # A live claim is runtime state every read path must report.

        def graph_content(value):
            # The evaluation clock and the way the projection was obtained are the only allowed differences.
            return {k: v for k, v in value.items() if k not in TRANSPORT_FIELDS and k != "evaluated_at"}

        refreshed = self.cli(["work", "graph"])
        self.assertEqual(refreshed["freshness_basis"], "source_refresh")
        self.assertTrue(refreshed["source_refresh_performed"])
        self.assertFalse(refreshed["read_only"])
        before = self.state()
        cached = self.cli(["work", "graph", "--cached"])
        self.assertEqual(self.state(), before, "cached CLI read changed persisted state or source files")
        self.assertEqual(cached["freshness_basis"], "last_recorded_source_state")
        self.assertFalse(cached["source_refresh_performed"])
        self.assertTrue(cached["read_only"])
        self.assertTrue(cached["snapshot"]["coherent"])
        self.assertFalse(cached["snapshot"]["source_currentness_verified"])
        self.assertIsNone(cached["snapshot"]["source_refresh_revision"])
        mcp = self.tool("awr_work_graph", {}, readonly=True)
        self.assertEqual(mcp["freshness_basis"], "source_verified_readonly")
        self.assertFalse(mcp["source_refresh_performed"])
        self.assertTrue(mcp["read_only"])
        self.assertTrue(mcp["snapshot"]["source_currentness_verified"])
        for other in (cached, mcp):
            self.assertEqual(other["project_revision"], refreshed["project_revision"])
            self.assertEqual(other["snapshot"]["source_state_fingerprint"], refreshed["snapshot"]["source_state_fingerprint"])
            self.assertEqual(graph_content(other), graph_content(refreshed))
        node = next(n for n in cached["nodes"] if n["key"] == "W")
        self.assertEqual(len(node["active_claims"]), 1)

        # A source edit that no refresh has recorded yet: the recorded read keeps serving what was recorded,
        # while the verified MCP read refuses to answer from facts that no longer match the files.
        (self.root / "work.yaml").write_text(WORK.replace("Prepare customer analysis", "Changed source title"))
        before = self.state()
        stale = self.cli(["work", "graph", "--cached"])
        self.assertEqual(graph_content(stale), graph_content(refreshed))
        self.assertEqual(self.state(), before)
        self.assertEqual(self.tool("awr_work_graph", {}, error=True, readonly=True)["code"], "SourceStale")
        recorded = self.cli(["work", "graph"])
        self.assertGreater(recorded["project_revision"], refreshed["project_revision"])
        self.assertEqual(next(n for n in recorded["nodes"] if n["key"] == "W")["title"], "Changed source title")
        self.assertEqual(graph_content(self.cli(["work", "graph", "--cached"])), graph_content(recorded))
        self.assertEqual(graph_content(self.tool("awr_work_graph", {}, readonly=True)), graph_content(recorded))

    def test_context_hash_gaps_and_budget_parity(self):
        for work in ["W", "ABSENT"]:
            args = {"work": work, "detached": True, "budget": 5000}
            if work == "W":
                cli = self.cli(["context", "compile", "--work", work, "--detached"])
            else:
                cli, error = self.cli(["context", "compile", "--work", work, "--detached"], code=1)
                self.assertEqual(cli["error"], error)
                self.assertEqual(error["code"], "ContextIncomplete")
            mcp = self.tool("awr_context_compile", args, error=work != "W", readonly=True)
            self.equal_read(cli, mcp)  # No hash, rendered text or gap normalization.
        _, error = self.cli(["context", "compile", "--work", "W", "--detached", "--budget", "1"], code=1)
        self.assertEqual(error, self.tool("awr_context_compile", {"work": "W", "detached": True, "budget": 1}, error=True, readonly=True))

    def test_explicit_branch_reads_do_not_switch_defaults(self):
        branch = self.cli(["branch", "create", "review", "--actor", "fixture", "--reason", "Review shared work",
                           "--expected-revision", self.revision()])["branch"]["id"]
        for cli, tool, args in [
            (["status", "--branch", "review"], "awr_project_status", {"branch": branch}),
            (["ready", "--branch", branch], "awr_work_ready", {"branch": "review"}),
            (["work", "show", "W", "--branch", "review"], "awr_work_get", {"work": "W", "branch": branch}),
            (["branch", "context", "review", "--work", "W", "--detached"], "awr_context_compile",
             {"work": "W", "branch": branch, "detached": True}),
        ]:
            self.equal_read(self.cli(cli), self.tool(tool, args, readonly=True))
        self.assertIsNone(self.cli(["branch", "list"])["current_branch_id"])

    def test_event_and_evidence_mutation_receipts(self):
        expected = self.revision()
        snapshot = self.snapshot()
        payload = {"body": "Stored event payload, excluded from default human output"}
        (self.root / "payload.json").write_text(json.dumps(payload))
        cli = self.cli(["event", "append", "--type", "work.observed", "--work", "W", "--summary", "Review identified a next step",
                        "--payload", "payload.json", "--expected-revision", expected])
        self.assertEqual(cli["event"]["payload"], payload)
        self.assertEqual(cli["project_revision"], expected + 1)
        self.restore(snapshot)
        mcp = self.tool("awr_event_append", {"event_type": "work.observed", "work": "W", "summary": "Review identified a next step",
                                             "payload": payload, "expected_revision": expected})
        for value in [cli, mcp]:
            self.assertEqual(len(value["event"].pop("id")), 26)
            self.assertIsInstance(value["event"].pop("created_at"), int)
        self.assertEqual(cli, mcp)

        draft = {"external_key": "E", "work_item_key": "W", "evidence_type": "report", "level": "implemented",
                 "summary": "Evidence summary " * 24, "locator": "report.json", "command": "Documented command " * 24,
                 "scope": [f"scope-{i}" for i in range(24)], "branch_id": None}
        (self.root / "evidence.json").write_text(json.dumps(draft))
        expected = self.revision()
        snapshot = self.snapshot()
        cli = self.cli(["evidence", "add", "--input", "evidence.json", "--expected-revision", expected])
        self.assertEqual(cli["evidence"]["summary"], draft["summary"])
        self.assertEqual(cli["evidence"]["scope"], draft["scope"])
        self.restore(snapshot)
        args = {**draft, "expected_revision": expected, "work": draft["work_item_key"], "branch": "main"}
        del args["work_item_key"], args["branch_id"]
        mcp = self.tool("awr_evidence_record", args)
        for value in [cli, mcp]:
            self.assertEqual(len(value.pop("event_id")), 26)
            self.assertEqual(len(value["evidence"].pop("id")), 26)
        self.assertEqual(cli, mcp)

    def test_transitions_share_receipts_source_changes_and_rejections(self):
        sid = self.session()
        for action, field, message, status in [
            ("progress", "next_action", "Review the analysis", "in_progress"),
            ("block", "blocker", "Await customer input", "blocked"),
            ("unblock", "next_action", "Review restored input", "in_progress"),
            ("cancel", "next_action", "Record cancellation", "cancelled"),
            ("reopen", "next_action", "Replan the analysis", "planned"),
        ]:
            expected = self.revision()
            snapshot = self.snapshot()
            cli = self.cli(["work", action, "W", "--session", sid, "--reason", message,
                            "--" + field.replace("_", "-"), message, "--expected-revision", expected])
            source = (self.root / "work.yaml").read_bytes()
            work = self.cli(["work", "show", "W"])
            self.assert_projection_receipt(work)
            self.restore(snapshot)
            mcp = self.tool("awr_work_transition", {"work": "W", "action": action, "session": sid, "reason": message,
                                                    field: message, "expected_revision": expected})
            self.assertEqual(set(cli), set(mcp))
            for key in ["ok", "code", "project_revision", "source_refresh_performed", "source_write_performed", "write_outcome", "error"]:
                self.assertEqual(cli[key], mcp[key], key)
            self.assertEqual(cli["proposal"]["status"], "applied")
            self.assertEqual(mcp["proposal"]["status"], "applied")
            self.assertEqual(cli["event"]["event_type"], mcp["event"]["event_type"])
            self.assertEqual((self.root / "work.yaml").read_bytes(), source)
            current = self.tool("awr_work_get", {"work": "W"}, readonly=True)
            self.assert_projection_receipt(current)
            self.equal_read(work, current, replayed_projection=True)
            self.assertEqual(current["work"]["status"], status)
        expected = self.revision() - 1
        before = self.state()
        _, error = self.cli(["work", "progress", "W", "--session", sid, "--reason", "Inspect outdated revision",
                             "--next-action", "Refresh", "--expected-revision", expected], code=1)
        self.assertEqual(error, self.tool("awr_work_transition", {"work": "W", "action": "progress", "session": sid,
                        "reason": "Inspect outdated revision", "next_action": "Refresh", "expected_revision": expected}, error=True))
        self.assertEqual(self.state(), before)

    def test_typed_errors_and_stale_cas_do_not_write(self):
        for cli, name, args in [
            (["work", "show", "ABSENT"], "awr_work_get", {"work": "ABSENT"}),
            (["ready", "--limit", "0"], "awr_work_ready", {"limit": 0}),
            (["status", "--branch", "ABSENT"], "awr_project_status", {"branch": "ABSENT"}),
            (["search", "analysis", "--limit", "0"], "awr_search", {"text": "analysis", "limit": 0}),
        ]:
            _, error = self.cli(cli, code=1)
            self.assertEqual(error, self.tool(name, args, error=True, readonly=True))
        expected = self.revision()
        before = self.state()
        _, error = self.cli(["event", "append", "--type", "work.completed", "--summary", "Reserved event",
                             "--expected-revision", expected], code=1)
        self.assertEqual(error, self.tool("awr_event_append", {"event_type": "work.completed", "summary": "Reserved event",
                                                              "expected_revision": expected}, error=True))
        self.assertEqual(self.state(), before)
        (self.root / "work.yaml").write_text(WORK.replace("Draft the analysis", "Changed source next action"))
        before = self.state()
        _, error = self.cli(["evidence", "add", "--input", "absent.json", "--expected-revision", expected - 1], code=1)
        self.assertEqual(error["code"], "RevisionConflict")
        self.assertEqual(error, self.tool("awr_evidence_record", {"external_key": "E", "evidence_type": "report", "level": "implemented",
                        "summary": "Observe stale revision", "locator": "report.json", "scope": ["W"], "expected_revision": expected - 1}, error=True))
        self.assertEqual(self.state(), before, "stale expected revision must fail before source refresh")
        stale = self.tool("awr_project_status", {}, error=True, readonly=True)
        self.assertEqual(stale["error"]["code"], "SourceStale")
        self.assertFalse(stale["ok"])
        self.assertEqual(stale["organization"]["state"], "source_unreadable")
        self.assertFalse(stale["organization"]["business_execution_ready"])
        self.assertIsNone(stale["total"])
        refreshed = self.cli(["status"])
        self.assertGreater(refreshed["project_revision"], expected)
        self.equal_read(refreshed, self.tool("awr_project_status", {}, readonly=True))

    def test_completion_keeps_acceptance_evidence_and_claim_receipts(self):
        sid = self.session()
        self.cli(["work", "progress", "W", "--session", sid, "--reason", "Analysis drafted",
                  "--next-action", "Review delivery", "--expected-revision", self.revision()])
        sha, criterion = "a" * 40, "Deliver the reviewed analysis"
        completion = {"version": 1, "source_sha": sha,
                      "acceptance": [{"criterion": criterion, "evidence": ["E"]}]}
        (self.root / "completion.json").write_text(json.dumps(completion))
        expected = self.revision()
        args = {"work": "W", "action": "complete", "session": sid, "reason": "Deliver reviewed analysis",
                "completion": completion, "expected_revision": expected}
        command = ["work", "complete", "W", "--session", sid, "--reason", args["reason"],
                   "--input", self.root / "completion.json", "--expected-revision", expected]
        before = self.state()
        _, error = self.cli(command, code=1)
        self.assertEqual(error, self.tool("awr_work_transition", args, error=True))
        self.assertEqual(error["code"], "NotFound")
        self.assertEqual(before, self.state())
        verified_at = int(time.time() * 1000)
        report = {"version": 1, "work_item": "W", "source_sha": sha, "command": "review fixture analysis",
                  "scope": ["W"], "verified_at": verified_at,
                  "checks": [{"name": "review and delivery", "passed": True,
                              "details": "Reviewed fixture result", "criteria": [criterion]}]}
        report_bytes = json.dumps(report).encode()
        (self.root / "report.json").write_bytes(report_bytes)
        draft = {"external_key": "E", "work_item_key": "W", "evidence_type": "completion_report",
                 "level": "locally_verified", "summary": "Reviewed fixture result", "locator": "report.json",
                 "sha256": hashlib.sha256(report_bytes).hexdigest(), "source_sha": sha,
                 "command": report["command"], "scope": ["W"], "verified_at": verified_at}
        (self.root / "evidence.json").write_text(json.dumps(draft))
        self.cli(["evidence", "add", "--input", "evidence.json", "--expected-revision", self.revision()])
        snapshot = self.snapshot()
        args["expected_revision"] = self.revision()
        command[-1] = args["expected_revision"]
        cli = self.cli(command)
        source = (self.root / "work.yaml").read_bytes()
        work = self.cli(["work", "show", "W", "--source-sha", sha])
        self.assert_projection_receipt(work)
        self.restore(snapshot)
        mcp = self.tool("awr_work_transition", args)
        for key in ["ok", "code", "project_revision", "source_write_performed", "write_outcome", "error"]:
            self.assertEqual(cli[key], mcp[key], key)
        self.assertEqual(cli["event"]["event_type"], "work.completed")
        self.assertEqual(mcp["event"]["event_type"], "work.completed")
        self.assertEqual(cli["event"]["payload"]["released_claim_ids"], mcp["event"]["payload"]["released_claim_ids"])
        self.assertEqual(len(mcp["event"]["payload"]["released_claim_ids"]), 1)
        self.assertEqual(source, (self.root / "work.yaml").read_bytes())
        self.assertEqual(work["work"]["status"], "completed")
        self.assertEqual(work["work"]["active_claims"], [])
        current = self.tool("awr_work_get", {"work": "W", "source_sha": sha}, readonly=True)
        self.assert_projection_receipt(current)
        self.equal_read(work, current, replayed_projection=True)

    def test_bad_input_and_unknown_tool_contracts(self):
        sid = self.session()
        expected = self.revision()
        before = self.state()
        for body in ["{", json.dumps({"unexpected": True})]:
            (self.root / "input.json").write_text(body)
            for command in [
                ["evidence", "add", "--input", "input.json", "--expected-revision", expected],
                ["work", "complete", "W", "--session", sid, "--reason", "Inspect supplied mapping",
                 "--input", self.root / "input.json", "--expected-revision", expected],
            ]:
                _, error = self.cli(command, code=1)
                self.assertEqual(error["code"], "InvalidInput")
        malformed = self.tool("awr_evidence_record", {"expected_revision": expected, "unexpected": True}, error=True)
        self.assertEqual(malformed["code"], "InvalidInput")
        (self.root / "payload.json").write_text("{")
        _, error = self.cli(["event", "append", "--type", "work.observed", "--summary", "Inspect input",
                             "--payload", "payload.json", "--expected-revision", expected], code=1)
        self.assertEqual(error["code"], "InvalidInput")
        self.assertEqual(before, self.state())
        client = Client(OPTIONS.mcp, self.root)
        try:
            response = client.rpc("tools/call", {"name": "unsupported-tool", "arguments": {}})
            self.assertEqual(response["error"]["code"], -32601)
            self.assertNotIn("result", response)
        finally:
            client.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--awr", type=Path, required=True)
    parser.add_argument("--mcp", type=Path, required=True)
    parser.add_argument("--report", type=Path)
    OPTIONS = parser.parse_args()
    OPTIONS.awr = OPTIONS.awr.resolve(strict=True)
    OPTIONS.mcp = OPTIONS.mcp.resolve(strict=True)
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Parity)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if OPTIONS.report:
        OPTIONS.report.write_text(json.dumps({"suite": "cli-mcp-parity", "tests_run": result.testsRun,
            "passed": result.wasSuccessful(), "failures": [str(t) for t, _ in result.failures],
            "errors": [str(t) for t, _ in result.errors], "tools_exercised": sorted(TOOLS),
            "binaries": {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in [OPTIONS.awr, OPTIONS.mcp]},
            "evidence_level": "local_cli_and_mcp_stdio_only", "model_calls": 0}, indent=2) + "\n")
    raise SystemExit(0 if result.wasSuccessful() else 1)
